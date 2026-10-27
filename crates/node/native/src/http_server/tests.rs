//! P6b: transaction submission is off unless the node is started with
//! `--enable-tx-submission`, on both `POST /send-payment` and the GraphQL
//! mutations.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use mina_node::core::channels::mpsc;
use mina_node_common::rpc::{NodeRpcRequest, RpcSender};
use serde_json::{json, Value};

use super::{router, HttpServerOptions, TX_SUBMISSION_DISABLED};

#[tokio::test]
async fn http_listener_binds_given_address() {
    let addr = "127.0.0.1".parse().unwrap();
    let listener = super::bind_http_listener(addr, 0).await.unwrap();
    assert_eq!(listener.local_addr().unwrap().ip(), addr);
    assert_ne!(listener.local_addr().unwrap().port(), 0);
}

#[tokio::test]
async fn operator_http_serves_its_prebound_socket_without_rebinding() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/build_env", listener.local_addr().unwrap());
    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
    let rpc = mina_node_common::rpc::RpcService::new();
    let server = tokio::spawn(super::run_on_listener(
        listener,
        rpc.req_sender(),
        Default::default(),
    ));
    assert_eq!(reqwest::get(url).await.unwrap().status().as_u16(), 200);
    server.abort();
}

#[test]
fn non_loopback_worker_control_warns_that_port_must_be_private() {
    let options = HttpServerOptions {
        worker_control: true,
        ..Default::default()
    };
    let warning = super::public_worker_control_warning("0.0.0.0".parse().unwrap(), &options)
        .expect("non-loopback worker control must warn");
    assert!(warning.contains("the port must be private"));
    assert!(super::public_worker_control_warning("127.0.0.1".parse().unwrap(), &options).is_none());
    assert!(
        super::public_worker_control_warning("0.0.0.0".parse().unwrap(), &Default::default())
            .is_none()
    );
}

#[test]
fn dashboard_hint_uses_the_bound_loopback_family() {
    assert!(
        super::loopback_dashboard_hint("127.0.0.1".parse().unwrap(), 3800)
            .contains("ssh -N -L 3800:127.0.0.1:3800 you@server")
    );
    assert!(super::loopback_dashboard_hint("::1".parse().unwrap(), 3800)
        .contains("ssh -N -L 3800:[::1]:3800 you@server"));
}

/// Serves the router on a free local port, with a node that counts the
/// requests it gets and answers none of them.
async fn serve(tx_submission: bool) -> (String, Arc<AtomicUsize>) {
    let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
    let count = Arc::new(AtomicUsize::new(0));
    let counted = count.clone();
    tokio::spawn(async move {
        while let Some(request) = rx.recv().await {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(request);
        }
    });
    let app = router(
        RpcSender::new(tx),
        HttpServerOptions {
            tx_submission,
            ..Default::default()
        },
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), count)
}

async fn send_payment(base: &str) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{base}/send-payment"))
        .json(&json!([]))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

const SEND_PAYMENT: &str = r#"
mutation {
  sendPayment(
    input: {
      from: "B62qrPN5Y5yq8kGE3FbVKbGTdTAJNdtNtB5sNVpxyRwWGcDEhpMzc8g"
      to: "B62qrPN5Y5yq8kGE3FbVKbGTdTAJNdtNtB5sNVpxyRwWGcDEhpMzc8g"
      amount: "1000000000"
      fee: "10000000"
    }
    signature: { field: "1", scalar: "1" }
  ) {
    payment { id }
  }
}
"#;

const SEND_DELEGATION: &str = r#"
mutation {
  sendDelegation(
    input: {
      from: "B62qrPN5Y5yq8kGE3FbVKbGTdTAJNdtNtB5sNVpxyRwWGcDEhpMzc8g"
      to: "B62qrPN5Y5yq8kGE3FbVKbGTdTAJNdtNtB5sNVpxyRwWGcDEhpMzc8g"
      fee: "10000000"
    }
    signature: { field: "1", scalar: "1" }
  ) {
    delegation { id }
  }
}
"#;

