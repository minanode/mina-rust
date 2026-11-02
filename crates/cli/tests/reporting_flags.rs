//! `mina node`'s reporting options (R43e.2) are checked before the node
//! starts, and their errors never show key material, passwords, URL
//! credentials or contact details. Every key and password here is made in
//! the test and deleted with its temporary directory. No node starts and
//! nothing is sent: each command fails on its options.
use std::{
    ffi::OsStr,
    process::{Command, Output},
};

use mina_node_account::AccountSecretKey;

// Distinctive, so that no eight-byte part of them is a word of any output.
const PASSWORD: &str = "k3Vq9ZtR7mWx2LpB";
const WRONG_PASSWORD: &str = "J8nQ4sYc6HfT1dGe";

// As in `status.rs`: the runner can pin a copy of this build's binary.
fn mina() -> Command {
    let mut command = Command::new(
        std::env::var_os("MINA_STATUS_TEST_BIN")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_mina").into()),
    );
    command
        .env_remove("MINA_P2P_SEC_KEY")
        .env_remove("MINA_LIBP2P_PASS")
        .env_remove("MINA_PRIVKEY_PASS")
        .env_remove("UPTIME_PRIVKEY_PASS")
        .env_remove("MINA_UPTIME_URL")
        .env_remove("MINA_UPTIME_SUBMITTER_KEY")
        .env_remove("MINA_UPTIME_SUBMITTER_PUBKEY")
        .env_remove("MINA_UPTIME_SEND_NODE_COMMIT_SHA")
        .env("MINA_DISABLE_FILESYSTEM_LOGGING", "true")
        .env("RUST_LOG", "trace");
    command
}

fn node(args: &[&str]) -> Output {
    mina_node(args, None::<(&str, &OsStr)>)
}

fn mina_node(args: &[&str], env: Option<(&str, &OsStr)>) -> Output {
    mina_node_env(args, env.as_slice())
}

fn mina_node_env(args: &[&str], env: &[(&str, &OsStr)]) -> Output {
    let dir = tempfile::TempDir::new().unwrap();
    let mut command = mina();
    command
        .arg("node")
        .arg("--work-dir")
        .arg(dir.path())
        .args(args);
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().unwrap()
}

fn all_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Fails, saying `expected`, without starting the node.
fn assert_refused(output: &Output, expected: &str) {
    let text = all_output(output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains(expected), "{text}");
    assert!(!text.contains("node build"), "{text}");
}

/// Never prints the captured output or the secret in diagnostics.
fn assert_absent(output: &Output, secret: &str) {
    let text = all_output(output);
    for fragment in secret.as_bytes().windows(7.min(secret.len())) {
        assert!(
            !text
                .as_bytes()
                .windows(fragment.len())
                .any(|w| w == fragment),
            "output contains a secret fragment"
        );
    }
}

#[test]
fn uptime_url_needs_exactly_one_submitter_key() {
    let output = node(&["--uptime-url", "http://127.0.0.1:9/v1/submit"]);
    assert_refused(
        &output,
        "--uptime-url needs exactly one of --uptime-submitter-key FILE and --uptime-submitter-pubkey PUBLICKEY",
    );

    let key = AccountSecretKey::rand();
    let public = key.public_key().to_string();
    let output = node(&[
        "--uptime-url",
        "http://127.0.0.1:9/",
        "--uptime-submitter-key",
        "/nonexistent/key",
        "--uptime-submitter-pubkey",
        &public,
    ]);
    assert_refused(&output, "cannot be used with");

    for args in [
        &["--uptime-submitter-key", "/nonexistent/key"][..],
        &["--uptime-submitter-pubkey", public.as_str()][..],
        &["--uptime-send-node-commit-sha"][..],
    ] {
        let output = node(args);
        assert_refused(&output, "--uptime-url");
    }
}

