//! `mina wallet generate` never shows the secret key it writes
//! (R35-key-output, P5.0a). The keys and passwords here are throwaway: made in
//! the test, kept in a temporary directory and deleted with it.
use std::process::{Command, Output};

use mina_node_account::AccountSecretKey;

const PASSWORD: &str = "throwaway-test-password";

// As in `status.rs`: the runner can pin a copy of this build's binary.
fn mina() -> Command {
    let mut command = Command::new(
        std::env::var_os("MINA_STATUS_TEST_BIN")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_mina").into()),
    );
    command
        .env_remove("MINA_ENC_KEY")
        .env_remove("MINA_SEC_KEY")
        .env_remove("MINA_P2P_SEC_KEY")
        .env_remove("MINA_RUN_SNARKER")
        .env_remove("MINA_LIBP2P_PASS")
        .env_remove("MINA_PRIVKEY_PASS")
        .env("RUST_LOG", "trace");
    command
}

fn all_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_no_secret(output: &str, secret_key: &AccountSecretKey) {
    assert!(
        !output.contains(&secret_key.to_string()),
        "the output holds the base58 secret key"
    );
    let hex = hex::encode(secret_key.to_bytes());
    assert!(
        !output.contains(&hex),
        "the output holds the secret key's hex"
    );
    assert!(!output.contains(PASSWORD), "the output holds the password");
}

#[test]
fn generated_key_reaches_only_its_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("producer-key");

    let output = mina()
        .args(["wallet", "generate", "--output"])
        .arg(&file)
        .env("MINA_PRIVKEY_PASS", PASSWORD)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", all_output(&output));

    let secret_key =
        AccountSecretKey::from_encrypted_file(file.to_str().unwrap(), PASSWORD).unwrap();
    let public_key = secret_key.public_key().to_string();
    let text = all_output(&output);
    assert_no_secret(&text, &secret_key);
    // The public key is what OCaml's `generate-keypair` prints.
    assert!(
        text.contains(&format!("Public key: {public_key}")),
        "{text}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("producer-key.pub")).unwrap(),
        public_key
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "OCaml's Secret_file.write sets 0600");
    }
}

#[test]
fn imported_key_reaches_only_its_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("producer-key");
    let secret_key = AccountSecretKey::rand();

    let output = mina()
        .args(["wallet", "generate", "--output"])
        .arg(&file)
        .env("MINA_SEC_KEY", secret_key.to_string())
        .env("MINA_PRIVKEY_PASS", PASSWORD)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", all_output(&output));
    assert_no_secret(&all_output(&output), &secret_key);

    let loaded = AccountSecretKey::from_encrypted_file(file.to_str().unwrap(), PASSWORD).unwrap();
    assert_eq!(loaded.public_key(), secret_key.public_key());
}

/// CLI-REF: the older command path, `misc mina-encrypted-key --file`, runs
/// `wallet generate`, and the older variable `MINA_ENC_KEY` is still read.
#[test]
fn the_older_command_and_variable_still_work() {
    let dir = tempfile::TempDir::new().unwrap();
    for (index, path) in [
        &["misc", "mina-encrypted-key", "--file"][..],
        &["keys", "mina-encrypted-key", "-f"],
    ]
    .into_iter()
    .enumerate()
    {
        let file = dir.path().join(format!("producer-key-{index}"));
        let secret_key = AccountSecretKey::rand();
        let output = mina()
            .args(path)
            .arg(&file)
            .env("MINA_ENC_KEY", secret_key.to_string())
            .env("MINA_PRIVKEY_PASS", PASSWORD)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", all_output(&output));
        let text = all_output(&output);
        assert_no_secret(&text, &secret_key);
        assert!(text.starts_with("Keypair generated\n"), "{text}");
        let loaded =
            AccountSecretKey::from_encrypted_file(file.to_str().unwrap(), PASSWORD).unwrap();
        assert_eq!(loaded.public_key(), secret_key.public_key());
    }
}

/// CLI-REF fix round 1: MINA_ENC_KEY is read only when no key was given by
/// `--secret-key` or MINA_SEC_KEY, so an invalid stale value doesn't stop a
/// run that has a key. Alone, an invalid value is refused without echo.
#[test]
fn the_older_key_variable_is_read_only_without_a_key() {
    const GARBAGE: &str = "zz-not-an-account-secret-zz";
    let dir = tempfile::TempDir::new().unwrap();
    for (index, from_flag) in [false, true].into_iter().enumerate() {
        let file = dir.path().join(format!("producer-key-{index}"));
        let secret_key = AccountSecretKey::rand();
        let mut command = mina();
        command.args(["wallet", "generate", "--output"]).arg(&file);
        if from_flag {
            command.arg("--secret-key").arg(secret_key.to_string());
        } else {
            command.env("MINA_SEC_KEY", secret_key.to_string());
        }
        let output = command
            .env("MINA_ENC_KEY", GARBAGE)
            .env("MINA_PRIVKEY_PASS", PASSWORD)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", all_output(&output));
        assert_no_secret(&all_output(&output), &secret_key);
        let loaded =
            AccountSecretKey::from_encrypted_file(file.to_str().unwrap(), PASSWORD).unwrap();
        assert_eq!(loaded.public_key(), secret_key.public_key());
    }

    let file = dir.path().join("producer-key-old");
    let output = mina()
        .args(["wallet", "generate", "--output"])
        .arg(&file)
        .env("MINA_ENC_KEY", GARBAGE)
        .env("MINA_PRIVKEY_PASS", PASSWORD)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", all_output(&output));
    assert_no_fragments(&output, GARBAGE);
    assert!(all_output(&output).contains("MINA_ENC_KEY: invalid secret key"));
    assert!(!file.exists());
}