async fn graphql(base: &str, query: &str) -> Value {
    reqwest::Client::new()
        .post(format!("{base}/graphql"))
        .json(&json!({ "query": query }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn error_messages(response: &Value) -> Vec<String> {
    response["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .map(|e| e["message"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn send_payment_is_forbidden_when_submission_is_off() {
    let (base, requests) = serve(false).await;
    let (status, body) = send_payment(&base).await;
    assert_eq!(status, 403);
    assert_eq!(body["error"], TX_SUBMISSION_DISABLED);
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "nothing reached the node"
    );
}

#[tokio::test]
async fn send_payment_reaches_the_node_when_submission_is_on() {
    let (base, requests) = serve(true).await;
    let (status, _) = send_payment(&base).await;
    assert_ne!(status, 403);
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn graphql_mutations_are_refused_when_submission_is_off() {
    let (base, requests) = serve(false).await;
    for query in [SEND_PAYMENT, SEND_DELEGATION] {
        let response = graphql(&base, query).await;
        assert_eq!(
            error_messages(&response),
            vec![TX_SUBMISSION_DISABLED.to_owned()],
            "{response}"
        );
    }
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "nothing reached the node"
    );
}

#[tokio::test]
async fn graphql_mutations_reach_the_node_when_submission_is_on() {
    let (base, requests) = serve(true).await;
    for query in [SEND_PAYMENT, SEND_DELEGATION] {
        let response = graphql(&base, query).await;
        assert!(
            !error_messages(&response)
                .iter()
                .any(|m| m == TX_SUBMISSION_DISABLED),
            "{response}"
        );
    }
    assert!(requests.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn queries_work_when_submission_is_off() {
    let (base, _) = serve(false).await;
    let response = graphql(&base, "{ version }").await;
    assert!(
        !error_messages(&response)
            .iter()
            .any(|m| m == TX_SUBMISSION_DISABLED),
        "{response}"
    );
}

#[tokio::test]
async fn input_caps_keep_http_error_statuses_and_skip_rpc() {
    use crate::input_limits::{self as caps, tests::KEY};
    let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
    let app = router(
        RpcSender::new(tx),
        HttpServerOptions {
            tx_submission: true,
            ..Default::default()
        },
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    for path in [
        format!("/stats/sync?limit={}", "0".repeat(caps::U64_DECIMAL + 1)),
        format!(
            "/stats/verification?limit={}",
            "0".repeat(caps::U64_DECIMAL + 1)
        ),
        format!(
            "/stats/verification?offset={}",
            "0".repeat(caps::U64_DECIMAL + 1)
        ),
        // R26's lookup by state hash, held to the same 52 characters.
        format!(
            "/stats/verification?hashes={}",
            "z".repeat(caps::STATE_HASH + 1)
        ),
        format!("/scan-state/summary/{}", "z".repeat(caps::STATE_HASH + 1)),
        format!("/stats/actions?id={}", "0".repeat(caps::U64_DECIMAL + 1)),
        format!("/snark-pool/job/{}", "z".repeat(caps::JOB_ID + 1)),
        format!("/snarker/job/spec?id={}", "z".repeat(caps::JOB_ID + 1)),
    ] {
        let response = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 400, "{path}");
    }
    let response = client
        .post(format!("{base}/snarker/job/commit"))
        .body("z".repeat(65_000))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    assert_eq!(response.json::<Value>().await.unwrap(), "invalid_input");
    let payment = json!({"from":KEY,"to":"z".repeat(65_000),"memo":"","fee":1,"amount":1,
        "nonce":0,"valid_until":0,"signature_field":"1","signature_scalar":"1"});
    let response = client
        .post(format!("{base}/send-payment"))
        .json(&json!([payment]))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 422);
    assert!(response.text().await.unwrap().contains("maximum length"));
    #[cfg(feature = "p2p-webrtc")]
    for pointer in [
        "/identity_pub_key",
        "/target_peer_id",
        "/chain_id",
        "/host/Domain",
        "/sdp",
    ] {
        let mut offer = caps::tests::offer_fixture();
        *offer.pointer_mut(pointer).unwrap() = "z".repeat(caps::SDP + 1).into();
        let response = client
            .post(format!("{base}/mina/webrtc/signal"))
            .json(&offer)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 422, "{pointer}");
        assert!(response.text().await.unwrap().contains("maximum length"));
    }
    assert!(rx.try_recv().is_err());
    server.abort();
    let _ = server.await;
}

/// A devnet account key (LN1's sample account), for routes that take one.
const LIGHT_TEST_KEY: &str = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";

/// Exercise the actual listener/router with transaction submission enabled.
/// Enumerate the operator OpenAPI routes so newly added routes are also checked.
#[tokio::test]
async fn public_listener_only_serves_graphql_status_and_probes() {
    let _guard = super::routes::ledger::EXPORT_TEST_LOCK.lock().await;
    const PUBLIC_READS: &[&str] = &["/status", "/healthz", "/readyz"];
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    // Valid ledger hash from the OCaml one-account ledger oracle (I7).
    let hash = "jxwqr9HpSsaM8zoMoszLFooftiQpejdKWqc3vVPzgGgpfyPZCBT";
    let job_id = format!("{hash}_{hash}-{hash}_{hash}");
    let spec = serde_json::to_value(super::openapi_spec()).unwrap();
    for public_graphql in [true, false] {
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
        let count = Arc::new(AtomicUsize::new(0));
        let counted = count.clone();
        let node = tokio::spawn(async move {
            while let Some(request) = rx.recv().await {
                counted.fetch_add(1, Ordering::SeqCst);
                // Deliberately drop responses. HTTP 500 then proves dispatch
                // reached the node, rather than mistaking a 404 for success.
                drop(request);
            }
        });
        let app = router(
            RpcSender::new(tx),
            HttpServerOptions {
                tx_submission: true,
                public_graphql,
                ledger_export: true,
                worker_control: true,
                worker_control_origins: Default::default(),
                read_origins: Default::default(),
                dashboard: false,
                work_dir: None,
            },
        );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let response = client.get(format!("{base}/metrics")).send().await.unwrap();
        assert_eq!(
            response.status().as_u16(),
            404,
            "metrics leaked onto API port"
        );
        for (path, item) in spec["paths"].as_object().unwrap() {
            for method in ["get", "post"] {
                if item.get(method).is_none() {
                    continue;
                }
                let allowed = method == "get" && PUBLIC_READS.contains(&path.as_str());
                let mut uri = path
                    .replace("{job_id}", &job_id)
                    .replace("{block}", "1")
                    .replace("{offer}", "invalid")
                    .replace("{kind}", "staged-ledger")
                    .replace("{public_key}", LIGHT_TEST_KEY);
                if path == "/snarker/job/spec" {
                    uri.push_str(&format!("?id={job_id}"));
                }
                if path.starts_with("/light/account/") {
                    uri.push_str("?ledger=staged");
                }
                let before = count.load(Ordering::SeqCst);
                let body = if path == "/state" {
                    json!({})
                } else if path == "/snarker/control" {
                    json!({ "enabled": false })
                } else {
                    json!([])
                };
                let request = client.request(
                    method.to_uppercase().parse().unwrap(),
                    format!("{base}{uri}"),
                );
                let request = if path == "/snarker/job/commit" {
                    request.body(job_id.clone())
                } else if path == "/snarker/control" {
                    request.header("x-mina-worker-control", "1").json(&body)
                } else if path.starts_with("/report") {
                    request.header("x-mina-report", "1").json(&json!({}))
                } else {
                    request.json(&body)
                };
                let response = request.send().await.unwrap();
                let status = response.status().as_u16();
                let after = count.load(Ordering::SeqCst);
                if public_graphql && !allowed {
                    assert_eq!(status, 404, "{method} {path}");
                    assert_eq!(after, before, "private route reached RPC: {method} {path}");
                } else if path == "/build_env" {
                    assert_eq!(status, 200);
                } else if path.starts_with("/report") {
                    // This node has no work directory, so no report; the
                    // guard let the operator's request through.
                    assert_eq!(status, 503, "{method} {path}");
                    assert_eq!(after, before, "{method} {path}");
                } else if path.starts_with("/mina/webrtc/signal") {
                    // These fixtures intentionally fail the route's input parser.
                    assert!([400, 422].contains(&status), "{method} {path}: {status}");
                } else {
                    assert_eq!(
                        status,
                        if public_graphql && path == "/status" {
                            503
                        } else {
                            500
                        },
                        "{method} {path}: expected dropped RPC response"
                    );
                    assert_eq!(after, before + 1, "{method} {path} did not reach RPC");
                }
            }
        }
        if public_graphql {
            let before = count.load(Ordering::SeqCst);
            for path in [
                "/build_env",
                "/transaction-pool",
                "/best-chain-user-commands",
                "/stats/sync",
                "/stats/verification",
                "/discovery/bootstrap_stats",
            ] {
                for method in [
                    reqwest::Method::GET,
                    reqwest::Method::HEAD,
                    reqwest::Method::POST,
                ] {
                    let response = client
                        .request(method.clone(), format!("{base}{path}?limit=999999"))
                        .send()
                        .await
                        .unwrap();
                    assert_eq!(response.status(), 404, "{method} {path}");
                }
            }
            assert_eq!(count.load(Ordering::SeqCst), before);
        }
        // Retained reads are GET/HEAD-only, even with submission enabled.
        for path in PUBLIC_READS {
            assert_eq!(
                client
                    .post(format!("{base}{path}"))
                    .send()
                    .await
                    .unwrap()
                    .status()
                    .as_u16(),
                405
            );
        }
        // Worker control is on here, so the pages that load scripts from
        // CDNs are not served even in a developer build that has them
        // (F-console fix round). The OpenAPI spec is always served; Swagger
        // UI only in a build with it.
        for (path, enabled) in [
            ("/graphiql", false),
            ("/api-docs/openapi.json", true),
            ("/api-docs/swagger-ui/", cfg!(feature = "swagger-ui")),
            ("/api-docs/scalar", false),
            ("/api-docs/stoplight", false),
        ] {
            let status = client
                .get(format!("{base}{path}"))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16();
            assert_eq!(
                status,
                if public_graphql || !enabled { 404 } else { 200 },
                "{path}"
            );
        }
        let response = graphql(&base, "{ __typename }").await;
        assert_eq!(response, json!({"data": {"__typename": "Query"}}));
        if public_graphql {
            let before = count.load(Ordering::SeqCst);
            for query in [SEND_PAYMENT, SEND_DELEGATION] {
                let response = graphql(&base, query).await;
                assert_eq!(
                    response["errors"][0]["extensions"]["code"],
                    "PUBLIC_GRAPHQL_NOT_ALLOWED"
                );
            }
            assert_eq!(count.load(Ordering::SeqCst), before);
        }
        server.abort();
        node.abort();
        let _ = server.await;
        let _ = node.await;
    }
}

// Synthetic values only: no live node data, secrets, or real operator addresses.
fn fixture_status() -> mina_node::rpc::RpcNodeStatus {
    use mina_node::rpc::*;
    RpcNodeStatus {
        next_block_production: Some(RpcNextBlockProduction {
            generated_from_consensus_at: RpcConsensusSlot::new(4, 4, 7140, 0, 90_000).unwrap(),
            next_won_slot: RpcConsensusSlot::new(7, 7, 7140, 0, 90_000),
        }),
        network: "fixture".into(),
        uptime_secs: 1,
        chain_id: Some("ab".repeat(32)),
        transition_frontier: RpcNodeStatusTransitionFrontier {
            root: None,
            best_tip: Some(RpcNodeStatusTransitionFrontierBlockSummary {
                hash: mina_p2p_messages::v2::StateHash::zero(),
                height: 42,
                timestamp_ms: Some(123),
                timestamp: Some(redux::Timestamp::new(123_000_000)),
                global_slot: 4,
            }),
            sync: RpcNodeStatusTransitionFrontierSync {
                time: None,
                status: "diagnostic at 10.23.0.5".into(),
                phase: "Synced".into(),
                target: None,
            },
        },
        ledger: RpcNodeStatusLedger {
            alive_masks_after_last_commit: 123,
            pending_writes: vec![],
            pending_reads: vec![],
        },
        snark_pool: Default::default(),
        transaction_pool: Default::default(),
        current_block_production_attempt: None,
        previous_block_production_attempt: None,
        peers: vec![RpcPeerInfo {
            peer_id: mina_node::p2p::PeerId::from_bytes([1; 32]),
            best_tip: None,
            best_tip_height: None,
            best_tip_global_slot: None,
            best_tip_timestamp: None,
            connection_status: PeerConnectionStatus::Connecting,
            connecting_details: Some(
                "/ip6/2001:db8::1/tcp/8302/p2p/private-peer operator diagnostic".into(),
            ),
            address: Some("10.23.0.5:8302".into()),
            incoming: false,
            is_libp2p: true,
            time: 0,
        }],
        resources_status: RpcNodeStatusResources {
            p2p_malloc_size: 0,
            memory_debug: None,
            transition_frontier: serde_json::json!({}),
            snark_pool: serde_json::json!({}),
        },
        service_queues: mina_node::service::Queues {
            events: 0,
            snark_block_verify: 0,
            ledger: 0,
            vrf_evaluator: None,
            block_prover: None,
            p2p_webrtc: 0,
            p2p_libp2p: 0,
            rpc: 0,
        },
        network_info: RpcNodeStatusNetworkInfo {
            bind_ip: "0.0.0.0".into(),
            external_ip: Some("10.23.0.4".into()),
            client_port: None,
            libp2p_port: Some(8302),
        },
        block_producer: Some(crate::input_limits::tests::KEY.parse().unwrap()),
        coinbase_receiver: None,
    }
}

#[tokio::test]
async fn public_status_is_projected_and_refreshes_once_per_cache_window() {
    use mina_node::{core::channels::oneshot, rpc::*};
    let client = reqwest::Client::new();
    for public_graphql in [true, false] {
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
        let count = Arc::new(AtomicUsize::new(0));
        let counted = count.clone();
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let failed = fail.clone();
        let node = tokio::spawn(async move {
            while let Some(request) = rx.recv().await {
                match request.req {
                    RpcRequest::StatusGet => {
                        counted.fetch_add(1, Ordering::SeqCst);
                        // Let requests overlap, exercising refresh coalescing.
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        if !failed.load(Ordering::SeqCst) {
                            let sender = request
                                .responder
                                .downcast::<oneshot::Sender<RpcStatusGetResponse>>()
                                .unwrap();
                            let _ = sender.send(Some(fixture_status()));
                        }
                    }
                    RpcRequest::HealthCheck | RpcRequest::ReadinessCheck => {
                        let sender = request
                            .responder
                            .downcast::<oneshot::Sender<Result<(), String>>>()
                            .unwrap();
                        let _ = sender.send(Ok(()));
                    }
                    _ => panic!("unexpected RPC"),
                }
            }
        });
        let app = router(
            RpcSender::new(tx),
            HttpServerOptions {
                public_graphql,
                ledger_export: true,
                tx_submission: true,
                worker_control: true,
                worker_control_origins: Default::default(),
                read_origins: Default::default(),
                dashboard: false,
                work_dir: None,
            },
        );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        for path in ["/healthz", "/readyz"] {
            assert_eq!(
                client
                    .get(format!("{base}{path}"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                200
            );
        }
        let mut requests = tokio::task::JoinSet::new();
        for i in 0..12 {
            let request = client.request(
                if i % 2 == 0 {
                    reqwest::Method::GET
                } else {
                    reqwest::Method::HEAD
                },
                format!("{base}/status?ignored={i}"),
            );
            requests.spawn(async move { request.send().await.unwrap() });
        }
        while let Some(response) = requests.join_next().await {
            assert_eq!(response.unwrap().status(), 200);
        }
        let response = client.get(format!("{base}/status")).send().await.unwrap();
        let body: Value = response.json().await.unwrap();
        if public_graphql {
            assert_eq!(
                count.load(Ordering::SeqCst),
                1,
                "GET/HEAD/query strings share the same cache"
            );
            let tip = fixture_status().transition_frontier.best_tip.unwrap();
            assert_eq!(
                body,
                json!({
                    "chain_id": "ab".repeat(32),
                    "transition_frontier": {
                        "best_tip": {"hash": tip.hash, "height": 42, "timestamp": 123_000_000},
                        "sync": {"phase": "Synced", "time": null}
                    }
                })
            );
            // Explicit field and address-pattern assertions, independent of fixture values.
            fn check_public(value: &Value) {
                match value {
                    Value::Object(fields) => {
                        for (key, value) in fields {
                            assert!(
                                [
                                    "chain_id",
                                    "transition_frontier",
                                    "best_tip",
                                    "hash",
                                    "height",
                                    "timestamp",
                                    "sync",
                                    "phase",
                                    "time"
                                ]
                                .contains(&key.as_str()),
                                "private key: {key}"
                            );
                            check_public(value);
                        }
                    }
                    Value::String(s) => {
                        for segment in s.split(|c: char| !c.is_ascii_digit() && c != '.') {
                            assert!(
                                segment.parse::<std::net::Ipv4Addr>().is_err(),
                                "IPv4 address"
                            );
                        }
                        for segment in s.split(|c: char| !c.is_ascii_hexdigit() && c != ':') {
                            assert!(
                                segment.parse::<std::net::Ipv6Addr>().is_err(),
                                "IPv6 address"
                            );
                        }
                        for pattern in [
                            "/ip4/",
                            "/ip6/",
                            "/dns",
                            "/tcp/",
                            "/udp/",
                            "/p2p/",
                            "B62",
                            "diagnostic",
                        ] {
                            assert!(!s.contains(pattern), "private value: {pattern}");
                        }
                    }
                    Value::Array(_) => panic!("public status has no unbounded collections"),
                    _ => {}
                }
            }
            check_public(&body);
            // Expiration refreshes, then caches a failure rather than hammering RPC.
            fail.store(true, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(5100)).await;
            for _ in 0..3 {
                let response = client.get(format!("{base}/status")).send().await.unwrap();
                assert_eq!(response.status(), 503);
                assert_eq!(
                    response.json::<Value>().await.unwrap(),
                    json!({"error": "status unavailable"})
                );
            }
            assert_eq!(count.load(Ordering::SeqCst), 2);
        } else {
            assert_eq!(
                count.load(Ordering::SeqCst),
                13,
                "private status remains uncached"
            );
            assert_eq!(
                body,
                serde_json::to_value(fixture_status()).unwrap(),
                "private payload unchanged"
            );
            let build: Value = client
                .get(format!("{base}/build_env"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(
                build,
                serde_json::to_value(mina_node::BuildEnv::get()).unwrap()
            );
        }
        server.abort();
        node.abort();
        let _ = server.await;
        let _ = node.await;
    }
}

/// The default operator gate covers every ledger kind before dispatching RPC.
#[tokio::test]
async fn ledger_export_requires_explicit_enablement() {
    let _guard = super::routes::ledger::EXPORT_TEST_LOCK.lock().await;
    use mina_node::rpc::{LedgerExportKind, RpcRequest};

    for enabled in [false, true] {
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
        let app = router(
            RpcSender::new(tx),
            HttpServerOptions {
                ledger_export: enabled,
                ..Default::default()
            },
        );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        for kind in LedgerExportKind::ALL {
            let request = client.get(format!("{base}/ledger/export/{kind}")).send();
            if enabled {
                let (response, ()) = tokio::join!(request, async {
                    let request = rx.recv().await.unwrap();
                    assert!(matches!(request.req, RpcRequest::LedgerExport(k) if k == kind));
                    drop(request);
                });
                assert_eq!(response.unwrap().status().as_u16(), 500);
            } else {
                let response = request.await.unwrap();
                assert_eq!(response.status().as_u16(), 403);
                let body: Value = response.json().await.unwrap();
                assert!(body["error"]
                    .as_str()
                    .unwrap()
                    .contains("--enable-ledger-export"));
                assert!(
                    rx.try_recv().is_err(),
                    "disabled export must not dispatch RPC"
                );
            }
        }
        server.abort();
        let _ = server.await;
    }
}

#[tokio::test]
async fn public_graphql_never_dispatches_ledger_exports_even_when_enabled() {
    use mina_node::rpc::LedgerExportKind;
    let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
    let app = router(
        RpcSender::new(tx),
        HttpServerOptions {
            tx_submission: true,
            ledger_export: true,
            public_graphql: true,
            worker_control: true,
            worker_control_origins: Default::default(),
            read_origins: Default::default(),
            dashboard: false,
            work_dir: None,
        },
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    for kind in LedgerExportKind::ALL {
        let response = client
            .get(format!("{base}/ledger/export/{kind}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 404, "{kind}");
        assert!(rx.try_recv().is_err(), "public {kind} must dispatch no RPC");
    }
    server.abort();
    let _ = server.await;
}

// P5.4: the SNARK worker control.

/// Our funded devnet account's public key.
const WORKER_KEY: &str = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX";

async fn serve_options(
    options: HttpServerOptions,
) -> (
    String,
    mpsc::Receiver<NodeRpcRequest>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, rx) = mpsc::channel::<NodeRpcRequest>(16);
    let app = router(RpcSender::new(tx), options);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, rx, server)
}

async fn post_control(base: &str, body: Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{base}/snarker/control"))
        .header("x-mina-worker-control", "1")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

/// Changing the worker changes where fees go: refused without
/// `--allow-worker-control`, and the public API has no worker routes at
/// all, whatever else is allowed.
#[tokio::test]
async fn worker_control_needs_the_flag_and_no_public_api() {
    let start = json!({ "enabled": true, "public_key": WORKER_KEY, "fee": "0.004" });
    for (worker_control, public_graphql, expected) in
        [(false, false, 403), (true, true, 404), (false, true, 404)]
    {
        let (base, mut rx, server) = serve_options(HttpServerOptions {
            tx_submission: true,
            ledger_export: true,
            public_graphql,
            worker_control,
            worker_control_origins: Default::default(),
            read_origins: Default::default(),
            dashboard: false,
            work_dir: None,
        })
        .await;
        for body in [start.clone(), json!({ "enabled": false })] {
            let (status, body) = post_control(&base, body).await;
            assert_eq!(status, expected, "{worker_control} {public_graphql}");
            if expected == 403 {
                assert!(body["error"]
                    .as_str()
                    .unwrap()
                    .contains("--allow-worker-control"));
            }
        }
        if public_graphql {
            for path in ["/snarker/control", "/snarker/included"] {
                let response = reqwest::get(format!("{base}{path}")).await.unwrap();
                assert_eq!(response.status().as_u16(), 404, "{path}");
            }
        }
        assert!(rx.try_recv().is_err(), "nothing reached the node");
        server.abort();
        let _ = server.await;
    }
}

/// Keys and fees are read as `--run-snarker` and `--snarker-fee` read them,
/// before anything reaches the node; a refused value is not echoed.
#[tokio::test]
async fn worker_control_rejects_bad_keys_and_fees() {
    let (base, mut rx, server) = serve_options(HttpServerOptions {
        worker_control: true,
        ..Default::default()
    })
    .await;
    let secret = format!("EK{}", "123456789ABCDEFGHJKLMNPQRSTUVWXYZ".repeat(2));
    let mut corrupted = WORKER_KEY.to_owned();
    corrupted.replace_range(10..11, "x");
    for (body, error) in [
        (json!({ "enabled": true }), "public key"),
        (json!({ "enabled": true, "fee": "0.1" }), "public key"),
        (
            json!({ "enabled": true, "public_key": corrupted, "fee": "0.1" }),
            "not a valid public key",
        ),
        (
            json!({ "enabled": true, "public_key": secret, "fee": "0.1" }),
            "looks like a private key",
        ),
        (
            json!({ "enabled": true, "public_key": WORKER_KEY, "fee": "-1" }),
            "invalid fee",
        ),
        (
            json!({ "enabled": true, "public_key": WORKER_KEY, "fee": "1e9" }),
            "invalid fee",
        ),
        (
            json!({ "enabled": true, "public_key": WORKER_KEY, "fee": "18446744073.709551616" }),
            "64 bits",
        ),
    ] {
        let (status, response) = post_control(&base, body.clone()).await;
        assert_eq!(status, 400, "{body}");
        let message = response["error"].as_str().unwrap();
        assert!(message.contains(error), "{body}: {message}");
        assert!(!message.contains("EK12345"), "a refused key is not echoed");
    }
    // Unknown fields and a missing switch are malformed requests.
    for body in [
        json!({ "enabled": true, "key": WORKER_KEY }),
        json!({ "public_key": WORKER_KEY }),
    ] {
        let (status, _) = post_control(&base, body.clone()).await;
        assert!((400..500).contains(&status), "{body}: {status}");
    }
    assert!(rx.try_recv().is_err(), "nothing reached the node");
    server.abort();
    let _ = server.await;
}

/// OCaml's fee rule: work from a key without an account must ask at least
/// 1 MINA. A lower fee is refused with the reason; 1 MINA passes and the
/// node gets the change.
#[tokio::test]
async fn worker_control_names_the_fee_rule_for_a_key_without_an_account() {
    use mina_node::{
        core::channels::oneshot,
        external_snark_worker::RpcSnarkWorkerControl,
        rpc::{RpcRequest, RpcSnarkWorkerControlSetResponse, RpcSnarkWorkerStatus},
        SnarkerSetting,
    };

    fn respond<T: 'static + Send>(responder: Box<dyn Send + std::any::Any>, response: T) {
        let sender = responder
            .downcast::<oneshot::Sender<T>>()
            .expect("another response type");
        let _ = sender.send(response);
    }
    let control = |setting: Option<&SnarkerSetting>| RpcSnarkWorkerControl {
        allowed: true,
        enabled: setting.is_some(),
        public_key: setting.map(|s| s.public_key.clone()),
        fee: setting.map(|s| s.fee),
        account_creation_fee: 1_000_000_000,
        worker: RpcSnarkWorkerStatus::None,
        job: None,
        recent: Vec::new(),
        paused_for_block: false,
        best_tip_height: Some(100),
        zkapp_jobs: 0,
        setting_warning: None,
        proofs_made: 7,
        strategy: mina_node::SnarkerStrategy::Sequential,
    };

    let (base, mut rx, server) = serve_options(HttpServerOptions {
        worker_control: true,
        ..Default::default()
    })
    .await;
    let changes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = changes.clone();
    let node = tokio::spawn(async move {
        while let Some(NodeRpcRequest { req, responder }) = rx.recv().await {
            match req {
                RpcRequest::SnarkWorkerControlGet => respond(responder, control(None)),
                // The key has no account at the best tip.
                RpcRequest::LedgerAccountsGet(_) => {
                    respond(responder, Vec::<ledger::Account>::new())
                }
                RpcRequest::SnarkWorkerControlSet(setting) => {
                    let response: RpcSnarkWorkerControlSetResponse = Ok(control(setting.as_ref()));
                    seen.lock().unwrap().push(setting);
                    respond(responder, response)
                }
                other => panic!("unexpected request {other:?}"),
            }
        }
    });

    let (status, body) = post_control(
        &base,
        json!({ "enabled": true, "public_key": WORKER_KEY, "fee": "0.004" }),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        body["error"],
        "this account doesn't exist yet: fee must be at least 1 MINA, or fund it first"
    );
    assert!(changes.lock().unwrap().is_empty());

    let (status, body) = post_control(
        &base,
        json!({ "enabled": true, "public_key": WORKER_KEY, "fee": "1" }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["fee"], 1_000_000_000u64);
    assert_eq!(body["account_exists"], false);
    assert_eq!(body["fee_problem"], Value::Null);
    // F-dash2: the dashboard's proofs made, next to the flattened fields.
    assert_eq!(body["proofs_made"], 7);
    assert_eq!(body["strategy"], "Sequential");
    let change = changes.lock().unwrap().pop().unwrap().unwrap();
    assert_eq!(change.public_key.to_string(), WORKER_KEY);
    assert_eq!(change.fee, 1_000_000_000);

    // Stopping needs no key or fee.
    let (status, body) = post_control(&base, json!({ "enabled": false })).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["enabled"], false);
    assert_eq!(changes.lock().unwrap().pop(), Some(None));

    server.abort();
    node.abort();
}

/// P5.4's review: a page of another site open in the operator's browser must
/// not change the worker. Its POST is refused before anything reaches the
/// node, and the browser's preflight gets no CORS grant; the dashboard's
/// request, from this machine or a listed origin, goes through.
#[tokio::test]
async fn worker_control_refuses_other_sites() {
    use mina_node::{
        core::channels::oneshot,
        external_snark_worker::RpcSnarkWorkerControl,
        rpc::{RpcRequest, RpcSnarkWorkerControlSetResponse, RpcSnarkWorkerStatus},
    };

    let (base, mut rx, server) = serve_options(HttpServerOptions {
        worker_control: true,
        worker_control_origins: super::control_origin::ControlOrigins::new([
            "https://dash.example.org".to_owned(),
        ])
        .unwrap(),
        ..Default::default()
    })
    .await;
    let changes = Arc::new(AtomicUsize::new(0));
    let counted = changes.clone();
    let node = tokio::spawn(async move {
        while let Some(NodeRpcRequest { req, responder }) = rx.recv().await {
            let RpcRequest::SnarkWorkerControlSet(None) = req else {
                panic!("unexpected request {req:?}");
            };
            counted.fetch_add(1, Ordering::SeqCst);
            let response: RpcSnarkWorkerControlSetResponse = Ok(RpcSnarkWorkerControl {
                allowed: true,
                enabled: false,
                public_key: None,
                fee: None,
                account_creation_fee: 1_000_000_000,
                worker: RpcSnarkWorkerStatus::None,
                job: None,
                recent: Vec::new(),
                paused_for_block: false,
                best_tip_height: Some(100),
                zkapp_jobs: 0,
                setting_warning: None,
                proofs_made: 0,
                strategy: Default::default(),
            });
            let _ = responder
                .downcast::<oneshot::Sender<RpcSnarkWorkerControlSetResponse>>()
                .unwrap()
                .send(response);
        }
    });
    let client = reqwest::Client::new();
    let stop = r#"{"enabled": false}"#;
    let post = |origin: Option<&str>, header: bool, content_type: &str| {
        let mut request = client
            .post(format!("{base}/snarker/control"))
            .header("content-type", content_type.to_owned())
            .body(stop);
        if let Some(origin) = origin {
            request = request.header("origin", origin.to_owned());
        }
        if header {
            request = request.header("x-mina-worker-control", "1");
        }
        request.send()
    };

    // Another site's page: refused, whatever it sends.
    for (header, content_type) in [
        (true, "application/json"),
        (false, "application/json"),
        (false, "text/plain"),
    ] {
        let response = post(Some("http://evil.example"), header, content_type)
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 403);
        let body: Value = response.json().await.unwrap();
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("--worker-control-origin"));
    }
    // A request a form or a simple fetch could send: no header, or no JSON.
    assert_eq!(
        post(None, false, "application/json")
            .await
            .unwrap()
            .status()
            .as_u16(),
        403
    );
    assert_eq!(
        post(None, true, "text/plain")
            .await
            .unwrap()
            .status()
            .as_u16(),
        415
    );
    assert_eq!(
        changes.load(Ordering::SeqCst),
        0,
        "nothing reached the node"
    );

    // The browser asks first: no grant for another site, one for ours.
    let preflight = |origin: &str, path: &str| {
        client
            .request(reqwest::Method::OPTIONS, format!("{base}{path}"))
            .header("origin", origin.to_owned())
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "content-type,x-mina-worker-control",
            )
            .send()
    };
    let response = preflight("http://evil.example", "/snarker/control")
        .await
        .unwrap();
    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
    for origin in ["http://127.0.0.1:4260", "https://dash.example.org"] {
        let response = preflight(origin, "/snarker/control").await.unwrap();
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            origin,
            "{origin}"
        );
        let allowed = response.headers()["access-control-allow-headers"]
            .to_str()
            .unwrap()
            .to_ascii_lowercase();
        assert!(allowed.contains("x-mina-worker-control"), "{allowed}");
    }
    // Other routes are open to the pages that may change the worker too.
    let response = preflight("https://dash.example.org", "/status")
        .await
        .unwrap();
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://dash.example.org"
    );

    // The dashboard's request, from this machine or a listed origin, and a
    // program's (no origin), are the operator's.
    for origin in [
        Some("http://127.0.0.1:4260"),
        Some("https://dash.example.org"),
        None,
    ] {
        let response = post(origin, true, "application/json").await.unwrap();
        assert_eq!(response.status().as_u16(), 200, "{origin:?}");
    }
    assert_eq!(changes.load(Ordering::SeqCst), 3);

    server.abort();
    node.abort();
}

/// A dashboard on a listed origin reads the worker's state of a node whose
/// control is off (the default) and shows it as off, not as a node that
/// didn't answer; changing the worker stays limited to the origins that may.
#[tokio::test]
async fn worker_state_is_readable_from_listed_pages() {
    let (base, rx, server) = serve_options(HttpServerOptions {
        read_origins: super::control_origin::ReadOrigins::new(
            ["http://192.0.2.44:4250".to_owned()],
        )
        .unwrap(),
        ..Default::default()
    })
    .await;
    // No node behind it: the answer is an error, but the grant is what counts.
    drop(rx);
    let client = reqwest::Client::new();
    let response = client
        .get(format!("{base}/snarker/control"))
        .header("origin", "http://192.0.2.44:4250")
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "http://192.0.2.44:4250"
    );
    let preflight = client
        .request(reqwest::Method::OPTIONS, format!("{base}/snarker/control"))
        .header("origin", "http://192.0.2.44:4250")
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "content-type,x-mina-worker-control",
        )
        .send()
        .await
        .unwrap();
    assert!(preflight
        .headers()
        .get("access-control-allow-origin")
        .is_none());
    server.abort();
}

// F-console: the dashboard at `/`, behind every API route.

/// Every route the HTTP router serves besides the dashboard, with a method
/// it answers: the documented ones from the OpenAPI spec, and the others
/// `router` adds.
#[cfg(feature = "dashboard")]
fn api_routes() -> Vec<(reqwest::Method, String)> {
    let spec = serde_json::to_value(super::openapi_spec()).unwrap();
    let mut routes = Vec::new();
    for (path, item) in spec["paths"].as_object().unwrap() {
        // A path parameter gets a value of the kind it takes.
        let path = path
            .replace("{offer}", "3vQB7B6MrGQZaxCuFg4oh")
            .replace("{kind}", "staged")
            .replace("{block}", "1")
            .replace("{job_id}", "x")
            .replace("{public_key}", LIGHT_TEST_KEY);
        for method in item.as_object().unwrap().keys() {
            if ["get", "post", "put", "delete", "patch"].contains(&method.as_str()) {
                routes.push((method.to_uppercase().parse().unwrap(), path.clone()));
            }
        }
    }
    for (method, path) in [
        ("POST", "/graphql"),
        ("GET", "/graphql"),
        ("GET", "/archive/status"),
        ("POST", "/archive/resend"),
    ] {
        routes.push((method.parse().unwrap(), path.to_owned()));
    }
    #[cfg(feature = "graphiql")]
    routes.push((reqwest::Method::GET, "/graphiql".to_owned()));
    // The spec is served with or without Swagger UI.
    routes.push((reqwest::Method::GET, "/api-docs/openapi.json".to_owned()));
    #[cfg(feature = "swagger-ui")]
    routes.push((reqwest::Method::GET, "/api-docs/swagger-ui/".to_owned()));
    #[cfg(feature = "scalar")]
    routes.push((reqwest::Method::GET, "/api-docs/scalar".to_owned()));
    #[cfg(feature = "stoplight-elements")]
    routes.push((reqwest::Method::GET, "/api-docs/stoplight".to_owned()));
    routes
}

/// A response the dashboard gave: its pages carry its Content-Security-Policy.
#[cfg(feature = "dashboard")]
fn from_dashboard(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get("content-security-policy")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("frame-ancestors 'none'"))
}

