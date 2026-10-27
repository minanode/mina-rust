//! Offline regression using the public, manifest-pinned mainnet genesis binary.
//! Run with MINA_MAINNET_GENESIS=/path/to/mesa-mainnet.bin cargo test --release
//! -p ledger-tool --test genesis_roots -- --ignored --nocapture (under build lock).
use std::{
    fs,
    io::{Cursor, Read},
    path::Path,
    process::Command,
};

use mina_node::ProtocolConstants;
use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite, Nat0},
    v2::{self, MinaBaseAccountBinableArgStableV2 as Account},
};

struct Scratch(std::path::PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn check(repo: &Path, directory: &Path, label: &str, success: bool) {
    let output = Command::new("python3")
        .arg(repo.join("tools/regression/inputs.py"))
        .args(["check", "--genesis-roots", "--ledger-tool"])
        .arg(env!("CARGO_BIN_EXE_ledger-tool"))
        .arg("--dir")
        .arg(directory)
        .arg(repo.join("tools/regression/inputs/genesis.txt"))
        .output()
        .unwrap();
    println!(
        "{label}: exit={}\n{}",
        output.status.code().unwrap(),
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        output.status.success(),
        success,
        "{label}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    if !success {
        assert!(String::from_utf8_lossy(&output.stdout)
            .contains("differs from the canonical mainnet genesis"));
    }
}

#[test]
#[ignore = "requires the 255 MB public mainnet genesis fixture; hashes all accounts offline"]
fn rejects_balance_mutations_with_unchanged_stored_roots_and_caches() {
    let source = std::env::var_os("MINA_MAINNET_GENESIS").expect("set MINA_MAINNET_GENESIS");
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scratch =
        Scratch(std::env::temp_dir().join(format!("mina-genesis-roots-{}", std::process::id())));
    fs::create_dir(&scratch.0).unwrap();
    let directory = scratch.0.join("genesis-mainnet");
    fs::create_dir(&directory).unwrap();
    let canonical =
        fs::read(repo.join("genesis_ledgers/mesa-mainnet-without-ledgers.json")).unwrap();
    // Independent expected values: OCaml 4.0.2 packaged config, collected by P11d.
    let pins: serde_json::Value = serde_json::from_slice(&canonical).unwrap();
    let ocaml: serde_json::Value = serde_json::from_slice(
        &fs::read(repo.join("tests/files/ocaml/mainnet-4.0.2-constants.json")).unwrap(),
    )
    .unwrap();
    for field in [
        "genesis_ledger_hash",
        "staking_epoch_ledger_hash",
        "next_epoch_ledger_hash",
    ] {
        assert_eq!(pins[field], ocaml["constants"][field][0]["value"]);
    }
    fs::write(
        directory.join("mesa-mainnet-without-ledgers.json"),
        canonical,
    )
    .unwrap();
    let binary = directory.join("mesa-mainnet.bin");
    let mut bytes = fs::read(source).unwrap();
    fs::write(&binary, &bytes).unwrap();
    check(&repo, &scratch.0, "untouched", true);

    // Decode only to locate each first account. Preserve every other byte,
    // including the stored roots and every cached inner hash, when mutating.
    let mut reader = Cursor::new(bytes.as_slice());
    if bytes.starts_with(b"MINAGEN1") {
        reader.set_position(8);
        let mut length = [0; 4];
        reader.read_exact(&mut length).unwrap();
        reader.set_position(12 + u32::from_le_bytes(length) as u64);
    }
    ProtocolConstants::binprot_read(&mut reader).unwrap();
    let mut mutations = Vec::new();
    for ledger in ["genesis", "staking", "next"] {
        let count = Nat0::binprot_read(&mut reader).unwrap().0;
        assert!(count > 0);
        let start = reader.position() as usize;
        let mut first = Account::binprot_read(&mut reader).unwrap();
        let end = reader.position() as usize;
        let balance = first.balance.0 .0.as_u64();
        first.balance =
            v2::CurrencyBalanceStableV1(v2::CurrencyAmountStableV1((balance + 1).into()));
        let mut encoded = Vec::new();
        first.binprot_write(&mut encoded).unwrap();
        assert_eq!(encoded.len(), end - start, "mutation must preserve framing");
        mutations.push((ledger, start, end, encoded));
        for _ in 1..count {
            Account::binprot_read(&mut reader).unwrap();
        }
        v2::LedgerHash::binprot_read(&mut reader).unwrap();
        Vec::<(u64, v2::LedgerHash)>::binprot_read(&mut reader).unwrap();
        if ledger != "genesis" {
            v2::EpochSeed::binprot_read(&mut reader).unwrap();
        }
    }
    for (ledger, start, end, encoded) in mutations {
        let original = bytes[start..end].to_vec();
        bytes[start..end].copy_from_slice(&encoded);
        fs::write(&binary, &bytes).unwrap();
        check(
            &repo,
            &scratch.0,
            &format!("{ledger} balance +1 nanomina, stored roots/caches unchanged"),
            false,
        );
        bytes[start..end].copy_from_slice(&original);
    }
}