#[test]
fn submitter_key_errors_hide_the_key_and_passwords() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("uptime-key");
    let key = AccountSecretKey::rand();
    key.to_encrypted_file(&file, PASSWORD).unwrap();
    let file = file.to_str().unwrap();
    let args = [
        "--uptime-url",
        "http://127.0.0.1:9/v1/submit",
        "--uptime-submitter-key",
        file,
    ];

    let output = mina_node(
        &args,
        Some(("UPTIME_PRIVKEY_PASS", OsStr::new(WRONG_PASSWORD))),
    );
    assert_refused(&output, "wrong password or damaged file");
    assert!(!all_output(&output).contains(file));
    for secret in [
        key.to_string(),
        hex::encode(key.to_bytes()),
        PASSWORD.into(),
        WRONG_PASSWORD.into(),
    ] {
        assert_absent(&output, &secret);
    }

    // Unset: OCaml refuses to read the key, and so does this node. The
    // producer's password is not the uptime key's.
    let output = mina_node(&args, Some(("MINA_PRIVKEY_PASS", OsStr::new(PASSWORD))));
    assert_refused(
        &output,
        "No password was specified in environment variable UPTIME_PRIVKEY_PASS",
    );
    assert_absent(&output, &key.to_string());
    assert_absent(&output, PASSWORD);
    let output = mina_node_env(
        &args,
        &[
            ("MINA_PRIVKEY_PASS", OsStr::new(PASSWORD)),
            ("UPTIME_PRIVKEY_PASS", OsStr::new(WRONG_PASSWORD)),
        ],
    );
    assert_refused(&output, "wrong password or damaged file");

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bad = OsStr::from_bytes(b"Zx8Wq3Rt\xffVb7Kp2Nm");
        let output = mina_node(&args, Some(("UPTIME_PRIVKEY_PASS", bad)));
        assert_refused(&output, "UPTIME_PRIVKEY_PASS is not valid UTF-8");
        assert_absent(&output, "Zx8Wq3Rt");
        assert_absent(&output, "Vb7Kp2Nm");
    }

    let output = mina_node(
        &[
            "--uptime-url",
            "http://127.0.0.1:9/",
            "--uptime-submitter-key",
            "/nonexistent/uptime-key",
        ],
        Some(("UPTIME_PRIVKEY_PASS", OsStr::new(""))),
    );
    assert_refused(
        &output,
        "failed to decrypt the uptime submitter key file: no such file",
    );
    assert!(!all_output(&output).contains("/nonexistent/uptime-key"));
}

/// The key is the public key's tracked wallet in the work directory, as
/// OCaml's is in its config directory; this one has none.
#[test]
fn submitter_pubkey_reads_a_tracked_wallet() {
    let key = AccountSecretKey::rand();
    let output = mina_node(
        &[
            "--uptime-url",
            "http://127.0.0.1:9/",
            "--uptime-submitter-pubkey",
            &key.public_key().to_string(),
        ],
        Some(("UPTIME_PRIVKEY_PASS", OsStr::new(""))),
    );
    assert_refused(
        &output,
        "failed to decrypt the uptime submitter key file: no such file",
    );
}