#[cfg(feature = "dashboard")]
async fn serve_dashboard(
    options: HttpServerOptions,
) -> (
    String,
    tokio::task::JoinHandle<()>,
    tokio::task::JoinHandle<()>,
) {
    let (base, mut rx, server) = serve_options(options).await;
    // A node that never answers: handlers that ask it fail, which is still
    // their own answer.
    let node = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    (base, server, node)
}

/// Every API route still reaches its handler with the dashboard on: none is
/// answered by it, and none is axum's empty 404.
#[cfg(feature = "dashboard")]
#[tokio::test]
async fn the_dashboard_never_shadows_an_api_route() {
    let (base, server, node) = serve_dashboard(HttpServerOptions {
        dashboard: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let routes = api_routes();
    for expected in [
        "/status",
        "/readyz",
        "/healthz",
        "/build_env",
        "/state",
        "/state/peers",
        "/stats/sync",
        "/snarker/control",
        "/snarker/included",
        "/transaction-pool",
        "/accounts",
        "/graphql",
    ] {
        assert!(
            routes.iter().any(|(_, path)| path == expected),
            "{expected} missing from {routes:?}"
        );
    }
    #[cfg(feature = "p2p-webrtc")]
    assert!(routes
        .iter()
        .any(|(_, path)| path.starts_with("/mina/webrtc/signal")));
    for (method, path) in routes {
        let mut request = client.request(method.clone(), format!("{base}{path}"));
        if method == reqwest::Method::POST {
            request = request
                .header("content-type", "application/json")
                .body("{}");
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        assert!(
            !from_dashboard(&response),
            "{method} {path}: the dashboard answered"
        );
        let body = response.bytes().await.unwrap();
        assert!(
            !(status == 404 && body.is_empty()),
            "{method} {path}: no handler"
        );
    }
    server.abort();
    node.abort();
}

/// The dashboard's own pages get `index.html` (or the placeholder of a
/// binary built without it); other paths are still 404, and `/metrics`
/// stays on its own listener.
#[cfg(feature = "dashboard")]
#[tokio::test]
async fn the_dashboard_answers_its_pages_only() {
    let (base, server, node) = serve_dashboard(HttpServerOptions {
        dashboard: true,
        ..Default::default()
    })
    .await;
    for pattern in super::dashboard::route_patterns() {
        let path = pattern.replace(":id", "578883");
        let response = reqwest::get(format!("{base}{path}")).await.unwrap();
        assert_eq!(response.status().as_u16(), 200, "{path}");
        assert!(from_dashboard(&response), "{path}");
        let html = response.text().await.unwrap();
        assert!(html.starts_with("<!doctype html>"), "{path}");
    }
    for path in [
        "/nope",
        "/blocks/1/2",
        "/metrics",
        "/index.html",
        "/assets/nope.js",
    ] {
        let response = reqwest::get(format!("{base}{path}")).await.unwrap();
        assert_eq!(response.status().as_u16(), 404, "{path}");
        assert!(!from_dashboard(&response), "{path}");
    }
    // Without Swagger UI its paths are not the dashboard's either; the spec
    // is still served by its own route.
    #[cfg(not(feature = "swagger-ui"))]
    for path in ["/api-docs/swagger-ui/", "/api-docs/swagger-ui/index.html"] {
        let response = reqwest::get(format!("{base}{path}")).await.unwrap();
        assert_eq!(response.status().as_u16(), 404, "{path}");
        assert!(!from_dashboard(&response), "{path}");
    }
    let response = reqwest::get(format!("{base}/api-docs/openapi.json"))
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert!(!from_dashboard(&response));
    let response = reqwest::Client::new()
        .post(format!("{base}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
    server.abort();
    node.abort();
}

/// `--no-dashboard`, and a public node, serve nothing at `/`, as before.
#[cfg(feature = "dashboard")]
#[tokio::test]
async fn no_dashboard_serves_nothing_at_root() {
    for (dashboard, public_graphql) in [(false, false), (false, true), (true, true)] {
        let (base, server, node) = serve_dashboard(HttpServerOptions {
            dashboard,
            public_graphql,
            ..Default::default()
        })
        .await;
        for path in ["/", "/snark-worker", "/peers"] {
            let response = reqwest::get(format!("{base}{path}")).await.unwrap();
            assert_eq!(
                response.status().as_u16(),
                404,
                "{dashboard} {public_graphql} {path}"
            );
            assert!(!from_dashboard(&response));
        }
        server.abort();
        node.abort();
    }
}

/// A node that answers every worker change with a stopped worker, and counts
/// the changes it got.
fn worker_stopping_node(
    mut rx: mpsc::Receiver<NodeRpcRequest>,
) -> (tokio::task::JoinHandle<()>, Arc<AtomicUsize>) {
    use mina_node::{
        core::channels::oneshot,
        external_snark_worker::RpcSnarkWorkerControl,
        rpc::{RpcRequest, RpcSnarkWorkerControlSetResponse, RpcSnarkWorkerStatus},
    };
    let changes = Arc::new(AtomicUsize::new(0));
    let counted = changes.clone();
    let node = tokio::spawn(async move {
        while let Some(NodeRpcRequest { req, responder }) = rx.recv().await {
            let RpcRequest::SnarkWorkerControlSet(None) = req else {
                panic!("unexpected request {req:?}");
            };
            counted.fetch_add(1, Ordering::SeqCst);
            let response: RpcSnarkWorkerControlSetResponse = Ok(RpcSnarkWorkerControl {
                allowed: true,
                enabled: false,
                public_key: None,
                fee: None,
                account_creation_fee: 1_000_000_000,
                worker: RpcSnarkWorkerStatus::None,
                job: None,
                recent: Vec::new(),
                paused_for_block: false,
                best_tip_height: Some(100),
                zkapp_jobs: 0,
                setting_warning: None,
                proofs_made: 0,
                strategy: Default::default(),
            });
            let _ = responder
                .downcast::<oneshot::Sender<RpcSnarkWorkerControlSetResponse>>()
                .unwrap()
                .send(response);
        }
    });
    (node, changes)
}

/// `POST /snarker/control` to stop the worker, as a page at `origin` sends
/// it to `Host: host`: the status and the error, if any.
async fn stop_from_page(base: &str, host: &str, origin: &str) -> (u16, String) {
    let response = reqwest::Client::new()
        .post(format!("{base}/snarker/control"))
        .header("host", host.to_owned())
        .header("origin", origin.to_owned())
        .header("content-type", "application/json")
        .header("x-mina-worker-control", "1")
        .body(r#"{"enabled": false}"#)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    (
        status,
        body["error"].as_str().unwrap_or_default().to_owned(),
    )
}

/// F-console: the node's own dashboard, opened at an IP address, changes the
/// worker; the same page under a name (DNS rebinding) or on another port does
/// not. `--allow-worker-control` is still needed, and so is the dashboard.
#[tokio::test]
async fn worker_control_from_the_nodes_own_page() {
    let serving = cfg!(feature = "dashboard");
    for worker_control in [true, false] {
        let (base, rx, server) = serve_options(HttpServerOptions {
            worker_control,
            dashboard: true,
            ..Default::default()
        })
        .await;
        let (node, changes) = worker_stopping_node(rx);
        let mut allowed = 0;
        for (host, origin, same) in [
            ("192.0.2.44:3700", "http://192.0.2.44:3700", true),
            (
                "[fd7a:115c:a1e0::1]:3700",
                "http://[fd7a:115c:a1e0::1]:3700",
                true,
            ),
            // DNS rebinding: an attacker's name resolving to this node.
            ("evil.example:3700", "http://evil.example:3700", false),
            // Another port of the same address is another site.
            ("192.0.2.44:3700", "http://192.0.2.44:4282", false),
            // A name the operator didn't list, even a real one.
            (
                "node.tailnet.ts.net:3700",
                "http://node.tailnet.ts.net:3700",
                false,
            ),
            ("localhost.:3700", "http://localhost.:3700", false),
            ("192.0.2.44:3700", "null", false),
        ] {
            let (status, error) = stop_from_page(&base, host, origin).await;
            if !worker_control {
                assert_eq!(status, 403, "{host} {origin}");
                assert!(error.contains("--allow-worker-control"), "{error}");
            } else if same && serving {
                assert_eq!(status, 200, "{host} {origin}: {error}");
                allowed += 1;
            } else {
                assert_eq!(status, 403, "{host} {origin}");
                assert!(error.contains("--worker-control-origin"), "{error}");
            }
        }
        assert_eq!(changes.load(Ordering::SeqCst), allowed);
        server.abort();
        node.abort();
    }
}

/// F-console fix round: a node's own origin is trusted only while it serves
/// the dashboard. Every combination of `--allow-worker-control`,
/// `--no-dashboard` and `--public-graphql`: a page at the node's IP address
/// changes the worker only with control on and the dashboard served; a page
/// of this machine (the loopback rule) and a listed origin keep their old
/// rights.
#[tokio::test]
async fn the_nodes_own_origin_is_trusted_only_with_the_dashboard() {
    for worker_control in [true, false] {
        for dashboard in [true, false] {
            for public_graphql in [true, false] {
                let case = format!(
                    "worker_control {worker_control}, dashboard {dashboard}, public {public_graphql}"
                );
                let (base, rx, server) = serve_options(HttpServerOptions {
                    worker_control,
                    dashboard,
                    public_graphql,
                    worker_control_origins: super::control_origin::ControlOrigins::new([
                        "https://dash.example.org".to_owned(),
                    ])
                    .unwrap(),
                    ..Default::default()
                })
                .await;
                let (node, changes) = worker_stopping_node(rx);
                let control = worker_control && !public_graphql;
                let own = control && dashboard && cfg!(feature = "dashboard");
                let mut expected_changes = 0;
                for (host, origin, allowed) in [
                    ("192.0.2.44:3700", "http://192.0.2.44:3700", own),
                    ("localhost:3700", "http://localhost:3700", control),
                    ("127.0.0.1:3700", "http://127.0.0.1:4282", control),
                    ("192.0.2.44:3700", "https://dash.example.org", control),
                ] {
                    let (status, error) = stop_from_page(&base, host, origin).await;
                    if public_graphql {
                        // The public API has no worker routes at all.
                        assert_eq!(status, 404, "{case}: {origin}");
                    } else if !worker_control {
                        assert_eq!(status, 403, "{case}: {origin}");
                        assert!(error.contains("--allow-worker-control"), "{case}");
                    } else if allowed {
                        assert_eq!(status, 200, "{case}: {origin}: {error}");
                        expected_changes += 1;
                    } else {
                        assert_eq!(status, 403, "{case}: {origin}");
                        assert!(error.contains("--worker-control-origin"), "{case}");
                    }
                }
                assert_eq!(changes.load(Ordering::SeqCst), expected_changes, "{case}");
                server.abort();
                node.abort();
            }
        }
    }
}

/// The parts of an HTML page that load a script from another origin: a
/// `<script src>`, an import map entry, an `import` of a URL, or a module
/// preload. Lower-cased snippets, for the assertion message.
fn remote_scripts(html: &str) -> Vec<String> {
    let html = html.to_ascii_lowercase();
    let remote = |value: &str| {
        let value = value.trim_start_matches(['"', '\'', ' ']);
        value.starts_with("http://") || value.starts_with("https://") || value.starts_with("//")
    };
    let mut found = Vec::new();
    let mut rest = html.as_str();
    while let Some(start) = rest.find("<script") {
        let after = &rest[start..];
        let tag_end = after.find('>').unwrap_or(after.len());
        let tag = &after[..tag_end];
        let body_end = after.find("</script").unwrap_or(after.len());
        let body = &after[tag_end.min(body_end)..body_end];
        if let Some(src) = tag.split("src=").nth(1) {
            if remote(src) {
                found.push(tag.to_owned());
            }
        }
        if tag.contains("importmap")
            && (body.contains("\"http") || body.contains("'http") || body.contains("\"//"))
        {
            found.push(format!("import map in {tag}"));
        }
        for import in ["from", "import", "import("] {
            for piece in body.split(import).skip(1) {
                if remote(piece.trim_start_matches(['(', ' '])) {
                    found.push(format!("{import} {}", &piece[..piece.len().min(60)]));
                }
            }
        }
        rest = &after[body_end.max(1)..];
    }
    for link in html.split("<link").skip(1) {
        let tag = &link[..link.find('>').unwrap_or(link.len())];
        let preload = tag.contains("modulepreload") || tag.contains("as=\"script\"");
        if preload && tag.split("href=").nth(1).is_some_and(remote) {
            found.push(format!("<link{tag}"));
        }
    }
    found
}

#[test]
fn the_scanner_finds_remote_scripts() {
    for page in [
        r#"<script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference"></script>"#,
        r#"<SCRIPT SRC='//unpkg.com/x.js'></SCRIPT>"#,
        r#"<script type="importmap">{"imports":{"react":"https://esm.sh/react@19"}}</script>"#,
        r#"<script type="module">import x from "https://esm.sh/x";</script>"#,
        r#"<script type="module">await import('https://esm.sh/x')</script>"#,
        r#"<link rel="modulepreload" href="https://esm.sh/x.js">"#,
    ] {
        assert!(!remote_scripts(page).is_empty(), "{page}");
    }
    for page in [
        r#"<script src="/theme-init.js"></script><script type="module" src="/assets/index-a.js"></script>"#,
        r#"<link rel="stylesheet" href="/assets/index.css"><a href="https://o1-labs.github.io/">x</a>"#,
        r#"<script>const docs = "see https://example.org";</script>"#,
    ] {
        assert!(remote_scripts(page).is_empty(), "{page}");
    }
    #[cfg(feature = "dashboard")]
    assert!(remote_scripts(super::dashboard::PLACEHOLDER).is_empty());
}

/// The pages that load scripts from CDNs when built in (developer features).
const DEVELOPER_UIS: [&str; 3] = ["/graphiql", "/api-docs/scalar", "/api-docs/stoplight"];

/// Every page the HTTP router may serve by GET: the documented routes, the
/// dashboard's pages, Swagger UI and the developer UIs.
fn get_pages() -> Vec<String> {
    let spec = serde_json::to_value(super::openapi_spec()).unwrap();
    let mut pages: Vec<String> = spec["paths"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, item)| item.get("get").is_some())
        .map(|(path, _)| {
            path.replace("{offer}", "3vQB7B6MrGQZaxCuFg4oh")
                .replace("{kind}", "staged")
                .replace("{block}", "1")
                .replace("{job_id}", "x")
        })
        .collect();
    pages.extend(
        [
            "/",
            "/snark-worker",
            "/blocks/1",
            "/graphql-endpoint",
            "/graphql",
            "/archive/status",
            "/api-docs/openapi.json",
            "/api-docs/swagger-ui/",
            "/api-docs/swagger-ui/index.html",
        ]
        .map(str::to_owned),
    );
    pages.extend(DEVELOPER_UIS.map(str::to_owned));
    pages
}

/// Fetches every page of [`get_pages`] from `base`: the path, the status,
/// whether it is HTML, and its body.
async fn fetch_pages(base: &str) -> Vec<(String, u16, bool, String)> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let mut pages = Vec::new();
    for page in get_pages() {
        let response = client.get(format!("{base}{page}")).send().await.unwrap();
        let status = response.status().as_u16();
        let html = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"));
        pages.push((page, status, html, response.text().await.unwrap()));
    }
    pages
}

/// F-console fix round: a default build serves no HTML page that loads a
/// script from another origin, whatever the flags, as such a script runs on
/// the node's own origin and could change the worker. GraphiQL, Scalar and
/// Stoplight are 404; Swagger UI, the OpenAPI JSON and the dashboard stay.
#[cfg(not(any(
    feature = "graphiql",
    feature = "scalar",
    feature = "stoplight-elements"
)))]
#[tokio::test]
async fn no_page_of_a_default_build_loads_remote_scripts() {
    assert!(super::CDN_PAGES.is_empty());
    for worker_control in [true, false] {
        for dashboard in [true, false] {
            let (base, mut rx, server) = serve_options(HttpServerOptions {
                worker_control,
                dashboard,
                ..Default::default()
            })
            .await;
            let node = tokio::spawn(async move { while rx.recv().await.is_some() {} });
            let mut html_pages = 0;
            for (page, status, html, body) in fetch_pages(&base).await {
                if DEVELOPER_UIS.contains(&page.as_str()) {
                    assert_eq!(status, 404, "{page}");
                }
                if html {
                    html_pages += 1;
                    let found = remote_scripts(&body);
                    assert!(found.is_empty(), "{page} loads remote scripts: {found:?}");
                }
            }
            // The spec is data, served in every build; Swagger UI only in a
            // developer build with it.
            let response = reqwest::get(format!("{base}/api-docs/openapi.json"))
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), 200);
            assert_eq!(response.headers()["content-type"], "application/json");
            let spec: Value = response.json().await.unwrap();
            assert_eq!(spec, serde_json::to_value(super::openapi_spec()).unwrap());
            for page in ["/api-docs/swagger-ui/", "/api-docs/swagger-ui/index.html"] {
                let status = reqwest::get(format!("{base}{page}"))
                    .await
                    .unwrap()
                    .status()
                    .as_u16();
                let expected = if cfg!(feature = "swagger-ui") {
                    200
                } else {
                    404
                };
                assert_eq!(status, expected, "{page}");
            }
            // Swagger UI's page, and the dashboard's when it is on.
            let expected = usize::from(cfg!(feature = "swagger-ui")) * 2
                + usize::from(dashboard && cfg!(feature = "dashboard")) * 4;
            assert!(html_pages >= expected, "{html_pages} < {expected}");
            server.abort();
            node.abort();
        }
    }
}

