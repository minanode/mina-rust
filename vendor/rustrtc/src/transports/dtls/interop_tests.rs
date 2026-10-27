use super::*;
use crate::transports::ice::IceSocketWrapper;
use anyhow::Result;
use dtls::cipher_suite::CipherSuiteId;
use dtls::config::Config;
use dtls::crypto::Certificate as DtlsCertificate;
use dtls::extension::extension_use_srtp::SrtpProtectionProfile;
use dtls::listener::listen;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tracing::{error, info};
use webrtc_util::conn::Listener;

/// Requires webrtc-dtls server - environment dependent
#[tokio::test]
#[ignore]
async fn test_interop_rustrtc_client_webrtc_server() -> Result<()> {
    rustls::crypto::CryptoProvider::install_default(rustls::crypto::ring::default_provider()).ok();

    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    // 1. Setup webrtc-dtls server
    // Generate certificate for webrtc-dtls
    let cert = DtlsCertificate::generate_self_signed(vec!["localhost".to_string()])?;

    let config = Config {
        certificates: vec![cert],
        cipher_suites: vec![CipherSuiteId::Tls_Ecdhe_Ecdsa_With_Aes_128_Gcm_Sha256],
        srtp_protection_profiles: vec![SrtpProtectionProfile::Srtp_Aead_Aes_128_Gcm],
        ..Default::default()
    };

    let listener = listen("127.0.0.1:0", config).await?;
    let server_addr = listener.addr().await?;

    info!("webrtc-dtls server listening on {}", server_addr);

    tokio::spawn(async move {
        while let Ok((conn, _)) = listener.accept().await {
            info!("webrtc-dtls server accepted connection");
            tokio::spawn(async move {
                let mut buf = vec![0u8; 1024];
                while let Ok(n) = conn.recv(&mut buf).await {
                    info!(
                        "webrtc-dtls server received: {}",
                        String::from_utf8_lossy(&buf[..n])
                    );
                    if let Err(e) = conn.send(&buf[..n]).await {
                        error!("webrtc-dtls server send error: {}", e);
                        break;
                    }
                }
            });
        }
    });

    // 2. Setup rustrtc client
    let client_socket = UdpSocket::bind("127.0.0.1:0").await?;
    // Clone socket for the read loop
    let socket_reader = Arc::new(client_socket);
    let socket_writer = socket_reader.clone();

    let (socket_tx, _) = tokio::sync::watch::channel(Some(IceSocketWrapper::Udp(socket_writer)));
    let client_conn = IceConn::new(socket_tx.subscribe(), server_addr, None);

    // Start read loop
    let conn_clone = client_conn.clone();
    let reader_clone = socket_reader.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        let mut marshal_buf = Vec::new();
        loop {
            match reader_clone.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    let packet = Bytes::copy_from_slice(&buf[..len]);
                    conn_clone.receive(packet, addr, &mut marshal_buf).await;
                }
                Err(e) => {
                    error!("Client socket read error: {}", e);
                    break;
                }
            }
        }
    });

    let cert = generate_certificate()?;
    let (client_dtls, mut incoming_rx, runner) =
        DtlsTransport::new(client_conn, cert, true, 1500, None).await?;
    tokio::spawn(runner);

    // Wait for handshake
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Check state
    {
        let state = client_dtls.get_state();
        match state {
            DtlsState::Connected(..) => info!("rustrtc client connected!"),
            _ => panic!("rustrtc client failed to connect, state: {}", state),
        }
    }

    // Send data
    let msg = b"hello world";
    info!("rustrtc client sending: {:?}", String::from_utf8_lossy(msg));
    client_dtls.send(Bytes::from_static(msg)).await?;

    // Receive echo
    info!("rustrtc client waiting for echo...");
    let echo = incoming_rx
        .recv()
        .await
        .ok_or(anyhow::anyhow!("Channel closed"))?;
    info!(
        "rustrtc client received: {:?}",
        String::from_utf8_lossy(&echo)
    );

    assert_eq!(&echo[..], msg);
    info!("Echo verified!");

    Ok(())
}

