//! The report command's tests, and those that need `mina`'s own command line
//! or panic hook. The rest live with the shared code, in `mina-node-report`.

use std::{fs, path::Path};

use clap::{CommandFactory, Parser};
use mina_node::{account::AccountSecretKey, p2p::identity::SecretKey};
use mina_node_report::{
    bundle::{build, test_support::*, NodeInfo, StoreCheck},
    crash::{
        enable, newest_crash, now_ms, now_rfc3339, read_last_start, StartRecord, CRASH_DIR,
        LAST_START,
    },
    logs,
    redact::{redact_argv, Redactor, KEY_FILE, PATH_VALUE, REDACTED},
    system::ProcessInfo,
};

use super::{collect, ReportArgs, StoreCheckMode};

fn cli() -> clap::Command {
    crate::commands::MinaCli::command()
}

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

fn redactor() -> Redactor {
    Redactor::new(false, [], Some("/home/tester".into()))
}

#[test]
fn argv_keeps_safe_values_and_hides_the_rest() {
    let p2p = SecretKey::rand().to_string();
    let redacted = redact_argv(
        &argv(&[
            "mina",
            "--network",
            "devnet",
            "node",
            "-d",
            "/data/mina",
            "--port=3650",
            "--libp2p-port",
            "8810",
            "--p2p-secret-key",
            &p2p,
            "--producer-key",
            "/home/tester/keys/producer.json",
            "--libp2p-keypair=/home/tester/libp2p.json",
            "--rng-seed",
            "00ff",
            "--frontier-store",
            "positional-password",
            "--some-future-flag=hidden-value",
        ]),
        cli(),
    );
    assert_eq!(
        redacted,
        argv(&[
            "mina",
            "--network",
            "devnet",
            "node",
            "-d",
            "<path>/mina",
            "--port=3650",
            "--libp2p-port",
            "8810",
            "--p2p-secret-key",
            REDACTED,
            "--producer-key",
            KEY_FILE,
            "--libp2p-keypair=<key-file>",
            "--rng-seed",
            REDACTED,
            "--frontier-store",
            REDACTED,
            "--some-future-flag=<redacted>",
        ])
    );
}

#[test]
fn argv_shows_harmless_new_flags_and_scrubs_worker_origin() {
    let redacted = redact_argv(
        &argv(&[
            "mina", "node", "--no-frontier-store", "--allow-worker-control",
            "--worker-control-origin=https://user:password@dash.example.test:4200/path?token=secret",
            "--log-level=debug", "--log-file-size-mb=5", "--log-file-count=3",
        ]),
        cli(),
    );
    assert_eq!(
        redacted,
        argv(&[
            "mina",
            "node",
            "--no-frontier-store",
            "--allow-worker-control",
            "--worker-control-origin=https://dash.example.test:4200/<redacted>",
            "--log-level=debug",
            "--log-file-size-mb=5",
            "--log-file-count=3",
        ])
    );
}

#[test]
fn argv_scrubs_webrtc_file_and_keeps_range_and_network_id() {
    let redacted = redact_argv(
        &argv(&[
            "mina",
            "node",
            "--webrtc-ice-servers",
            "/private/turn.json",
            "--webrtc-udp-ports=40000-40999",
            "--webrtc-nat-ip",
            "203.0.113.4",
        ]),
        cli(),
    );
    assert_eq!(
        redacted,
        argv(&[
            "mina",
            "node",
            "--webrtc-ice-servers",
            PATH_VALUE,
            "--webrtc-udp-ports=40000-40999",
            "--webrtc-nat-ip",
            "203.0.113.4",
        ])
    );
    assert_ne!(redactor().text(&redacted[6]), "203.0.113.4");
    assert_eq!(
        Redactor::new(true, [], Some("/home/tester".into())).text(&redacted[6]),
        "203.0.113.4"
    );
}