#[test]
fn help_hides_secret_environment_values() {
    let secret_key = AccountSecretKey::rand();
    let output = mina()
        .args(["wallet", "generate", "--help"])
        .env("MINA_SEC_KEY", secret_key.to_string())
        .env("MINA_PRIVKEY_PASS", PASSWORD)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", all_output(&output));
    let text = all_output(&output);
    assert!(text.contains("MINA_SEC_KEY"), "{text}");
    assert!(!text.contains("MINA_ENC_KEY"), "{text}");
    assert_no_secret(&text, &secret_key);
}

// Every raw-key argument, including the intentionally plaintext pair commands.
// Neither those commands nor a node may run when parsing rejects a value.
const KEY_ARGS: &[(&[&str], &str, &str, bool)] = &[
    (
        &["wallet", "generate", "--output", "/nonexistent-key"],
        "--secret-key",
        "MINA_SEC_KEY",
        false,
    ),
    (
        &["keys", "mina-key-pair"],
        "--secret-key",
        "MINA_SEC_KEY",
        false,
    ),
    (
        &["keys", "p2p-key-pair"],
        "--p2p-secret-key",
        "MINA_P2P_SEC_KEY",
        true,
    ),
    (&["node"], "--p2p-secret-key", "MINA_P2P_SEC_KEY", true),
    (&["node"], "--run-snarker", "MINA_RUN_SNARKER", false),
];

fn assert_no_fragments(output: &Output, input: &str) {
    let text = all_output(output);
    // Check every eight-byte window, not only the complete value. Never put
    // captured output or any part of the input in assertion diagnostics.
    for fragment in input.as_bytes().windows(8) {
        assert!(
            !text.as_bytes().windows(8).any(|window| window == fragment),
            "output contains a secret fragment"
        );
    }
}

#[test]
fn rejected_keys_never_reach_output() {
    let account_key = AccountSecretKey::rand().to_string();
    let p2p_key = mina_node::p2p::identity::SecretKey::rand().to_string();
    for &(args, flag, env, is_p2p) in KEY_ARGS {
        let key = if is_p2p { &p2p_key } else { &account_key };
        for value in [
            format!("{key} \t\n"),
            format!(" \t{key}"),
            format!("{key}0dummy-invalid-secret"),
        ] {
            for from_env in [false, true] {
                let mut command = mina();
                command.args(args);
                if from_env {
                    command.env(env, &value);
                } else {
                    command.arg(flag).arg(&value);
                }
                let output = command.output().unwrap();
                assert_no_fragments(&output, &value);
                assert_eq!(output.status.code(), Some(2));
                assert!(output.stdout.is_empty());
                // P5.2 takes a public fee receiver here. Keep the no-fragment
                // check above while expecting its input-free public-key hint.
                let expected = if flag == "--run-snarker" {
                    if value.starts_with("EK") {
                        "error: this looks like a private key: the SNARK worker takes the public key \
                         (B62...) that receives the fees, as OCaml's --run-snark-worker does"
                    } else {
                        "error: not a valid public key: expected B62..."
                    }
                } else {
                    "error: invalid secret key"
                };
                assert_eq!(String::from_utf8_lossy(&output.stderr).trim(), expected);
            }
        }
    }
}

// Passwords are positional arguments as well as environment inputs. All UTF-8
// strings are valid passwords; these commands fail before any network/node work.
const PASSWORD_ARGS: &[(&[&str], &str)] = &[
    (
        &["misc", "mina-encrypted-key", "--file", "."],
        "MINA_PRIVKEY_PASS",
    ),
    (
        &["wallet", "generate", "--output", "."],
        "MINA_PRIVKEY_PASS",
    ),
    (&["node"], "MINA_LIBP2P_PASS"),
    (&["node"], "MINA_PRIVKEY_PASS"),
    (&["wallet", "address"], "MINA_PRIVKEY_PASS"),
    (
        &["wallet", "balance", "--from", "/nonexistent-r35-key"],
        "MINA_PRIVKEY_PASS",
    ),
    (&["wallet", "send"], "MINA_PRIVKEY_PASS"),
];