/// A developer build with the CDN pages still doesn't serve them while worker
/// control is on; without it they are served, and the scanner sees their
/// remote scripts (its positive control on the real pages).
#[cfg(any(
    feature = "graphiql",
    feature = "scalar",
    feature = "stoplight-elements"
))]
#[tokio::test]
async fn developer_uis_are_off_while_worker_control_is_on() {
    assert!(!super::CDN_PAGES.is_empty());
    for worker_control in [true, false] {
        let (base, mut rx, server) = serve_options(HttpServerOptions {
            worker_control,
            dashboard: true,
            ..Default::default()
        })
        .await;
        let node = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        for (page, status, html, body) in fetch_pages(&base).await {
            let built_in = super::CDN_PAGES.contains(&page.as_str());
            if worker_control {
                if DEVELOPER_UIS.contains(&page.as_str()) {
                    assert_eq!(status, 404, "{page}");
                }
                if html {
                    let found = remote_scripts(&body);
                    assert!(found.is_empty(), "{page} loads remote scripts: {found:?}");
                }
            } else if built_in {
                assert_eq!(status, 200, "{page}");
                assert!(html, "{page}");
                assert!(
                    !remote_scripts(&body).is_empty(),
                    "{page}: scanner missed it"
                );
            }
        }
        server.abort();
        node.abort();
    }
}

