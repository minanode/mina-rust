//! Startup errors must identify the selected input and exit without a panic.
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn missing_and_malformed_genesis_inputs_exit_nonzero_without_panicking() {
    use mina_p2p_messages::binprot::BinProtWrite;
    // Enough bytes for builder protocol_constants(), but no ledger payload:
    // this must reach GenesisLoad(Err) in the event loop.
    let mut truncated = Vec::new();
    mina_node::core::constants::PROTOCOL_CONSTANTS
        .binprot_write(&mut truncated)
        .unwrap();
    let mut cases: Vec<(&str, Option<Vec<u8>>, &str)> = Vec::new();
    for flag in ["--config", "--genesis-config"] {
        cases.push((flag, None, "selected-genesis.json"));
        cases.push((
            flag,
            Some(b"{broken genesis".to_vec()),
            "selected-genesis.json",
        ));
    }
    cases.push(("--config", Some(b"{}".to_vec()), "no ledger"));
    cases.push(("--genesis-config", Some(truncated), "precomputed data"));
    let mut config: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/files/fork-export/fork/rust/fork_config.json"
    ))
    .unwrap();
    config.as_object_mut().unwrap().remove("proof");
    for (pointer, hint) in [
        ("/ledger/hash", "bad ledger hash"),
        ("/epoch_data/staking/seed", "epoch_data.staking.seed"),
        ("/epoch_data/next/seed", "epoch_data.next.seed"),
    ] {
        let mut malformed = config.clone();
        *malformed.pointer_mut(pointer).unwrap() = serde_json::json!("bad");
        cases.push((
            "--config",
            Some(serde_json::to_vec(&malformed).unwrap()),
            hint,
        ));
    }
    for (flag, bytes, hint) in cases {
        assert_startup_error("devnet", flag, bytes.as_deref(), hint);
    }
}

#[test]
fn malformed_consensus_constants_exit_nonzero_on_both_networks() {
    for network in ["devnet", "mainnet"] {
        for (field, value) in [
            ("slots_per_epoch", 0),
            ("slots_per_epoch", 2),
            ("grace_period_slots", 7140),
            ("slots_per_sub_window", 0),
            ("slots_per_sub_window", u32::MAX),
            ("delta", u32::MAX),
        ] {
            let config = serde_json::json!({
                "ledger": { "accounts": [] },
                "genesis": { field: value },
            });
            assert_startup_error(
                network,
                "--config",
                Some(&serde_json::to_vec(&config).unwrap()),
                field,
            );
        }
    }
}

fn assert_startup_error(network: &str, flag: &str, bytes: Option<&[u8]>, hint: &str) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("selected-genesis.json");
    if let Some(bytes) = bytes {
        std::fs::write(&input, bytes).unwrap();
    }
    let stderr = std::fs::File::create(dir.path().join("stderr")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mina"))
        .env_clear()
        .env("MINA_HOME", dir.path())
        .args([
            "--network",
            network,
            "node",
            "--seed",
            "--no-peers-discovery",
            "--port",
            "0",
            "--libp2p-port",
            "0",
            "--work-dir",
        ])
        .arg(dir.path().join("node"))
        .arg(flag)
        .arg(&input)
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(30) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("genesis startup failure did not stop the process");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let stderr = std::fs::read_to_string(dir.path().join("stderr")).unwrap();
    assert_eq!(status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("selected-genesis.json"), "{stderr}");
    assert!(stderr.contains(hint), "{stderr}");
    assert!(
        !stderr.contains("panicked") && !stderr.contains("not yet implemented"),
        "{stderr}"
    );
}