#[test]
fn passwords_stay_private_on_command_errors_and_help() {
    for &(args, env) in PASSWORD_ARGS {
        for value in [
            "r35fixture-92f8451a-0e56-4e1e-8035-b9e0a87f40c9",
            "r35fixture-92f8451a-0e56-4e1e-8035-b9e0a87f40c9 \t\n",
        ] {
            for from_env in [false, true] {
                let mut command = mina();
                command.args(args);
                if from_env {
                    command.env(env, value);
                } else {
                    // producer_key_password follows the optional libp2p_password.
                    if args == ["node"] && env == "MINA_PRIVKEY_PASS" {
                        command.arg("dummy-libp2p-password");
                    }
                    command.arg(value);
                }
                if args == ["node"] {
                    // Fail in clap before starting any node or opening logs.
                    command.args(["--port", "invalid-port"]);
                }
                let output = command.output().unwrap();
                assert_no_fragments(&output, value);
                assert!(!output.status.success());
            }
            let output = mina()
                .args(args)
                .arg("--help")
                .env(env, value)
                .output()
                .unwrap();
            assert_no_fragments(&output, value);
            assert!(output.status.success());
        }
    }
    // The SNARK key's implicit env name also needs hide_env_values.
    let value = AccountSecretKey::rand().to_string();
    let output = mina()
        .args(["node", "--help"])
        .env("MINA_RUN_SNARKER", &value)
        .output()
        .unwrap();
    assert_no_fragments(&output, &value);
    assert!(output.status.success());
}

// Passwords are environment inputs, not command line arguments: argv is
// visible to other processes for as long as this process runs. A password
// typed as an argument is refused, without being echoed, and the message
// says where the password is read from instead. With `node`, the port
// error is only there so no node starts while the arguments are still
// accepted; the refusal must come first.
#[test]
fn passwords_are_refused_on_the_command_line() {
    // Not the file's PASSWORD: its last eight letters occur in the
    // refusal message itself, and no fragment of this value does.
    const VALUE: &str = "r35fixture-92f8451a-0e56-4e1e-8035-b9e0a87f40c9";
    let dir = tempfile::TempDir::new().unwrap();
    for &(args, env) in PASSWORD_ARGS {
        // `node` takes two passwords and names both: a single value was
        // most likely meant for the producer key, but it lands in the
        // libp2p password's positional.
        let expected: &[&str] = if args == ["node"] {
            &["MINA_PRIVKEY_PASS", "MINA_LIBP2P_PASS"]
        } else {
            &["MINA_PRIVKEY_PASS"]
        };
        let mut command = mina();
        command.args(args);
        if args == ["node"] && env == "MINA_PRIVKEY_PASS" {
            // producer_key_password follows the optional libp2p_password.
            command.arg("dummy-libp2p-password");
        }
        command.arg(VALUE);
        if args == ["node"] {
            command.args(["--port", "invalid-port"]);
        }
        let output = command.output().unwrap();
        assert_no_fragments(&output, VALUE);
        assert_eq!(output.status.code(), Some(2), "{env}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("not the command line"), "{env}");
        for name in expected {
            assert!(stderr.contains(name), "{env}: {name}");
        }
    }
    // Nothing ran: the command that would have written a key did not.
    assert!(!dir.path().join("k").exists());
}

#[cfg(unix)]
#[test]
fn non_utf8_secrets_fail_without_echo() {
    use std::os::unix::ffi::OsStringExt;
    let marker = "dummy-secret-prefix-middle-suffix";
    let value = std::ffi::OsString::from_vec([marker.as_bytes(), &[0xff]].concat());
    for &(args, flag, env, _) in KEY_ARGS {
        for from_env in [false, true] {
            let mut command = mina();
            command.args(args);
            if from_env {
                command.env(env, &value);
            } else {
                command.arg(flag).arg(&value);
            }
            let output = command.output().unwrap();
            assert_no_fragments(&output, marker);
            assert_eq!(output.status.code(), Some(2));
            let expected = if flag == "--run-snarker" {
                "error: expected a UTF-8 public key or fee"
            } else {
                "error: invalid secret key"
            };
            assert_eq!(String::from_utf8_lossy(&output.stderr).trim(), expected);
        }
    }
    for &(args, env) in PASSWORD_ARGS {
        for from_env in [false, true] {
            let mut command = mina();
            command.args(args);
            if from_env {
                command.env(env, &value);
            } else {
                if args == ["node"] && env == "MINA_PRIVKEY_PASS" {
                    command.arg("dummy-libp2p-password");
                }
                command.arg(&value);
            }
            let output = command.output().unwrap();
            assert_no_fragments(&output, marker);
            assert!(!output.status.success());
        }
    }
}