// O3 in the dashboard: the bug report.

/// A build with pages that load CDN scripts serves the report only while
/// those pages are off, as with worker control: the report tests turn it on
/// there (the default build has no such pages).
const REPORT_NEEDS_CONTROL: bool = !super::CDN_PAGES.is_empty();

/// A node that answers `/status` with the fixture and has no SNARK worker,
/// counting the status requests; its HTTP server reads `work_dir`.
async fn serve_report(
    options: HttpServerOptions,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    serve_report_after(options, std::time::Duration::ZERO).await
}

/// [`serve_report`], with a node that takes `delay` to answer `/status`.
async fn serve_report_after(
    options: HttpServerOptions,
    delay: std::time::Duration,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    use mina_node::{
        core::channels::oneshot,
        rpc::{RpcRequest, RpcSnarkerConfigGetResponse, RpcStatusGetResponse},
    };
    let (base, mut rx, server) = serve_options(options).await;
    let count = Arc::new(AtomicUsize::new(0));
    let counted = count.clone();
    tokio::spawn(async move {
        while let Some(NodeRpcRequest { req, responder }) = rx.recv().await {
            match req {
                RpcRequest::StatusGet => {
                    counted.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(delay).await;
                    let _ = responder
                        .downcast::<oneshot::Sender<RpcStatusGetResponse>>()
                        .unwrap()
                        .send(Some(fixture_status()));
                }
                RpcRequest::SnarkerConfig => {
                    let _ = responder
                        .downcast::<oneshot::Sender<RpcSnarkerConfigGetResponse>>()
                        .unwrap()
                        .send(None);
                }
                other => panic!("unexpected request {other:?}"),
            }
        }
    });
    (base, count, server)
}

