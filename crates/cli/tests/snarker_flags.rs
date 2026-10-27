//! Exercise the built CLI: an accidentally pasted private key must not reach
//! clap diagnostics, usage, help, logs or panic output. Only fake markers are used.
use std::process::{Command, Output};

use rand::{Rng, SeedableRng};

fn mina() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mina"));
    command
        .env_clear()
        .env("RUST_LOG", "trace")
        .env("RUST_BACKTRACE", "full")
        .arg("node");
    command
}

fn marker() -> String {
    // Base58-shaped, deliberately not a generated or imported private key.
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut rng = rand::rngs::StdRng::seed_from_u64(52);
    let suffix: String = (0..50)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect();
    format!("EK{suffix}")
}

fn assert_no_marker(output: &Output, marker: &str) {
    for stream in [&output.stdout, &output.stderr] {
        for fragment in marker.as_bytes().windows(7) {
            assert!(
                !stream.windows(7).any(|window| window == fragment),
                "CLI output disclosed at least seven consecutive marker bytes"
            );
        }
    }
}

fn assert_rejected(output: &Output, marker: &str, hint: &str) {
    assert_no_marker(output, marker);
    assert_eq!(output.status.code(), Some(2), "expected a clap rejection");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(hint),
        "expected the input-free validation hint"
    );
    assert!(output.stdout.is_empty());
}

const FLAGS: [(&str, &str, &str); 2] = [
    (
        "--run-snarker",
        "MINA_RUN_SNARKER",
        "looks like a private key",
    ),
    (
        "--snarker-fee",
        "MINA_SNARKER_FEE",
        "expected MINA, like 0.1",
    ),
];

#[test]
fn snarker_flags_reject_without_echo() {
    let marker = marker();
    for (flag, _, hint) in FLAGS {
        for value in [marker.clone(), format!("{marker}.1.2")] {
            let output = mina().args([flag, &value]).output().unwrap();
            assert_rejected(&output, &marker, hint);

            let output = mina().arg(format!("{flag}={value}")).output().unwrap();
            assert_rejected(&output, &marker, hint);

            // Usage and unrelated validation errors must also be input-free,
            // regardless of which argument clap validates first.
            for args in [
                vec![flag, &value, "--port", "invalid-port"],
                vec!["--port", "invalid-port", flag, &value],
                vec![flag, &value, flag, &value],
                vec![flag, &value, "--help"],
            ] {
                let output = mina().args(args).output().unwrap();
                assert_no_marker(&output, &marker);
                assert!(matches!(output.status.code(), Some(0 | 2)));
            }
        }

        // Malformed keys exercise the underlying key-parser error; the fee
        // cases also cover multiple dots and the UTF-8 truncation boundary.
        for value in [format!(" {marker}"), format!("0.12345678é{marker}")] {
            let output = mina().args([flag, &value]).output().unwrap();
            assert_rejected(&output, &marker, "error:");
        }
    }

    let overflow = "18446744073.709551616";
    let output = mina().args(["--snarker-fee", overflow]).output().unwrap();
    assert_rejected(&output, overflow, "does not fit in 64 bits");
}

#[test]
fn snarker_environment_rejects_and_help_hides_values() {
    let marker = marker();
    for (_, env, hint) in FLAGS {
        let output = mina().env(env, &marker).output().unwrap();
        assert_rejected(&output, &marker, hint);

        for args in [
            vec!["--help"],
            vec!["-h"],
            vec!["--port", "invalid-port"],
            vec!["--run-snarker"],
            vec!["--snarker-fee"],
        ] {
            let output = mina().env(env, &marker).args(args).output().unwrap();
            assert_no_marker(&output, &marker);
            assert!(matches!(output.status.code(), Some(0 | 2)));
        }

        // Underlying public-key, fee syntax and overflow errors via env too.
        for value in [format!(" {marker}"), format!("0.12345678é{marker}")] {
            let output = mina().env(env, value).output().unwrap();
            assert_rejected(&output, &marker, "error:");
        }
    }

    let overflow = "18446744073.709551616";
    let output = mina().env("MINA_SNARKER_FEE", overflow).output().unwrap();
    assert_rejected(&output, overflow, "does not fit in 64 bits");
}

#[cfg(unix)]
#[test]
fn non_utf8_snarker_inputs_reject_without_echo() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let marker = marker();
    let mut bytes = marker.as_bytes().to_vec();
    bytes.push(0xff);
    let value = OsString::from_vec(bytes);
    for (flag, env, _) in FLAGS {
        let output = mina().arg(flag).arg(&value).output().unwrap();
        assert_rejected(&output, &marker, "expected a UTF-8 public key or fee");

        let output = mina().env(env, &value).output().unwrap();
        assert_rejected(&output, &marker, "expected a UTF-8 public key or fee");

        let output = mina().env(env, &value).arg("--help").output().unwrap();
        assert_no_marker(&output, &marker);
        assert!(output.status.success());
    }
}