/// OpenSSL interop test - fails with LibreSSL due to compatibility issues
#[tokio::test]
#[ignore]
async fn test_interop_rustrtc_client_openssl_server() -> Result<()> {
    use std::process::{Command, Stdio};

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug")),
        )
        .try_init();

    // Check if openssl is available and supports DTLS
    let openssl_version = match Command::new("openssl").arg("version").output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout).to_string(),
        Err(_) => {
            info!("openssl command not found, skipping test");
            return Ok(());
        }
    };
    info!("OpenSSL version: {}", openssl_version.trim());

    // Check if s_server supports -dtls1_2
    let help_output = Command::new("openssl").args(["s_server", "-help"]).output();
    if let Ok(output) = help_output {
        let help_text = String::from_utf8_lossy(&output.stderr);
        if !help_text.contains("dtls1_2") {
            info!("openssl s_server does not support -dtls1_2, skipping test");
            return Ok(());
        }
    }

    let temp_dir = std::env::temp_dir();
    let key_path = temp_dir.join("rustrtc_test_key.pem");
    let cert_path = temp_dir.join("rustrtc_test_cert.pem");

    // Generate RSA certificate for the server
    let status = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-keyout",
            key_path.to_str().unwrap(),
            "-out",
            cert_path.to_str().unwrap(),
            "-days",
            "1",
            "-nodes",
            "-subj",
            "/CN=localhost",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    assert!(status.success(), "Failed to generate test certificate");

    // Find a free port by binding a temporary UDP socket
    let tmp_socket = std::net::UdpSocket::bind("127.0.0.1:0")?;
    let port = tmp_socket.local_addr()?.port();
    drop(tmp_socket);

    let port_str = port.to_string();
    let mut server_child = Command::new("openssl")
        .args([
            "s_server",
            "-dtls1_2",
            "-accept",
            &port_str,
            "-cert",
            cert_path.to_str().unwrap(),
            "-key",
            key_path.to_str().unwrap(),
            "-use_srtp",
            "SRTP_AES128_CM_SHA1_80",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    info!("OpenSSL s_server starting on port {}...", port);

    // Wait for the server to start listening
    tokio::time::sleep(Duration::from_millis(500)).await;

    let server_addr: std::net::SocketAddr = format!("127.0.0.1:{}", port).parse()?;
    let client_socket = UdpSocket::bind("127.0.0.1:0").await?;
    info!("Client socket bound to: {}", client_socket.local_addr()?);

    let socket_reader = Arc::new(client_socket);
    let socket_writer = socket_reader.clone();

    let (socket_tx, _) = tokio::sync::watch::channel(Some(IceSocketWrapper::Udp(socket_writer)));
    let client_conn = IceConn::new(socket_tx.subscribe(), server_addr, None);

    // Start read loop - forwards incoming UDP packets to IceConn
    let conn_clone = client_conn.clone();
    let reader_clone = socket_reader.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        let mut marshal_buf = Vec::new();
        loop {
            match reader_clone.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    let packet = Bytes::copy_from_slice(&buf[..len]);
                    conn_clone.receive(packet, addr, &mut marshal_buf).await;
                }
                Err(e) => {
                    error!("Client socket read error: {}", e);
                    break;
                }
            }
        }
    });

    let cert = generate_certificate()?;
    let (client_dtls, _incoming_rx, runner) =
        DtlsTransport::new(client_conn, cert, true, 1500, None).await?;
    tokio::spawn(runner);

    // Poll for handshake completion
    let mut success = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let state = client_dtls.get_state();
        if let DtlsState::Connected(..) = state {
            info!("rustrtc DTLS handshake with OpenSSL succeeded!");
            success = true;
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            error!("Timeout waiting for OpenSSL handshake, state: {}", state);
            break;
        }
    }

    let _ = server_child.kill();
    let _ = server_child.wait();
    let _ = std::fs::remove_file(&key_path);
    let _ = std::fs::remove_file(&cert_path);

    assert!(success, "DTLS Handshake with OpenSSL failed");

    Ok(())
}

// ==================== rustrtc SERVER interop ====================
// The tests above exercise rustrtc as a CLIENT against foreign servers; the
// ones below drive the opposite (and production-critical) direction: foreign
// clients (pion/Go, OpenSSL) connecting to a rustrtc server-role transport.

/// Boot a rustrtc server-role DTLS transport on a real UDP socket with a
/// wire→IceConn pump. Returns (transport, appdata_rx, server_addr).
async fn spawn_rustrtc_dtls_server() -> Result<(
    Arc<DtlsTransport>,
    tokio::sync::mpsc::UnboundedReceiver<Bytes>,
    std::net::SocketAddr,
)> {
    let server_socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
    let server_addr = server_socket.local_addr()?;
    let (server_socket_tx, _) =
        tokio::sync::watch::channel(Some(IceSocketWrapper::Udp(server_socket.clone())));
    // Remote is latched from the first inbound packet (port 0 = unknown).
    let server_conn = IceConn::new(
        server_socket_tx.subscribe(),
        "0.0.0.0:0".parse().unwrap(),
        None,
    );
    let (server_dtls, server_rx, runner) = DtlsTransport::new(
        server_conn.clone(),
        generate_certificate()?,
        false,
        1500,
        None,
    )
    .await?;
    tokio::spawn(runner);

    let pump_conn = server_conn.clone();
    let pump_socket = server_socket.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        let mut marshal_buf = Vec::new();
        while let Ok((len, addr)) = pump_socket.recv_from(&mut buf).await {
            let packet = Bytes::copy_from_slice(&buf[..len]);
            pump_conn.receive(packet, addr, &mut marshal_buf).await;
        }
    });

    Ok((server_dtls, server_rx, server_addr))
}