/// A work directory with a log that holds an error and an IP address.
fn report_work_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("mina.log.1"),
        "2026-10-23T20:00:00Z ERROR mina_node: kind=\"BlockApplyFailed\" peer 34.136.105.213\n",
    )
    .unwrap();
    dir
}

async fn post_report(
    base: &str,
    path: &str,
    origin: Option<&str>,
    header: bool,
    body: Option<&str>,
) -> reqwest::Response {
    let mut request = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .header("content-type", "application/json")
        .body(body.unwrap_or("{}").to_owned());
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    if header {
        request = request.header("x-mina-report", "1");
    }
    request.send().await.unwrap()
}

/// The report holds the node's logs: only the operator's requests get it,
/// and no other site's page can read it.
#[tokio::test]
async fn the_report_is_guarded_like_worker_control() {
    let dir = report_work_dir();
    let (base, _, server) = serve_report(HttpServerOptions {
        worker_control_origins: super::control_origin::ControlOrigins::new([
            "https://dash.example.org".to_owned(),
        ])
        .unwrap(),
        work_dir: Some(dir.path().to_owned()),
        worker_control: REPORT_NEEDS_CONTROL,
        ..Default::default()
    })
    .await;
    for path in ["/report", "/report/crash"] {
        // No header: a form or a simple fetch.
        let response = post_report(&base, path, None, false, None).await;
        assert_eq!(response.status().as_u16(), 403, "{path}");
        let body: Value = response.json().await.unwrap();
        assert!(
            body["error"].as_str().unwrap().contains("x-mina-report"),
            "{body}"
        );
        // Another site's page, header or not.
        for header in [true, false] {
            let response =
                post_report(&base, path, Some("http://evil.example"), header, None).await;
            assert_eq!(response.status().as_u16(), 403, "{path}");
            assert!(response
                .headers()
                .get("access-control-allow-origin")
                .is_none());
        }
        // Not JSON.
        let response = reqwest::Client::new()
            .post(format!("{base}{path}"))
            .header("x-mina-report", "1")
            .header("content-type", "text/plain")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 415, "{path}");
        // A program, a page of this machine, and a listed origin.
        for origin in [
            None,
            Some("http://localhost:4282"),
            Some("https://dash.example.org"),
        ] {
            let response = post_report(&base, path, origin, true, None).await;
            assert_eq!(response.status().as_u16(), 200, "{path} from {origin:?}");
        }
    }

    // CORS: the preflight and the answer name only allowed origins.
    let client = reqwest::Client::new();
    for (origin, allowed) in [
        ("http://evil.example", false),
        ("https://dash.example.org", true),
        ("http://127.0.0.1:4282", true),
    ] {
        let preflight = client
            .request(reqwest::Method::OPTIONS, format!("{base}/report"))
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "content-type,x-mina-report",
            )
            .send()
            .await
            .unwrap();
        assert_eq!(
            preflight
                .headers()
                .get("access-control-allow-origin")
                .is_some(),
            allowed,
            "preflight from {origin}"
        );
    }
    // Unlike `GET /status`, which the pages of this machine may read.
    let open = client
        .get(format!("{base}/status"))
        .header("origin", "http://127.0.0.1:4282")
        .send()
        .await
        .unwrap();
    assert!(open.headers().get("access-control-allow-origin").is_some());
    server.abort();
}