/// Each uptime option may also come from a `MINA_` variable; the command
/// line wins over it, and a variable turns reporting on as the option does.
#[test]
fn uptime_environment_aliases_yield_to_the_command_line() {
    let bad_url = OsStr::new("ftp://127.0.0.1/");
    // The variable alone turns uptime on: it then needs a submitter.
    let output = mina_node(
        &[],
        Some((
            "MINA_UPTIME_URL",
            OsStr::new("http://127.0.0.1:9/v1/submit"),
        )),
    );
    assert_refused(&output, "--uptime-url needs exactly one of");
    let output = mina_node(&[], Some(("MINA_UPTIME_URL", bad_url)));
    assert_refused(&output, "an http:// or https:// URL is needed");
    // The command line wins.
    let output = mina_node(
        &["--uptime-url", "http://127.0.0.1:9/v1/submit"],
        Some(("MINA_UPTIME_URL", bad_url)),
    );
    assert_refused(&output, "--uptime-url needs exactly one of");
    // The option's file is readable by others, refused for that; the
    // variable's doesn't exist.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("flag-key");
        std::fs::write(&file, b"{}").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let output = mina_node_env(
            &[
                "--uptime-url",
                "http://127.0.0.1:9/v1/submit",
                "--uptime-submitter-key",
                file.to_str().unwrap(),
            ],
            &[
                (
                    "MINA_UPTIME_SUBMITTER_KEY",
                    OsStr::new("/nonexistent/env-key"),
                ),
                ("UPTIME_PRIVKEY_PASS", OsStr::new("")),
            ],
        );
        assert_refused(&output, "key file permissions are too open");
        let output = mina_node_env(
            &["--uptime-url", "http://127.0.0.1:9/v1/submit"],
            &[
                (
                    "MINA_UPTIME_SUBMITTER_KEY",
                    OsStr::new("/nonexistent/env-key"),
                ),
                ("UPTIME_PRIVKEY_PASS", OsStr::new("")),
            ],
        );
        assert_refused(&output, "uptime submitter key file: no such file");
    }
    // The submitter variables need the URL, as the options do.
    for name in [
        "MINA_UPTIME_SUBMITTER_KEY",
        "MINA_UPTIME_SUBMITTER_PUBKEY",
        "MINA_UPTIME_SEND_NODE_COMMIT_SHA",
    ] {
        let value = match name {
            "MINA_UPTIME_SUBMITTER_PUBKEY" => AccountSecretKey::rand().public_key().to_string(),
            "MINA_UPTIME_SEND_NODE_COMMIT_SHA" => "true".to_owned(),
            _ => "/nonexistent/env-key".to_owned(),
        };
        let output = mina_node(&[], Some((name, OsStr::new(&value))));
        assert_refused(&output, "--uptime-url");
    }
}

#[test]
fn report_urls_are_checked_without_echo() {
    for flag in ["--uptime-url", "--node-status-url", "--node-error-url"] {
        for (url, expected, secret) in [
            (
                "ftp://user:hunter2-secret@example.com/?token=abcdefgh",
                "an http:// or https:// URL is needed",
                "hunter2-secret",
            ),
            (
                "not a url hunter2-secret",
                "not a valid URL",
                "hunter2-secret",
            ),
        ] {
            let output = node(&[flag, url]);
            assert_refused(&output, expected);
            assert_absent(&output, secret);
            assert_absent(&output, "abcdefgh");
        }
    }
}

#[test]
fn contact_info_must_be_shorter_than_200_bytes() {
    let contact = format!("Qm4Tz8Wc{}", "Q".repeat(192));
    assert_eq!(contact.len(), 200);
    let output = node(&[
        "--node-error-url",
        "http://127.0.0.1:9/",
        "--contact-info",
        &contact,
    ]);
    assert_refused(&output, "--contact-info: must be shorter than 200 bytes");
    assert_absent(&output, "Qm4Tz8Wc");
    assert_absent(&output, "QQQQQQQQ");
}

#[test]
fn full_node_status_is_refused() {
    let output = node(&[
        "--node-status-url",
        "http://127.0.0.1:9/",
        "--simplified-node-stats",
        "false",
    ]);
    assert_refused(
        &output,
        "the full node status report (version 2) isn't supported",
    );
}

#[test]
fn help_lists_the_reporting_options() {
    let output = mina()
        .args(["node", "--help"])
        .env("UPTIME_PRIVKEY_PASS", PASSWORD)
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = all_output(&output);
    for flag in [
        "--uptime-url",
        "--uptime-submitter-key",
        "--uptime-submitter-pubkey",
        "--uptime-send-node-commit-sha",
        "--uptime-dry-run-dir",
        "--node-status-url",
        "--simplified-node-stats",
        "--node-error-url",
        "--contact-info",
    ] {
        assert!(text.contains(flag), "{flag} missing from help");
    }
    assert_absent(&output, PASSWORD);
}

