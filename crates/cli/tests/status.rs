//! Process-level exit codes and the HTTP allowlist, using recorded relay2 data.
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

const STATUS: &str = include_str!("fixtures/status/relay2-status.json");
const BUILD: &str = include_str!("fixtures/status/relay2-build.json");
const ACTIONS: &str = include_str!("fixtures/status/relay2-actions.json");

// Shared build directories can be reused by another worktree between compiling
// and running tests. Allow the runner to pin a copy of this build's binary.
fn mina() -> Command {
    Command::new(
        std::env::var_os("MINA_STATUS_TEST_BIN")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_mina").into()),
    )
}

fn run(status: &str, optional_status: &str, flags: &[&str]) -> Output {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let status = status.to_owned();
    let optional_status = optional_status.to_owned();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut paths = Vec::new();
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0; 1024];
                    while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                        let n = stream.read(&mut buffer).unwrap();
                        if n == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..n]);
                    }
                    let request = String::from_utf8(request).unwrap();
                    let first = request.lines().next().unwrap();
                    assert!(first.starts_with("GET "), "{first}");
                    let path = first.split_whitespace().nth(1).unwrap();
                    paths.push(path.to_owned());
                    let (code, body) = match path {
                        "/status" => {
                            let code = if status == "redirect" {
                                "302 Found"
                            } else {
                                "200 OK"
                            };
                            (code, status.as_str())
                        }
                        "/build_env" => (optional_status.as_str(), BUILD),
                        "/stats/actions?id=latest" => (optional_status.as_str(), ACTIONS),
                        _ => panic!("unexpected request: {path}"),
                    };
                    let location = if code == "302 Found" {
                        "Location: /unexpected\r\n"
                    } else {
                        ""
                    };
                    write!(stream, "HTTP/1.1 {code}\r\n{location}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if stop_rx.try_recv().is_ok() {
                        break;
                    }
                    assert!(Instant::now() < deadline, "CLI did not finish");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("{e}"),
            }
        }
        paths
    });
    let output = mina()
        .args(["node", "status", "--node", &url, "--timeout", "1"])
        .args(flags)
        .output()
        .unwrap();
    stop_tx.send(()).unwrap();
    let paths = server.join().unwrap();
    assert_eq!(paths.first().map(String::as_str), Some("/status"));
    if flags.contains(&"--status-only") {
        assert_eq!(paths, ["/status"]);
    }
    output
}

#[test]
fn recorded_response_human_json_and_healthy_exit() {
    let output = run(
        STATUS,
        "200 OK",
        &["--check", "--max-tip-age", "18446744073709551615"],
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("OK:"));
    let output = run(STATUS, "200 OK", &["--json"]);
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["status"]["peers"]["connected"], 6);
    assert_eq!(
        v["status"]["build_commit"],
        "85225b2eba3c1fc16ed6164e53e23500a85f97bf"
    );
    assert_eq!(v["status"]["best_tip"]["height"], 564635);
    let output = run(STATUS, "200 OK", &[]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("6 libp2p / 0 WebRTC"));
}

#[test]
fn unhealthy_exit_and_one_line_reason() {
    let output = run(STATUS, "200 OK", &["--check", "--min-peers", "7"]);
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(text.contains("connected peers 6 below 7"));
}

#[test]
fn malformed_status_is_json_error_and_optional_failure_is_only_warning() {
    let output = run("null", "200 OK", &["--json", "--check"]);
    assert_eq!(output.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["healthy"], false);
    assert!(v["status"].is_null());
    let output = run(
        STATUS,
        "503 Service Unavailable",
        &["--check", "--json", "--max-tip-age", "18446744073709551615"],
    );
    assert_eq!(output.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(v["status"]["build_commit"].is_null());
    assert!(v["status"]["verification"].is_null());
    assert!(!v["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn unreachable_endpoint_and_invalid_flags_exit_nonzero() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let output = mina()
        .args([
            "node",
            "status",
            "--node",
            &url,
            "--json",
            "--check",
            "--timeout",
            "1",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["healthy"], false);
    let output = mina()
        .args(["node", "status", "--timeout", "0"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn redirects_are_errors_and_never_followed() {
    let output = run("redirect", "200 OK", &["--json", "--check"]);
    assert_eq!(output.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(v["reasons"][0].as_str().unwrap().contains("HTTP 302"));
}

#[test]
fn stalled_http_response_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match listener.accept() {
                Ok((_stream, _)) => {
                    thread::sleep(Duration::from_secs(2));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("{e}"),
            }
        }
    });
    let start = Instant::now();
    let output = mina()
        // `--url` is the hidden older name of `--node`.
        .args(["node", "status", "--url", &url, "--check", "--timeout", "1"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(String::from_utf8_lossy(&output.stdout).contains("timed out"));
    server.join().unwrap();
}

#[test]
fn status_only_sends_exactly_one_get() {
    let output = run(
        STATUS,
        "503 Service Unavailable",
        &[
            "--status-only",
            "--check",
            "--json",
            "--max-tip-age",
            "18446744073709551615",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["status"]["peers"]["connected"], 6);
    assert!(v["status"]["build_commit"].is_null());
    assert!(v["status"]["verification"].is_null());
}