#[test]
fn argv_keeps_no_credentials_or_key_paths() {
    let redacted = redact_argv(
        &argv(&[
            "/home/tester/bin/mina",
            "node",
            "--peer-list-url",
            "https://user:hunter22@seeds.example.org:8443/devnet/peers.txt?auth=abc123&v=2#t=1",
            "--config",
            "/home/tester/keys/producer-key.json",
            "--genesis-config=/srv/mina/mesa-devnet-without-ledgers.json",
            "--peer-list-file",
            "/srv/0123456789abcdef0123456789abcdef.txt",
            "--log-path",
            "/home/tester/secret-logs",
        ]),
        cli(),
    );
    assert_eq!(
        redacted,
        argv(&[
            "mina",
            "node",
            "--peer-list-url",
            "https://seeds.example.org:8443/<redacted>",
            "--config",
            PATH_VALUE,
            "--genesis-config=<path>/mesa-devnet-without-ledgers.json",
            "--peer-list-file",
            PATH_VALUE,
            "--log-path",
            PATH_VALUE,
        ])
    );
    let joined = redacted.join(" ");
    for leak in [
        "user",
        "hunter22",
        "auth",
        "abc123",
        "tester",
        "keys",
        "0123456789",
    ] {
        assert!(!joined.contains(leak), "{leak} kept: {joined}");
    }
    // The text redactor leaves what the argv layer kept alone.
    let kept = &redacted[3];
    assert_eq!(&Redactor::new(false, [], None).text(kept), kept);
    let unparsable = redact_argv(
        &argv(&["mina", "node", "--peer-list-url", "not a url?token=x"]),
        cli(),
    );
    assert_eq!(unparsable[3], "<url>");
    // A token in the URL's path goes with the path.
    let in_path = redact_argv(
        &argv(&[
            "mina",
            "node",
            "--peer-list-url",
            "https://seeds.example.test/sk_test_ABC123/peers",
        ]),
        cli(),
    );
    assert_eq!(in_path[3], "https://seeds.example.test/<redacted>");
}

#[test]
fn argv_short_forms_hide_secrets() {
    let p2p = SecretKey::rand().to_string();
    let joined = format!("-s{p2p}");
    let redacted = redact_argv(
        &argv(&["mina", "node", "-s", &p2p, &joined, "-p3650"]),
        cli(),
    );
    assert_eq!(
        redacted,
        argv(&["mina", "node", "-s", REDACTED, "-s<redacted>", "-p3650"])
    );
}