/// Fail unless the transport reaches `Connected` within `secs`.
async fn wait_connected(dtls: &Arc<DtlsTransport>, secs: u64) -> Result<()> {
    let mut state_rx = dtls.subscribe_state();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if matches!(state_rx.borrow().clone(), DtlsState::Connected(..)) {
            return Ok(());
        }
        if matches!(
            state_rx.borrow().clone(),
            DtlsState::Failed | DtlsState::Closed
        ) {
            anyhow::bail!(
                "transport reached terminal state {}",
                state_rx.borrow().clone()
            );
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            anyhow::bail!(
                "not connected within {secs}s (state {})",
                state_rx.borrow().clone()
            );
        }
        tokio::time::timeout_at(deadline, state_rx.changed()).await??;
    }
}

/// pion/dtls (Go) — the stack behind pion/webrtc — dials a rustrtc server:
/// full handshake with pion's default cipher offer (0xC02B first), plus one
/// application-data echo round trip. Skips gracefully when no Go toolchain
/// (or module download) is available.
#[tokio::test]
async fn test_interop_pion_client_rustrtc_server() -> Result<()> {
    use std::process::{Command, Stdio};

    if Command::new("go").arg("version").output().is_err() {
        info!("go toolchain not found, skipping pion interop");
        return Ok(());
    }
    let client_dir = format!("{}/interop/go/pion-dtls-client", env!("CARGO_MANIFEST_DIR"));
    let bin = std::env::temp_dir()
        .join(format!("pion-dtls-client-{}", std::process::id()))
        .to_string_lossy()
        .into_owned();
    let build = Command::new("go")
        .args(["build", "-o", &bin, "."])
        .current_dir(&client_dir)
        .output()?;
    if !build.status.success() {
        info!(
            "go build failed (offline CI?), skipping pion interop: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        return Ok(());
    }

    let (server, mut server_rx, server_addr) = spawn_rustrtc_dtls_server().await?;
    let child = Command::new(&bin)
        .arg(server_addr.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    wait_connected(&server, 30).await?;
    let ping = tokio::time::timeout(Duration::from_secs(5), server_rx.recv())
        .await
        .map_err(|_| anyhow::anyhow!("pion client sent no application data"))?
        .ok_or_else(|| anyhow::anyhow!("server rx closed"))?;
    assert_eq!(&ping[..], b"ping");
    server.send(Bytes::from_static(b"pong")).await?;

    let out = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    anyhow::ensure!(
        stdout.contains("HANDSHAKE_OK"),
        "pion handshake failed: {stdout} (stderr: {})",
        String::from_utf8_lossy(&out.stderr)
    );
    anyhow::ensure!(stdout.contains("ECHO_OK"), "pion echo failed: {stdout}");
    anyhow::ensure!(out.status.success(), "pion client exited with error");
    Ok(())
}

/// OpenSSL `s_client -dtls1_2` (the DTLS family used by Asterisk /
/// FreeSWITCH-style third-party PBXes) dials a rustrtc server using OpenSSL's
/// default cipher offer. Skips gracefully without a compatible openssl.
#[tokio::test]
async fn test_interop_openssl_client_rustrtc_server() -> Result<()> {
    use std::process::{Command, Stdio};

    let version = match Command::new("openssl").arg("version").output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => {
            info!("openssl not found, skipping openssl interop");
            return Ok(());
        }
    };
    if !version.starts_with("OpenSSL") {
        info!("{version}: LibreSSL/BoringSSL lack reliable s_client DTLS, skipping");
        return Ok(());
    }
    let help = Command::new("openssl")
        .args(["s_client", "-help"])
        .output()?;
    if !String::from_utf8_lossy(&help.stderr).contains("dtls1_2") {
        info!("openssl s_client has no -dtls1_2, skipping");
        return Ok(());
    }

    let (server, _rx, server_addr) = spawn_rustrtc_dtls_server().await?;
    let mut child = Command::new("openssl")
        .args([
            "s_client",
            "-dtls1_2",
            "-connect",
            &server_addr.to_string(),
            "-cipher",
            "ECDHE-ECDSA-AES128-GCM-SHA256",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let connected = wait_connected(&server, 15).await;
    drop(child.stdin.take()); // EOF → s_client sends close_notify and exits
    let _ = child.wait()?;
    connected
}
