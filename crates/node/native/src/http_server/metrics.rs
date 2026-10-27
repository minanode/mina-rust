//! Private Prometheus text endpoint. No route is installed on the API router.
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use mina_node::{
    rpc::{RpcMetricsGetResponse, RpcRequest},
    BuildEnv,
};
use mina_node_common::rpc::RpcSender;

#[derive(Clone)]
struct MetricsState {
    sender: RpcSender,
    store_root: Option<PathBuf>,
    store_size: Arc<tokio::sync::Mutex<Option<(Instant, Option<u64>)>>>,
}

pub async fn run(
    listener: std::net::TcpListener,
    sender: RpcSender,
    store_root: Option<PathBuf>,
) -> std::io::Result<()> {
    let addr = listener.local_addr()?;
    let listener = tokio::net::TcpListener::from_std(listener)?;
    let state = MetricsState {
        sender,
        store_root,
        store_size: Default::default(),
    };
    tracing::info!(%addr, "Prometheus metrics listening");
    axum::serve(listener, router(state)).await
}

fn router(state: MetricsState) -> Router {
    Router::new()
        .route("/metrics", get(scrape))
        .with_state(state)
}

async fn scrape(State(state): State<MetricsState>) -> impl IntoResponse {
    let snapshot = tokio::time::timeout(
        Duration::from_secs(2),
        state
            .sender
            .oneshot_request::<RpcMetricsGetResponse>(RpcRequest::MetricsGet),
    )
    .await
    .ok()
    .flatten();
    let Some(snapshot) = snapshot else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "metrics snapshot unavailable",
        )
            .into_response();
    };
    let rss = resident_bytes();
    let store_size = cached_store_size(&state, snapshot.chain_id.as_deref()).await;
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        render(&snapshot, rss, store_size, &BuildEnv::get()),
    )
        .into_response()
}

async fn cached_store_size(state: &MetricsState, chain_id: Option<&str>) -> Option<u64> {
    let root = state.store_root.as_ref()?;
    let chain_id =
        chain_id.filter(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))?;
    let mut cache = state.store_size.lock().await;
    if let Some((at, size)) = *cache {
        if at.elapsed() < Duration::from_secs(60) {
            return size;
        }
    }
    let path = root.join(chain_id).join("frontier");
    let size = tokio::task::spawn_blocking(move || {
        let files = std::fs::read_dir(path).ok()?;
        let mut sum = 0u64;
        for file in files {
            let file = file.ok()?;
            let metadata = file.metadata().ok()?;
            if metadata.is_file() {
                sum = sum.saturating_add(metadata.len());
            }
        }
        Some(sum)
    })
    .await
    .ok()
    .flatten();
    *cache = Some((Instant::now(), size));
    size
}

fn resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    kb.checked_mul(1024)
}

fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn render(
    snapshot: &RpcMetricsGetResponse,
    rss: Option<u64>,
    store_size: Option<u64>,
    build: &BuildEnv,
) -> String {
    let mut out = String::with_capacity(4096);
    macro_rules! metric {
        ($name:expr, $kind:expr, $help:expr, $value:expr) => {{
            out.push_str(&format!(
                "# HELP {} {}\n# TYPE {} {}\n{} {}\n",
                $name, $help, $name, $kind, $name, $value
            ));
        }};
    }
    out.push_str("# HELP mina_sync_status Current synchronization phase (one active phase).\n# TYPE mina_sync_status gauge\n");
    // The phase the node reports (F-dash3): before a peer is heard from,
    // `Connecting` or `Listening`, as OCaml's sync status.
    for phase in ["Connecting", "Listening", "Bootstrap", "Catchup", "Synced"] {
        out.push_str(&format!(
            "mina_sync_status{{status=\"{phase}\"}} {}\n",
            u8::from(snapshot.sync_phase == phase)
        ));
    }
    metric!(
        "mina_ready",
        "gauge",
        "One when the node's readiness check passes.",
        u8::from(snapshot.ready)
    );
    if let Some(height) = snapshot.best_tip_height {
        metric!(
            "mina_transition_frontier_best_tip_block_height",
            "gauge",
            "Current best tip height.",
            height
        );
    }
    if let Some(slot) = snapshot.best_tip_global_slot {
        metric!(
            "mina_best_tip_global_slot",
            "gauge",
            "Current best tip global slot.",
            slot
        );
    }
    if let Some(height) = snapshot.highest_peer_tip_height {
        metric!(
            "mina_highest_peer_tip_block_height",
            "gauge",
            "Highest connected peer tip height.",
            height
        );
    }
    out.push_str(
        "# HELP mina_peers Number of peers by connection state.\n# TYPE mina_peers gauge\n",
    );
    for (state, count) in [
        ("connecting", snapshot.peers_connecting),
        ("connected", snapshot.peers_connected),
        ("disconnecting", snapshot.peers_disconnecting),
        ("disconnected", snapshot.peers_disconnected),
    ] {
        out.push_str(&format!("mina_peers{{state=\"{state}\"}} {count}\n"));
    }
    metric!(
        "mina_blocks_applied_total",
        "counter",
        "Blocks applied by this process.",
        snapshot.verification.applied_since_start
    );
    metric!(
        "mina_block_proofs_verified_total",
        "counter",
        "Block proofs verified by this process.",
        snapshot.verification.verified_since_start
    );
    metric!(
        "mina_block_proofs_failed_total",
        "counter",
        "Failed block proof verification attempts observed by this process.",
        snapshot.verification.proof_failures_since_start
    );
    out.push_str("# HELP mina_block_proof_verification_seconds Time spent verifying successful block proofs.\n# TYPE mina_block_proof_verification_seconds histogram\n");
    for (bound, count) in ["0.05", "0.1", "0.25", "0.5", "1", "2", "5", "10"]
        .iter()
        .zip(snapshot.verification.proof_time_buckets)
    {
        out.push_str(&format!(
            "mina_block_proof_verification_seconds_bucket{{le=\"{bound}\"}} {count}\n"
        ));
    }
    out.push_str(&format!(
        "mina_block_proof_verification_seconds_bucket{{le=\"+Inf\"}} {}\n",
        snapshot.verification.proof_time_count
    ));
    out.push_str(&format!("mina_block_proof_verification_seconds_sum {}\nmina_block_proof_verification_seconds_count {}\n", snapshot.verification.proof_time_sum_seconds, snapshot.verification.proof_time_count));
    metric!(
        "mina_transaction_pool_size",
        "gauge",
        "Transactions currently in the pool.",
        snapshot.transaction_pool_size
    );
    for (name, value, help) in [
        (
            "mina_network_transaction_pool_diff_received",
            snapshot.transaction_pool_diff_received,
            "Transaction pool gossip diffs received.",
        ),
        (
            "mina_network_transaction_pool_diff_broadcasted",
            snapshot.transaction_pool_diff_broadcasted,
            "Transaction pool diffs broadcast.",
        ),
        (
            "mina_network_snark_pool_diff_received",
            snapshot.snark_pool_diff_received,
            "SNARK pool gossip diffs received.",
        ),
        (
            "mina_network_snark_pool_diff_broadcasted",
            snapshot.snark_pool_diff_broadcasted,
            "SNARK pool diffs broadcast.",
        ),
    ] {
        metric!(name, "gauge", help, value);
    }
    metric!(
        "mina_transaction_pool_transactions_added_to_pool",
        "counter",
        "Positive net transaction pool additions from applied diffs.",
        snapshot.transactions_added_to_pool
    );
    metric!(
        "mina_snark_pool_jobs",
        "gauge",
        "SNARK jobs currently in the pool.",
        snapshot.snark_pool_jobs
    );
    metric!(
        "mina_snark_work_snark_pool_size",
        "gauge",
        "Completed SNARK jobs currently in the pool.",
        snapshot.snark_pool_completed
    );
    metric!(
        "mina_snark_pool_pending_jobs",
        "gauge",
        "SNARK jobs without completed work.",
        snapshot
            .snark_pool_jobs
            .saturating_sub(snapshot.snark_pool_completed)
    );
    metric!(
        "mina_snark_work_received_total",
        "counter",
        "Completed SNARK works listed in peer RPC responses, including repeats.",
        snapshot.snark_work_received_total
    );
    metric!(
        "mina_block_producer_slots_won",
        "counter",
        "Won slots scheduled for production.",
        snapshot.block_producer_slots_won
    );
    metric!(
        "mina_block_producer_blocks_produced_total",
        "counter",
        "Own blocks committed to the frontier.",
        snapshot.block_producer_blocks_produced
    );
    metric!(
        "mina_block_producer_blocks_failed_total",
        "counter",
        "Production attempts discarded.",
        snapshot.block_producer_blocks_failed
    );
    out.push_str("# HELP mina_block_producer_block_production_delay Delay from the won slot start to producing a block, in milliseconds.\n# TYPE mina_block_producer_block_production_delay histogram\n");
    let mut cumulative = 0u64;
    for (bound, count) in [
        "60000", "90000", "120000", "150000", "180000", "210000", "+Inf",
    ]
    .iter()
    .zip(snapshot.block_production_delay_buckets)
    {
        cumulative = cumulative.saturating_add(count);
        out.push_str(&format!(
            "mina_block_producer_block_production_delay_bucket{{le=\"{bound}\"}} {cumulative}\n"
        ));
    }
    out.push_str(&format!("mina_block_producer_block_production_delay_sum {}\nmina_block_producer_block_production_delay_count {cumulative}\n", snapshot.block_production_delay_sum_ms));
    metric!(
        "mina_snark_worker_proofs_made_total",
        "counter",
        "Proof results the local worker returned, including those dropped after a stop or re-price.",
        snapshot.snark_worker_proofs_made
    );
    metric!(
        "mina_snark_worker_proofs_failed_total",
        "counter",
        "Local worker jobs reporting an error.",
        snapshot.snark_worker_proofs_failed
    );
    metric!(
        "mina_snark_worker_proofs_sold_total",
        "counter",
        "Work credited to the configured worker key in successfully applied blocks.",
        snapshot.snark_worker_proofs_sold
    );
    if let (Some(fetched), Some(estimated)) =
        (snapshot.ledger_sync_fetched, snapshot.ledger_sync_estimated)
    {
        metric!(
            "mina_ledger_sync_fetched",
            "gauge",
            "Accepted ledger accounts and hashes fetched for the active ledger.",
            fetched
        );
        metric!(
            "mina_ledger_sync_estimated",
            "gauge",
            "Estimated total ledger accounts and hashes for the active ledger.",
            estimated
        );
    }
    if let Some(bytes) = rss {
        metric!(
            "mina_process_resident_memory_bytes",
            "gauge",
            "Resident memory in bytes.",
            bytes
        );
    }
    if let Some(bytes) = store_size {
        metric!(
            "mina_store_size_bytes",
            "gauge",
            "Frontier store file bytes, cached for one minute.",
            bytes
        );
    }
    metric!(
        "mina_uptime_seconds",
        "gauge",
        "Seconds since node state initialization.",
        snapshot.uptime_secs
    );
    out.push_str(&format!("# HELP mina_build_info Build identity.\n# TYPE mina_build_info gauge\nmina_build_info{{commit=\"{}\",version=\"{}\"}} 1\n", escape_label(&build.git.commit_hash), escape_label(&build.version)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_value(text: &str, name: &str) -> Option<String> {
        text.lines().find_map(|line| {
            line.strip_prefix(name)
                .and_then(|rest| rest.strip_prefix(' '))
                .map(str::to_owned)
        })
    }

    #[test]
    fn scrape_values_reflect_snapshot_and_missing_values_are_absent() {
        let mut verification = mina_node::verification::VerificationState::default().summary();
        verification.applied_since_start = 4;
        verification.verified_since_start = 3;
        verification.proof_failures_since_start = 2;
        verification.proof_time_count = 3;
        verification.proof_time_sum_seconds = 0.7;
        verification.proof_time_buckets = [1, 1, 2, 3, 3, 3, 3, 3];
        let snapshot = RpcMetricsGetResponse {
            chain_id: None,
            uptime_secs: 77,
            sync_phase: "Catchup".into(),
            ready: false,
            best_tip_height: Some(4321),
            best_tip_global_slot: Some(9876),
            highest_peer_tip_height: Some(4324),
            peers_connecting: 2,
            peers_connected: 5,
            peers_disconnecting: 1,
            peers_disconnected: 3,
            transaction_pool_size: 8,
            transaction_pool_diff_received: 11,
            transaction_pool_diff_broadcasted: 2,
            transactions_added_to_pool: 3,
            snark_pool_jobs: 14,
            snark_pool_completed: 6,
            snark_pool_diff_received: 13,
            snark_pool_diff_broadcasted: 4,
            block_production_delay_buckets: [1, 2, 0, 0, 0, 0, 1],
            block_production_delay_sum_ms: 300_000,
            snark_work_received_total: 19,
            verification,
            block_producer_slots_won: 7,
            block_producer_blocks_produced: 4,
            block_producer_blocks_failed: 3,
            snark_worker_proofs_made: 9,
            snark_worker_proofs_failed: 2,
            snark_worker_proofs_sold: 5,
            ledger_sync_fetched: Some(33),
            ledger_sync_estimated: Some(100),
        };
        let text = render(&snapshot, Some(4096), Some(2048), &BuildEnv::get());
        for (name, expected) in [
            ("mina_transition_frontier_best_tip_block_height", "4321"),
            ("mina_best_tip_global_slot", "9876"),
            ("mina_highest_peer_tip_block_height", "4324"),
            ("mina_blocks_applied_total", "4"),
            ("mina_block_proofs_verified_total", "3"),
            ("mina_block_proofs_failed_total", "2"),
            ("mina_block_proof_verification_seconds_count", "3"),
            ("mina_transaction_pool_size", "8"),
            ("mina_network_transaction_pool_diff_received", "11"),
            ("mina_network_transaction_pool_diff_broadcasted", "2"),
            ("mina_network_snark_pool_diff_received", "13"),
            ("mina_network_snark_pool_diff_broadcasted", "4"),
            ("mina_transaction_pool_transactions_added_to_pool", "3"),
            ("mina_block_producer_block_production_delay_count", "4"),
            ("mina_block_producer_block_production_delay_sum", "300000"),
            ("mina_snark_pool_jobs", "14"),
            ("mina_snark_work_snark_pool_size", "6"),
            ("mina_snark_pool_pending_jobs", "8"),
            ("mina_snark_work_received_total", "19"),
            ("mina_block_producer_slots_won", "7"),
            ("mina_block_producer_blocks_produced_total", "4"),
            ("mina_block_producer_blocks_failed_total", "3"),
            ("mina_snark_worker_proofs_made_total", "9"),
            ("mina_snark_worker_proofs_failed_total", "2"),
            ("mina_snark_worker_proofs_sold_total", "5"),
            ("mina_ledger_sync_fetched", "33"),
            ("mina_ledger_sync_estimated", "100"),
            ("mina_process_resident_memory_bytes", "4096"),
            ("mina_store_size_bytes", "2048"),
            ("mina_uptime_seconds", "77"),
            ("mina_ready", "0"),
        ] {
            assert_eq!(
                sample_value(&text, name).as_deref(),
                Some(expected),
                "{name}"
            );
        }
        assert!(text.contains("mina_sync_status{status=\"Catchup\"} 1\n"));
        assert!(text.contains("mina_sync_status{status=\"Listening\"} 0\n"));
        assert!(!text.contains("mina_sync_status{status=\"Pending\"}"));
        assert!(text.contains("mina_peers{state=\"connected\"} 5\n"));
        assert!(text.contains("mina_block_proof_verification_seconds_bucket{le=\"0.5\"} 3\n"));
        assert!(
            text.contains("mina_block_producer_block_production_delay_bucket{le=\"90000\"} 3\n")
        );
        assert!(text.contains("mina_build_info{commit=\""));

        let mut empty = snapshot;
        empty.best_tip_height = None;
        empty.highest_peer_tip_height = None;
        empty.ledger_sync_fetched = None;
        assert!(!render(&empty, None, None, &BuildEnv::get())
            .contains("mina_transition_frontier_best_tip_block_height "));
    }

    #[tokio::test]
    async fn metrics_listener_serves_only_metrics_from_node_snapshot() {
        use mina_node::core::channels::{mpsc, oneshot};
        use mina_node_common::rpc::NodeRpcRequest;

        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
        tokio::spawn(async move {
            let request = rx.recv().await.unwrap();
            assert!(matches!(request.req, RpcRequest::MetricsGet));
            let responder = request
                .responder
                .downcast::<oneshot::Sender<RpcMetricsGetResponse>>()
                .unwrap();
            let response = RpcMetricsGetResponse {
                chain_id: None,
                uptime_secs: 12,
                sync_phase: "Synced".into(),
                ready: true,
                best_tip_height: Some(456),
                best_tip_global_slot: Some(900),
                highest_peer_tip_height: Some(457),
                peers_connecting: 0,
                peers_connected: 2,
                peers_disconnecting: 0,
                peers_disconnected: 0,
                transaction_pool_size: 3,
                transaction_pool_diff_received: 1,
                transaction_pool_diff_broadcasted: 2,
                transactions_added_to_pool: 3,
                snark_pool_jobs: 5,
                snark_pool_completed: 1,
                snark_pool_diff_received: 4,
                snark_pool_diff_broadcasted: 5,
                block_production_delay_buckets: [0; 7],
                block_production_delay_sum_ms: 0,
                snark_work_received_total: 0,
                verification: mina_node::verification::VerificationState::default().summary(),
                block_producer_slots_won: 0,
                block_producer_blocks_produced: 0,
                block_producer_blocks_failed: 0,
                snark_worker_proofs_made: 0,
                snark_worker_proofs_failed: 0,
                snark_worker_proofs_sold: 0,
                ledger_sync_fetched: None,
                ledger_sync_estimated: None,
            };
            (*responder).send(response).unwrap();
        });
        let state = MetricsState {
            sender: RpcSender::new(tx),
            store_root: None,
            store_size: Default::default(),
        };
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server =
            tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
        let client = reqwest::Client::new();
        let response = client.get(format!("{base}/metrics")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; version=0.0.4; charset=utf-8"
        );
        let body = response.text().await.unwrap();
        assert_eq!(
            sample_value(&body, "mina_transition_frontier_best_tip_block_height"),
            Some("456".into())
        );
        assert_eq!(
            sample_value(&body, "mina_transaction_pool_size"),
            Some("3".into())
        );
        assert_eq!(sample_value(&body, "mina_ready"), Some("1".into()));
        assert_eq!(
            client
                .get(format!("{base}/status"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        server.abort();
    }

    #[tokio::test]
    async fn store_size_reads_files_once_per_cache_window() {
        use mina_node::core::channels::mpsc;
        use mina_node_common::rpc::NodeRpcRequest;

        let chain = "a".repeat(64);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/test-data/store-size")
            .join(format!("store-size-test-{}", std::process::id()));
        let dir = root.join(&chain).join("frontier");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("block.bin"), [1u8, 2, 3]).unwrap();
        let (tx, _rx) = mpsc::channel::<NodeRpcRequest>(1);
        let state = MetricsState {
            sender: RpcSender::new(tx),
            store_root: Some(root.clone()),
            store_size: Default::default(),
        };
        assert_eq!(cached_store_size(&state, Some(&chain)).await, Some(3));
        std::fs::write(dir.join("ledger.bin"), [4u8, 5]).unwrap();
        assert_eq!(cached_store_size(&state, Some(&chain)).await, Some(3));
        assert_eq!(cached_store_size(&state, Some("invalid")).await, None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
