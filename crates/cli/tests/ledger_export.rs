//! Ledger export refusal guidance, against a local HTTP stub (no node started).
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn refusal(kind: &str, code: &str, body: &str) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let path = format!("/ledger/export/{kind}");
    let code = code.to_owned();
    let body = body.to_owned();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        for expected in ["/graphql", path.as_str()] {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing request: {expected}");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            let header_end = loop {
                if let Some(end) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                    break end + 4;
                }
                let n = stream.read(&mut buffer).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buffer[..n]);
            };
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            assert_eq!(
                headers.lines().next().unwrap().split_whitespace().nth(1),
                Some(expected)
            );
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while request.len() < header_end + content_length {
                let n = stream.read(&mut buffer).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buffer[..n]);
            }
            let (code, body) = if expected == "/graphql" {
                (
                    "200 OK",
                    r#"{"data":{"syncStatus":"SYNCED","networkID":"mina:devnet"}}"#,
                )
            } else {
                (code.as_str(), body.as_str())
            };
            write!(stream, "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let output = Command::new(env!("CARGO_BIN_EXE_mina"))
        .args([
            "--network",
            "devnet",
            "ledger",
            "export",
            kind,
            "--node",
            &base,
        ])
        .output()
        .unwrap();
    server.join().unwrap();
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--enable-ledger-export"), "{stderr}");
    assert!(stderr.contains("private HTTP endpoint"), "{stderr}");
    assert!(
        output.stdout.is_empty(),
        "refusals must not write an export"
    );
}

#[test]
fn export_refusal_explains_operator_flag_and_private_endpoint() {
    for kind in [
        "staged-ledger",
        "snarked-ledger",
        "staking-epoch-ledger",
        "next-epoch-ledger",
    ] {
        refusal(
            kind,
            "403 Forbidden",
            r#"{"error":"ledger export is disabled"}"#,
        );
        refusal(kind, "404 Not Found", "");
    }
}
