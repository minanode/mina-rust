//! Fork export publication and stop guards against a loopback HTTP stub.
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

const MANIFEST: &str = include_str!("../../../tests/files/fork-export/fork/rust/manifest.json");
const CONFIG: &str = include_str!("../../../tests/files/fork-export/fork/rust/fork_config.json");

// The captured 4.0.2 manifest remains a historical fixture. OCaml 4.0.4
// Account.Hardfork.empty copies Account.empty; migration preserves these roots.
// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/mina_base/account.ml#L1131-L1158>
fn manifest_4_0_4() -> Value {
    let mut manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    manifest["daemon_migrated"] = manifest["daemon"].clone();
    for name in ["ledger", "staking", "next"] {
        let oracle: Value = serde_json::from_str(include_str!(
            "../../../tests/files/fork-export/ocaml-4.0.4.json"
        ))
        .unwrap();
        assert_eq!(
            manifest["ledgers"][name]["hash"],
            oracle["fork_roots"][name]
        );
        manifest["ledgers"][name]["migrated_hash"] = oracle["fork_roots"][name].clone();
    }
    manifest
}

fn export(manifest: Value, args: &[&str]) -> (std::process::Output, tempfile::TempDir) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let body = format!("{manifest}\n{CONFIG}");
    let allow = args.contains(&"--allow-before-stop");
    let server = thread::spawn(move || {
        for graphql in [true, false] {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing request");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buf[..n]);
                if request.windows(4).any(|v| v == b"\r\n\r\n") {
                    break;
                }
            }
            let header_end = request.windows(4).position(|v| v == b"\r\n\r\n").unwrap() + 4;
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while request.len() < header_end + length {
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buf[..n]);
            }
            let request = String::from_utf8_lossy(&request);
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            assert!(if graphql {
                path == "/graphql"
            } else {
                path.starts_with("/hardfork/config")
            });
            if !graphql {
                assert_eq!(path.contains("allow_before_stop=true"), allow, "{path}");
            }
            let response = if graphql {
                r#"{"data":{"syncStatus":"SYNCED","networkID":"mina:devnet"}}"#
            } else {
                &body
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            )
            .unwrap();
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mina"))
        .args([
            "--network",
            "devnet",
            "fork",
            "export",
            "--node",
            &base,
            "--out",
        ])
        .arg(temp.path().join("fork"))
        .args(args)
        .output()
        .unwrap();
    server.join().unwrap();
    (output, temp)
}

#[test]
fn failed_verification_never_publishes_or_activates() {
    let mut manifest = manifest_4_0_4();
    manifest["ledgers"]["ledger"]["hash"] = manifest["ledgers"]["staking"]["hash"].clone();
    let (output, temp) = export(manifest, &["--verify"]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert!(
        !temp.path().join("fork").exists(),
        "failed export was published"
    );
    for entry in std::fs::read_dir(temp.path()).unwrap() {
        assert!(!entry.unwrap().path().join("activated").exists());
    }
}

#[test]
fn verified_export_publishes_complete_package() {
    let (output, temp) = export(manifest_4_0_4(), &["--verify"]);
    assert!(output.status.success(), "{output:?}");
    for file in [
        "fork_config.json",
        "daemon.json",
        "daemon-migrated.json",
        "manifest.json",
        "activated",
    ] {
        assert!(temp.path().join("fork").join(file).exists(), "{file}");
    }
}

#[test]
fn legacy_4_0_2_migrated_roots_fail_4_0_4_verification() {
    let (output, temp) = export(serde_json::from_str(MANIFEST).unwrap(), &["--verify"]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert!(!temp.path().join("fork").exists());
}

#[test]
fn every_selector_requires_stop_or_explicit_override() {
    for selector in [
        vec![],
        vec!["--height", "545439"],
        vec![
            "--state-hash",
            "3NLvdGJk7DjwHeMW9Hhmv2AMrRnaou5NXfU2KMfBF3MXx6TEQEui",
        ],
    ] {
        let mut manifest = manifest_4_0_4();
        manifest["inputs"]["stop_reached"] = false.into();
        let (output, temp) = export(manifest.clone(), &selector);
        assert_eq!(output.status.code(), Some(3), "{selector:?}: {output:?}");
        assert!(!temp.path().join("fork").exists());
        let mut allowed = selector.clone();
        allowed.push("--allow-before-stop");
        let (output, temp) = export(manifest, &allowed);
        assert!(output.status.success(), "{allowed:?}: {output:?}");
        assert!(temp.path().join("fork/activated").exists());
    }
}
