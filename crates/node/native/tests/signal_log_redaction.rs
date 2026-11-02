//! A browser's signalling offer reaches the operator router as a GET path:
//! the base58 of its SDP, with the browser's ICE addresses. No log line, at
//! any level, may carry it. The request is still logged, with the offer
//! replaced and only its length kept.
#![cfg(feature = "p2p-webrtc")]

use std::{
    io,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
struct Writer(Arc<Mutex<Vec<u8>>>);

impl io::Write for Writer {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn signalling_get_logs_no_offer_or_address() {
    use mina_node_native::http_server;

    let rpc = mina_node_common::rpc::RpcService::new();
    let address = "198.51.100.77";
    let original = format!("v=0\r\na=candidate:1 1 udp 2122260223 {address} 40000 typ host\r\n");
    let encoded = bs58::encode(&original).into_string();

    let output = Arc::new(Mutex::new(Vec::new()));
    let writer = Writer(output.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    for public in [false, true] {
        output.lock().unwrap().clear();
        let app = if public {
            http_server::signaling::router(rpc.req_sender())
        } else {
            http_server::router(rpc.req_sender(), Default::default())
        };
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let url = format!(
            "http://{}/mina/webrtc/signal/{encoded}",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        // Not a valid offer: refused, but the request is logged first.
        assert_eq!(reqwest::get(url).await.unwrap().status().as_u16(), 400);
        server.abort();
        let _ = server.await;

        let log = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert!(
            !log.contains(&encoded),
            "public={public}: the encoded offer was logged: {log}"
        );
        assert!(
            !log.contains(address),
            "public={public}: an ICE address was logged: {log}"
        );
        if !public {
            let line = log
                .lines()
                .find(|l| l.contains("WebRTC signalling request"))
                .unwrap_or_else(|| panic!("the request was not logged: {log}"));
            assert!(line.contains("/mina/webrtc/signal/<offer>"), "{line}");
            assert!(
                line.contains(&format!("offer_len={}", encoded.len())),
                "{line}"
            );
        }
    }
}