/// A bundle built from inputs that hold every kind of secret shows none.
#[test]
fn a_report_leaks_no_secrets() {
    let account = AccountSecretKey::rand().to_string();
    let p2p_key = SecretKey::rand();
    let p2p = p2p_key.to_string();
    let peer = p2p_key.public_key().peer_id();
    let libp2p_peer = libp2p_identity::PeerId::try_from(peer).unwrap().to_string();
    let password = "correct-horse-battery";
    let key_path = "/home/tester/.mina/keys/block-producer.json";
    let key_file = "{\"box_primitive\":\"xsalsa20poly1305\",\"pw_primitive\":\"argon2i\",\
                    \"nonce\":\"6pcvpWSLkMi393dT5VSLR6ft56AWKkCYRqJoYia\",\
                    \"pwsalt\":\"ASoBkV3NsY7ZRuxztyPJdmJCiz3R\",\"pwdiff\":[134217728,6],\
                    \"ciphertext\":\"Dmq1Qd8uNbZRT5NWkWkzZ8ZGX8VZF5GrhTZxbcqKL5VQBkY\"}";
    let ip = "34.136.105.213";
    let ip6 = "2001:db8:85a3::8a2e:370:7334";

    let mut i = inputs();
    i.description = Some(format!("it broke in /home/tester/.mina after I set MINA_PRIVKEY_PASS={password}; log in /Users/visiting-user/.mina"));
    i.node = Ok(NodeInfo {
        network: Some("devnet".into()),
        sync_status: Some("Synced".into()),
        ..Default::default()
    });
    i.start = Some(Ok(start(redact_argv(
        &[
            "mina",
            "node",
            "--producer-key",
            key_path,
            "--p2p-secret-key",
            &p2p,
            "--libp2p-external-ip",
            ip,
            password,
        ]
        .map(str::to_owned),
        cli(),
    ))));
    i.start_process = Some(ProcessInfo {
        rss_bytes: Some(1 << 30),
    });
    i.crash = Some((
        "crash-2000-7-0.json".into(),
        Ok(crash_record(
            &format!("failed to decrypt {key_path} with {password}: key {account}"),
            "crates/node/src/account.rs:1:1",
            None,
        )),
        1,
    ));
    let log = format!(
        "2026-10-22T20:00:00Z ERROR mina_node: kind=\"KeyError\" read {key_file}\n\
         2026-10-22T20:00:01Z  WARN p2p: peer {libp2p_peer} at /ip4/{ip}/tcp/8302/p2p/{libp2p_peer}\n\
         2026-10-22T20:00:02Z  WARN p2p: peer {peer} from [{ip6}]:8302 secret {p2p}\n\
         2026-10-22T20:00:03Z ERROR mina_node: MINA_P2P_SEC_KEY={p2p} MINA_PRIVKEY_PASS='{password}'\n\
         2026-10-22T20:00:04Z ERROR mina_node: file_name=\"{key_path}\"\n"
    );
    let scan = |r: &Redactor| {
        vec![logs::scan(
            "/home/tester/.mina/mina.log.2026-10-22",
            &log,
            1,
            false,
            100,
            r,
        )]
    };
    i.store = Some(StoreCheck {
        dir: "/home/tester/.mina/ebfce0/frontier".into(),
        passed: Some(true),
        output: format!("written by {password}"),
        note: None,
    });

    let r = Redactor::new(false, [password.to_owned()], Some("/home/tester".into()));
    i.logs = scan(&r);
    let report = build(&i, &r, 100);
    let peer = peer.to_string();
    let leaks = [
        account.as_str(),
        p2p.as_str(),
        password,
        key_path,
        "block-producer",
        "6pcvpWSLkMi393dT5VSLR6ft56AWKkCYRqJoYia",
        "ASoBkV3NsY7ZRuxztyPJdmJCiz3R",
        "Dmq1Qd8uNbZRT5NWkWkzZ8ZGX8VZF5GrhTZxbcqKL5VQBkY",
        ip,
        ip6,
        peer.as_str(),
        libp2p_peer.as_str(),
        "tester",
        "/home/tester",
        "/Users/visiting-user",
        "visiting-user",
    ];
    for text in [&report.markdown, &report.summary, &report.title] {
        for leak in leaks {
            assert!(!text.contains(leak), "{leak} leaked:\n{text}");
        }
        // Not even a fragment of a key.
        for key in [&account, &p2p] {
            for fragment in key.as_bytes().windows(12) {
                assert!(
                    !text.as_bytes().windows(12).any(|w| w == fragment),
                    "a key fragment leaked"
                );
            }
        }
    }
    assert!(report.markdown.contains(REDACTED));
    assert!(
        report.markdown.contains("`MINA_PRIVKEY_PASS`"),
        "env names are kept"
    );
    assert!(report.markdown.contains("--producer-key <key-file>"));

    // With --include-network-ids, network ids stay; secrets still go.
    let r = Redactor::new(true, [password.to_owned()], Some("/home/tester".into()));
    i.logs = scan(&r);
    let report = build(&i, &r, 100);
    assert!(report.markdown.contains(ip) && report.markdown.contains(&libp2p_peer));
    for leak in [
        account.as_str(),
        p2p.as_str(),
        password,
        key_path,
        "/home/tester",
        "tester",
        "/Users/visiting-user",
        "visiting-user",
    ] {
        assert!(!report.markdown.contains(leak), "{leak} leaked");
    }
}

/// Set in the child process the test below starts.
const CHILD_WORK_DIR: &str = "MINA_REPORT_TEST_CRASH_WORK_DIR";
const SECRET: &str = "child-secret-pass-1234";
/// An unlabeled 32-byte key in hex.
const HEX_KEY: &str = "5f4dcc3b5aa765d61d8327deb882cf992b95990a9151374abd8ff8c5a7a0fe08";

fn crash_start(args: Vec<String>) -> StartRecord {
    StartRecord {
        started_at: now_rfc3339(),
        unix_ms: now_ms(),
        pid: std::process::id(),
        version: "test".into(),
        commit: "0".repeat(40),
        network: "devnet".into(),
        http_port: Some(3650),
        log_dir: None,
        frontier_store_dir: None,
        snark_worker: None,
        args,
        env_set: vec![],
    }
}

#[inline(never)]
fn crash_now(height: u32) -> ! {
    let key = mina_node::account::AccountSecretKey::rand();
    panic!(
        "frontier root missing at height {height} with {SECRET}, key {key}, raw {HEX_KEY}, \
         fetching https://seeds.example.org/peers?token=tok-777"
    );
}