#[tokio::test]
async fn the_report_is_never_on_the_public_api() {
    let dir = report_work_dir();
    let (base, _, server) = serve_report(HttpServerOptions {
        public_graphql: true,
        worker_control: true,
        work_dir: Some(dir.path().to_owned()),
        ..Default::default()
    })
    .await;
    for path in ["/report", "/report/crash"] {
        let response = post_report(&base, path, None, true, None).await;
        assert_eq!(response.status().as_u16(), 404, "{path}");
    }
    server.abort();
}

/// The report is `mina report`'s, redacted the same way, with its issue
/// link; asked again at once, the same report comes back.
#[tokio::test]
async fn the_report_is_redacted_and_kept_for_a_moment() {
    let dir = report_work_dir();
    let (base, status_requests, server) = serve_report(HttpServerOptions {
        work_dir: Some(dir.path().to_owned()),
        worker_control: REPORT_NEEDS_CONTROL,
        ..Default::default()
    })
    .await;
    let report = |body: &'static str| {
        let base = base.clone();
        async move {
            let response = post_report(&base, "/report", None, true, Some(body)).await;
            assert_eq!(response.status().as_u16(), 200);
            response.json::<Value>().await.unwrap()
        }
    };
    let first = report("{}").await;
    let markdown = first["markdown"].as_str().unwrap();
    assert!(first["title"]
        .as_str()
        .unwrap()
        .starts_with("Error: BlockApplyFailed [fp:"));
    assert!(markdown.contains("BlockApplyFailed"));
    assert!(!markdown.contains("34.136.105.213"), "network ids stay out");
    assert!(markdown.contains("Not checked: the dashboard's report doesn't check it"));
    assert_eq!(first["include_network_ids"], json!(false));
    let issue = &first["issue"];
    assert!(issue["url"]
        .as_str()
        .unwrap()
        .starts_with("https://github.com/minanode/mina-rust/issues/new?title="));
    assert_eq!(issue["complete"], json!(true));
    assert!(first["file_name"].as_str().unwrap().starts_with("report-"));

    // Again at once: the same report, and the node asked once.
    let again = report("{}").await;
    assert_eq!(again["generated_at"], first["generated_at"]);
    assert_eq!(again["markdown"], first["markdown"]);
    assert_eq!(status_requests.load(Ordering::SeqCst), 1);

    // With network ids: a new report, which keeps them.
    let with_ids = report(r#"{"include_network_ids": true}"#).await;
    assert!(with_ids["markdown"]
        .as_str()
        .unwrap()
        .contains("34.136.105.213"));
    assert_eq!(status_requests.load(Ordering::SeqCst), 2);

    // A description is the reporter's words, redacted too.
    let described = report(r#"{"description": "it stopped after 10.0.0.9 went away"}"#).await;
    let markdown = described["markdown"].as_str().unwrap();
    assert!(markdown.contains("it stopped after"));
    assert!(!markdown.contains("10.0.0.9"));

    // Unknown fields are refused.
    let response = post_report(&base, "/report", None, true, Some(r#"{"repo": "x/y"}"#)).await;
    assert_eq!(response.status().as_u16(), 400);
    server.abort();
}

#[tokio::test]
async fn the_crash_notice_says_when_and_what() {
    use mina_node_report::crash::{CrashRecord, CRASH_DIR};
    let dir = report_work_dir();
    let (base, _, server) = serve_report(HttpServerOptions {
        work_dir: Some(dir.path().to_owned()),
        worker_control: REPORT_NEEDS_CONTROL,
        ..Default::default()
    })
    .await;
    let notice = || async {
        post_report(&base, "/report/crash", None, true, None)
            .await
            .json::<Value>()
            .await
            .unwrap()
    };
    let crashes = dir.path().join(CRASH_DIR);
    std::fs::create_dir_all(&crashes).unwrap();
    let now = mina_node_report::crash::now_ms();
    let record = CrashRecord {
        time: String::new(),
        unix_ms: now,
        pid: 1,
        version: String::new(),
        commit: String::new(),
        thread: "main".into(),
        message: "frontier root missing at 34.136.105.213\nmore".into(),
        location: None,
        function: None,
        backtrace: "secret backtrace".into(),
        args: vec![],
    };
    std::fs::write(
        crashes.join(format!("crash-{now}-1-0.json")),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let crash = notice().await;
    assert_eq!(crash["crash"]["crashed_ms"], json!(now));
    assert_eq!(
        crash["crash"]["message"],
        json!("frontier root missing at <ip>")
    );
    assert_eq!(crash["crash"]["this_run"], json!(true));
    assert!(!crash.to_string().contains("backtrace"));
    // Kept for a moment: the files aren't read again at once.
    std::fs::remove_dir_all(&crashes).unwrap();
    assert_eq!(notice().await, crash);
    server.abort();
}

/// No crash file: no notice.
#[tokio::test]
async fn no_crash_no_notice() {
    let dir = report_work_dir();
    let (base, _, server) = serve_report(HttpServerOptions {
        work_dir: Some(dir.path().to_owned()),
        worker_control: REPORT_NEEDS_CONTROL,
        ..Default::default()
    })
    .await;
    let notice = post_report(&base, "/report/crash", None, true, None)
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(notice, json!({ "crash": null }));
    server.abort();
}

#[tokio::test]
async fn a_node_without_a_work_dir_has_no_report() {
    let (base, _, server) = serve_report(HttpServerOptions {
        worker_control: REPORT_NEEDS_CONTROL,
        ..Default::default()
    })
    .await;
    for path in ["/report", "/report/crash"] {
        let response = post_report(&base, path, None, true, None).await;
        assert_eq!(response.status().as_u16(), 503, "{path}");
    }
    server.abort();
}

/// Ten pages asking at once get one collection, and the same report.
#[tokio::test]
async fn concurrent_reports_share_one_collection() {
    let dir = report_work_dir();
    let (base, status_requests, server) = serve_report_after(
        HttpServerOptions {
            work_dir: Some(dir.path().to_owned()),
            worker_control: REPORT_NEEDS_CONTROL,
            ..Default::default()
        },
        std::time::Duration::from_millis(200),
    )
    .await;
    let requests = (0..10)
        .map(|_| {
            let base = base.clone();
            tokio::spawn(async move {
                let response = post_report(&base, "/report", None, true, None).await;
                (
                    response.status().as_u16(),
                    response.json::<Value>().await.unwrap(),
                )
            })
        })
        .collect::<Vec<_>>();
    let mut stamps = Vec::new();
    for request in requests {
        let (status, body) = request.await.unwrap();
        assert_eq!(status, 200, "{body}");
        stamps.push(body["generated_at"].clone());
    }
    assert!(stamps.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(status_requests.load(Ordering::SeqCst), 1);
    // The crash notice too: one read for ten.
    let crashes = (0..10)
        .map(|_| {
            let base = base.clone();
            tokio::spawn(async move {
                post_report(&base, "/report/crash", None, true, None)
                    .await
                    .status()
                    .as_u16()
            })
        })
        .collect::<Vec<_>>();
    for request in crashes {
        assert_eq!(request.await.unwrap(), 200);
    }
    server.abort();
}

/// While one report is collected, a request for another is refused at once,
/// not queued behind it.
#[tokio::test]
async fn a_report_for_other_options_meanwhile_is_refused() {
    let dir = report_work_dir();
    let (base, status_requests, server) = serve_report_after(
        HttpServerOptions {
            work_dir: Some(dir.path().to_owned()),
            worker_control: REPORT_NEEDS_CONTROL,
            ..Default::default()
        },
        std::time::Duration::from_millis(500),
    )
    .await;
    let first = {
        let base = base.clone();
        tokio::spawn(async move {
            post_report(&base, "/report", None, true, None)
                .await
                .status()
                .as_u16()
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let started = std::time::Instant::now();
    let other = post_report(
        &base,
        "/report",
        None,
        true,
        Some(r#"{"include_network_ids": true}"#),
    )
    .await;
    assert_eq!(other.status().as_u16(), 503);
    assert!(
        started.elapsed() < std::time::Duration::from_millis(300),
        "not queued"
    );
    let body: Value = other.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("busy"), "{body}");
    assert_eq!(first.await.unwrap(), 200);
    assert_eq!(status_requests.load(Ordering::SeqCst), 1);
    server.abort();
}

/// A build with a page that loads CDN scripts: while it serves one, the
/// report is off, as worker control turns those pages off.
#[cfg(any(
    feature = "graphiql",
    feature = "scalar",
    feature = "stoplight-elements"
))]
#[tokio::test]
async fn the_report_is_off_while_cdn_pages_are_served() {
    let dir = report_work_dir();
    let page = super::CDN_PAGES[0];
    for worker_control in [false, true] {
        let (base, _, server) = serve_report(HttpServerOptions {
            worker_control,
            dashboard: true,
            work_dir: Some(dir.path().to_owned()),
            ..Default::default()
        })
        .await;
        let served = reqwest::get(format!("{base}{page}"))
            .await
            .unwrap()
            .status()
            .as_u16();
        for path in ["/report", "/report/crash"] {
            for origin in [None, Some("http://localhost:3000")] {
                let response = post_report(&base, path, origin, true, None).await;
                let status = response.status().as_u16();
                if worker_control {
                    assert_eq!(served, 404, "{page} with worker control");
                    assert_eq!(status, 200, "{path} from {origin:?}");
                } else {
                    assert_eq!(served, 200, "{page}");
                    assert_eq!(status, 403, "{path} from {origin:?}");
                    let body: Value = response.json().await.unwrap();
                    assert!(body["error"].as_str().unwrap().contains("CDNs"), "{body}");
                }
            }
        }
        server.abort();
    }
}

/// The operator API answers browser pages of this machine and listed
/// origins, not other sites', and only for a `Host` that is an address or a
/// listed name; programs, the public API and WebRTC signalling are unchanged.
#[tokio::test]
async fn the_operator_api_answers_only_trusted_pages() {
    let get = |base: String,
               path: &'static str,
               host: Option<&'static str>,
               origin: Option<&'static str>| async move {
        let mut request = reqwest::Client::new().get(format!("{base}{path}"));
        if let Some(host) = host {
            request = request.header("host", host);
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let response = request.send().await.unwrap();
        let cors = response
            .headers()
            .get("access-control-allow-origin")
            .map(|value| value.to_str().unwrap().to_owned());
        (response.status().as_u16(), cors)
    };
    let (base, rx, server) = serve_options(HttpServerOptions {
        read_origins: super::control_origin::ReadOrigins::new([
            "https://dash.example.org".to_owned(),
            "http://node.tailnet.ts.net:3700".to_owned(),
        ])
        .unwrap(),
        ..Default::default()
    })
    .await;
    drop(rx);
    // No CORS grant for another site's page; one for this machine's and a
    // listed origin's.
    let (_, cors) = get(base.clone(), "/status", None, Some("http://evil.example")).await;
    assert_eq!(cors, None);
    for origin in ["http://localhost:4200", "https://dash.example.org"] {
        let (_, cors) = get(base.clone(), "/status", None, Some(origin)).await;
        assert_eq!(cors.as_deref(), Some(origin));
    }
    // A browser's request for a name nobody listed is refused; for an
    // address, `localhost` or a listed name it is not.
    let (status, _) = get(
        base.clone(),
        "/status",
        Some("evil.example:3700"),
        Some("http://evil.example:3700"),
    )
    .await;
    assert_eq!(status, 403);
    let refusal = reqwest::Client::new()
        .get(format!("{base}/status"))
        .header("host", "evil.example:3700")
        .header("origin", "http://evil.example:3700")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        refusal.contains("--http-cors-origin http://evil.example:3700"),
        "{refusal}"
    );
    for host in [
        "127.0.0.1:3700",
        "localhost:3700",
        "node.tailnet.ts.net:3700",
    ] {
        let (status, _) = get(
            base.clone(),
            "/status",
            Some(host),
            Some("http://localhost:4200"),
        )
        .await;
        assert_ne!(status, 403, "{host}");
    }
    // A program sends no Origin: any name.
    let (status, _) = get(base.clone(), "/status", Some("mina-node:3000"), None).await;
    assert_ne!(status, 403);
    // Signalling stays open to every site.
    let (status, cors) = get(
        base.clone(),
        "/mina/webrtc/signal/invalid",
        Some("relay.example.org"),
        Some("https://minanode.org"),
    )
    .await;
    assert_ne!(status, 403);
    assert_eq!(cors.as_deref(), Some("https://minanode.org"));
    server.abort();

    // `*` answers every page, as before 0.20.0, and so does the public API.
    for options in [
        HttpServerOptions {
            read_origins: super::control_origin::ReadOrigins::new(["*".to_owned()]).unwrap(),
            ..Default::default()
        },
        HttpServerOptions {
            public_graphql: true,
            ..Default::default()
        },
    ] {
        let (base, rx, server) = serve_options(options).await;
        drop(rx);
        let (status, cors) = get(
            base.clone(),
            "/healthz",
            Some("evil.example:3700"),
            Some("http://evil.example"),
        )
        .await;
        assert_ne!(status, 403);
        assert_eq!(cors.as_deref(), Some("http://evil.example"));
        server.abort();
    }
}

/// The refusal names the flag and the value that let the page in.
#[test]
fn a_refused_host_is_told_the_flag_to_add() {
    let message = super::host_refusal("Node.example:3000", Some("https://node.example:3000"));
    assert!(
        message.contains("--http-cors-origin https://node.example:3000"),
        "{message}"
    );
    // A browser that sent no matching Origin: the page's address over HTTP.
    for origin in [None, Some("http://other.example")] {
        let message = super::host_refusal("node.example:3000", origin);
        assert!(
            message.contains("--http-cors-origin http://node.example:3000"),
            "{message}"
        );
    }
}

/// The node's own dashboard, opened at a LAN or VPN address, needs no flag:
/// none of its pages, API reads or GraphQL queries is refused. The requests
/// carry what a browser sends for a same-origin page.
#[tokio::test]
async fn the_dashboard_at_a_lan_address_needs_no_flag() {
    let (base, mut rx, server) = serve_options(HttpServerOptions {
        dashboard: true,
        ..Default::default()
    })
    .await;
    // A node that never answers: handlers that ask it fail, which is still
    // their own answer.
    let node = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let client = reqwest::Client::new();
    for (host, origin) in [
        ("192.168.1.20:3000", "http://192.168.1.20:3000"),
        ("198.51.100.7:3000", "http://198.51.100.7:3000"),
        ("[2001:db8::1]:3000", "http://[2001:db8::1]:3000"),
    ] {
        let routes: Value =
            serde_json::from_str(include_str!("../../../../../ui/dashboard/src/routes.json"))
                .unwrap();
        let pages = routes["routes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|route| route.as_str().unwrap().replace(":id", "1"));
        let reads = [
            "/status",
            "/build_env",
            "/stats/sync",
            "/stats/block_producer",
            "/state/peers",
            "/snarker/control",
            "/snarker/included",
            "/transaction-pool",
        ]
        .into_iter()
        .map(str::to_owned);
        for path in pages.chain(reads) {
            let response = client
                .get(format!("{base}{path}"))
                .header("host", host)
                .header("origin", origin)
                .header("sec-fetch-site", "same-origin")
                .send()
                .await
                .unwrap();
            let status = response.status().as_u16();
            let body = response.text().await.unwrap();
            assert!(
                status != 403 && !body.contains("--http-cors-origin"),
                "{host} {path}: {status} {body}"
            );
        }
        let response = client
            .post(format!("{base}/graphql"))
            .header("host", host)
            .header("origin", origin)
            .header("sec-fetch-site", "same-origin")
            .json(&json!({ "query": "{ version }" }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200, "{host} /graphql");
        let body: Value = response.json().await.unwrap();
        assert!(
            body["data"]["version"].is_string(),
            "{host} /graphql: {body}"
        );
    }
    server.abort();
    node.abort();
}