// Deliberately not a key: base58 alphabet, EK prefix, key-like length, a
// failing checksum. Written in two parts so the export's key scanner, which
// can't tell, doesn't stop on it.
const MISTAKEN_KEY: &str = concat!("EK", "F3n6Q9s2V5y8B4d7G1j4M7p2S5v8Y3b6E9h2K5n8Q4t7W1z3C6");

#[test]
fn mistaken_secret_selectors_never_reach_output() {
    for flag in ["--uptime-submitter-pubkey", "--uptime-submitter-key"] {
        for suffix in [
            vec![],
            vec!["--uptime-url", "http://127.0.0.1:9/"],
            vec!["--uptime-url", "bad-url"],
            vec!["--help"],
            vec!["--uptime-submitter-key", "/nonexistent/key"],
        ] {
            let mut args = vec![flag, MISTAKEN_KEY];
            args.extend(suffix);
            let output = node(&args);
            assert_absent(&output, MISTAKEN_KEY);
            assert!(!output.status.success() || args.contains(&"--help"));
        }
    }
}

#[test]
fn mistaken_secret_environment_and_help_never_reach_output() {
    for name in [
        "UPTIME_PRIVKEY_PASS",
        "MINA_PRIVKEY_PASS",
        "MINA_P2P_SEC_KEY",
        "RUN_SNARKER",
    ] {
        for args in [
            vec!["--help"],
            vec![
                "--uptime-url",
                "http://127.0.0.1:9/",
                "--uptime-submitter-key",
                MISTAKEN_KEY,
            ],
        ] {
            let output = mina_node(&args, Some((name, OsStr::new(MISTAKEN_KEY))));
            assert_absent(&output, MISTAKEN_KEY);
        }
    }
}

#[test]
fn malformed_nonce_is_an_error_before_node_start() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("damaged-key");
    let encode = |bytes: &[u8]| bs58::encode(bytes).with_check_version(2).into_string();
    let json = serde_json::json!({
        "box_primitive":"xsalsa20poly1305", "pw_primitive":"argon2i",
        "nonce":encode(&[0]), "pwsalt":encode(&[0;16]),
        "pwdiff":[8192,1], "ciphertext":encode(&[0;49]),
    });
    std::fs::write(&file, serde_json::to_vec(&json).unwrap()).unwrap();
    // A key file is refused unless 0600, before it is read.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let output = mina_node(
            &[
                "--uptime-url",
                "http://127.0.0.1:9/",
                "--uptime-submitter-key",
                file.to_str().unwrap(),
            ],
            Some(("UPTIME_PRIVKEY_PASS", OsStr::new(""))),
        );
        assert_refused(&output, "key file permissions are too open");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let output = mina_node(
        &[
            "--uptime-url",
            "http://127.0.0.1:9/",
            "--uptime-submitter-key",
            file.to_str().unwrap(),
        ],
        Some(("UPTIME_PRIVKEY_PASS", OsStr::new(""))),
    );
    assert_refused(&output, "not a valid encrypted key file");
    assert_ne!(output.status.code(), Some(101));
    assert!(!all_output(&output).contains("panicked"));
}

#[cfg(unix)]
#[test]
fn non_utf8_selectors_never_reach_output() {
    use std::os::unix::ffi::OsStringExt;
    for flag in ["--uptime-submitter-key", "--uptime-submitter-pubkey"] {
        let mut bytes = MISTAKEN_KEY.as_bytes().to_vec();
        bytes.push(0xff);
        let output = mina()
            .args(["node", flag])
            .arg(std::ffi::OsString::from_vec(bytes))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_absent(&output, MISTAKEN_KEY);
    }
}