/// In the child: install the node's panic hook, as `mina` does, and
/// panic. In the parent: run the child and read its crash file.
#[test]
fn panic_hook_writes_a_crash_file() {
    if let Some(dir) = std::env::var_os(CHILD_WORK_DIR) {
        enable(
            Path::new(&dir),
            &crash_start(vec!["mina".into(), "node".into(), REDACTED.into()]),
            &[SECRET],
        )
        .unwrap();
        std::panic::set_hook(Box::new(crate::panic_hook::new_hook));
        crash_now(42);
    }
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "commands::report::tests::panic_hook_writes_a_crash_file",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_WORK_DIR, dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success(), "the child did not panic");

    let start = read_last_start(dir.path()).unwrap().unwrap();
    assert_eq!(start.http_port, Some(3650));
    let (path, crash, count) = newest_crash(dir.path()).expect("a crash file");
    let crash = crash.unwrap();
    assert_eq!(count, 1);
    assert!(path.starts_with(dir.path().join(CRASH_DIR)));
    assert_eq!(crash.pid, start.pid);
    assert!(!crash.thread.is_empty());
    assert!(
        crash
            .message
            .starts_with("frontier root missing at height 42"),
        "{}",
        crash.message
    );
    // The registered secret, the key and the URL's token: the full
    // redactor, not only the registered values.
    assert!(!crash.message.contains(SECRET), "{}", crash.message);
    assert!(
        crash.message.contains("key <private-key>"),
        "{}",
        crash.message
    );
    assert!(!crash.message.contains("tok-777"), "{}", crash.message);
    assert!(!crash.message.contains(HEX_KEY), "{}", crash.message);
    assert!(
        crash.message.contains("raw <redacted>"),
        "{}",
        crash.message
    );
    assert!(crash.location.as_deref().unwrap().contains("tests.rs:"));
    assert_eq!(crash.commit, mina_node::BuildEnv::get().git.commit_hash);
    assert_eq!(crash.args, vec!["mina", "node", REDACTED]);
    let function = crash.function.as_deref().unwrap_or_default();
    assert!(
        function.ends_with("tests::crash_now"),
        "panicking function {function:?}, backtrace:\n{}",
        crash.backtrace
    );
    assert!(!crash.backtrace.contains("rust_begin_unwind"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("mina report"), "{stderr}");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600, "crash file");
        assert_eq!(mode(&dir.path().join(LAST_START)), 0o600, "last-start.json");
        assert_eq!(mode(&dir.path().join(CRASH_DIR)), 0o700, "crashes/");
    }
}

// The command line and the whole collection path.

#[derive(Debug, Parser)]
struct Cli {
    #[command(flatten)]
    args: ReportArgs,
}

#[test]
fn defaults() {
    let args = Cli::try_parse_from(["report"]).unwrap().args;
    assert_eq!(args.repo, "minanode/mina-rust");
    assert_eq!(args.store_check, StoreCheckMode::Root);
    assert_eq!(args.log_lines, 100);
    assert!(!args.include_network_ids && !args.yes && !args.gh);
}

/// A node answering like a real one, for the whole collection path.
#[test]
fn a_report_is_collected_from_a_node_and_a_work_dir() {
    use std::{io::Write, net::TcpListener};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let n = std::io::Read::read(&mut stream, &mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..n]).into_owned();
            let body = if request.starts_with("GET /status ") {
                r#"{"network":"devnet","transition_frontier":{"sync":{"status":"Synced","phase":"Synced"},"best_tip":{"height":7}},"peers":[],"block_producer":null}"#
            } else if request.starts_with("GET /build_env ") {
                r#"{"git":{"commit_hash":"abc"}}"#
            } else {
                "null"
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let work = tempfile::tempdir().unwrap();
    std::fs::write(
        work.path().join("mina.log.1"),
        "2026-10-22T20:00:00Z ERROR mina_node: kind=\"SomethingFailed\" at 10.0.0.9\n",
    )
    .unwrap();
    let args = Cli::try_parse_from([
        "report",
        "--node",
        &format!("http://127.0.0.1:{port}"),
        "--store-check",
        "off",
    ])
    .unwrap()
    .args;
    let mut redactor = Redactor::new(false, [], None);
    let inputs = collect(&args, work.path(), Some("it failed".into()), &redactor).unwrap();
    server.join().unwrap();
    let node = inputs.node.as_ref().unwrap();
    assert_eq!(node.sync_status.as_deref(), Some("Synced"));
    assert_eq!((node.producer, node.worker), (Some(false), Some(false)));
    let report = mina_node_report::build(&inputs, &mut redactor, 100);
    assert!(
        report.title.starts_with("Error: SomethingFailed [fp:"),
        "{}",
        report.title
    );
    assert!(report.markdown.contains("| SNARK worker | off |"));
    assert!(!report.markdown.contains("10.0.0.9"));
}
