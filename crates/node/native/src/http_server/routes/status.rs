//! Status and health check endpoints.
//!
//! - `GET /build_env` - Build environment information
//! - `GET /status` - Node status
//! - `GET /healthz` - Liveness: the node answers, whatever its peers and sync
//! - `GET /readyz` - Readiness: recently synced, with a ready peer
//! - `POST /make_heartbeat` - Trigger a heartbeat

use axum::{extract::State, Json};
use utoipa_axum::{router::OpenApiRouter, routes};

use mina_node::{
    rpc::{
        RpcHealthCheckResponse, RpcHeartbeatGetResponse, RpcNodeStatus, RpcReadinessCheckResponse,
        RpcRequest, RpcStatusGetResponse,
    },
    BuildEnv,
};

use crate::http_server::{AppError, AppResult, AppState, JsonErrorResponse};

/// Returns status routes as an OpenApiRouter.
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(build_env))
        .routes(routes!(status))
        .routes(routes!(healthz))
        .routes(routes!(readyz))
        .routes(routes!(make_heartbeat))
}

/// Public status has its own handler and response type; never expose StatusGet.
pub(super) fn public_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(public_status))
        .routes(routes!(healthz))
        .routes(routes!(readyz))
}

/// Build environment information
#[utoipa::path(
    get,
    path = "/build_env",
    tag = "status",
    responses(
        (status = 200, body = BuildEnv)
    )
)]
async fn build_env() -> Json<BuildEnv> {
    Json(mina_node::BuildEnv::get())
}

/// Current node status
#[utoipa::path(
    get,
    path = "/status",
    tag = "status",
    responses(
        (status = 200, description = "Current node status", body = RpcNodeStatus)
    )
)]
async fn status(State(state): State<AppState>) -> AppResult<Json<RpcNodeStatus>> {
    let reply: RpcStatusGetResponse = rpc_request!(state, RpcRequest::StatusGet)?;
    reply.map(Json).ok_or(AppError::Internal(
        "StatusGet should always return Some(...)".into(),
    ))
}

/// Opt-in operator diagnostic, available only with MINA_MEMORY_DEBUG=1.
#[cfg(feature = "memory-debug")]
pub(crate) async fn memory_debug(
    State(state): State<AppState>,
) -> AppResult<Json<serde_json::Value>> {
    use axum::http::StatusCode;

    // One worker at a time, at most one new snapshot per minute. A queued
    // request must never make the state-machine thread do diagnostic work.
    let _permit = state
        .memory_debug_permit
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            AppError::Json(
                StatusCode::TOO_MANY_REQUESTS,
                serde_json::json!({"error": "memory diagnostic busy"}),
            )
        })?;
    {
        let mut last = state.memory_debug_last.lock().await;
        if last.is_some_and(|time| time.elapsed() < std::time::Duration::from_secs(60)) {
            return Err(AppError::Json(
                StatusCode::TOO_MANY_REQUESTS,
                serde_json::json!({"error": "memory diagnostic limited to once per minute"}),
            ));
        }
        *last = Some(std::time::Instant::now());
    }
    let mut reply: serde_json::Value = rpc_request!(state, RpcRequest::MemoryDebugGet)?;
    reply["jemalloc"] = jemalloc_stats();
    Ok(Json(reply))
}

#[cfg(all(feature = "memory-debug", not(target_arch = "aarch64")))]
fn jemalloc_stats() -> serde_json::Value {
    use std::{ffi::CString, mem::size_of, ptr};
    use tikv_jemalloc_sys::mallctl;

    fn read<T: Default>(name: &str) -> Option<T> {
        let name = CString::new(name).ok()?;
        let mut value = T::default();
        let mut len = size_of::<T>();
        let code = unsafe {
            mallctl(
                name.as_ptr(),
                (&mut value as *mut T).cast(),
                &mut len,
                ptr::null_mut(),
                0,
            )
        };
        (code == 0 && len == size_of::<T>()).then_some(value)
    }
    let epoch = CString::new("epoch").expect("static name");
    let mut new_epoch: u64 = 1;
    unsafe {
        mallctl(
            epoch.as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            (&mut new_epoch as *mut u64).cast(),
            size_of::<u64>(),
        );
    }
    serde_json::json!({
        "allocated": read::<usize>("stats.allocated"),
        "active": read::<usize>("stats.active"),
        "resident": read::<usize>("stats.resident"),
        "retained": read::<usize>("stats.retained"),
        "arenas": read::<u32>("arenas.narenas"),
    })
}

#[cfg(all(feature = "memory-debug", target_arch = "aarch64"))]
fn jemalloc_stats() -> serde_json::Value {
    serde_json::Value::Null
}

#[cfg(all(test, feature = "memory-debug"))]
mod memory_debug_tests {
    use super::*;
    use mina_node::core::channels::{mpsc, oneshot};
    use mina_node_common::rpc::{NodeRpcRequest, RpcSender};

    #[tokio::test]
    async fn diagnostic_uses_its_own_rpc_and_limits_repeat_requests() {
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(4);
        let state = AppState::new(RpcSender::new(tx));
        let node = tokio::spawn(async move {
            let request = rx.recv().await.expect("diagnostic request");
            assert!(matches!(request.req, RpcRequest::MemoryDebugGet));
            let sender = request
                .responder
                .downcast::<oneshot::Sender<serde_json::Value>>()
                .expect("diagnostic response channel");
            sender.send(serde_json::json!({"state": {}})).unwrap();
            assert!(rx.try_recv().is_err());
        });
        let first = memory_debug(State(state.clone())).await.unwrap();
        assert!(first.0.get("jemalloc").is_some());
        let second = memory_debug(State(state)).await.unwrap_err();
        assert!(matches!(
            second,
            AppError::Json(axum::http::StatusCode::TOO_MANY_REQUESTS, _)
        ));
        node.await.unwrap();
    }
}

/// Liveness probe
///
/// Answers 200 whenever the node's state machine handles the request, also
/// while it bootstraps, catches up or has no peers, so a liveness probe never
/// restarts a node for being unsynced. Use `/readyz` for sync and peers.
#[utoipa::path(
    get,
    path = "/healthz",
    tags = ["status", "kubernetes"],
    responses(
        (status = 200, description = "Node is alive")
    )
)]
async fn healthz(State(state): State<AppState>) -> AppResult<()> {
    let reply: RpcHealthCheckResponse = rpc_request!(state, RpcRequest::HealthCheck)?;
    reply.map_err(AppError::ServiceUnavailable)
}

/// Readiness probe
///
/// Answers 200 when the node reached `Synced` in the last 30 minutes and has at
/// least one ready peer, otherwise 503 listing every failing condition.
#[utoipa::path(
    get,
    path = "/readyz",
    tags = ["status", "kubernetes"],
    responses(
        (status = 200, description = "Node is ready to accept traffic"),
        (status = 503, description = "Node is not ready", body = JsonErrorResponse,
            examples(
                ("NotSynced" = (value = json!({"error": "not synced"}))),
                ("Desynced" = (value = json!({"error": "Synced Some(2000s) ago, which is more than the threshold 1800s"}))),
                ("NoReadyPeers" = (value = json!({"error": "peer count 0 < 1"}))),
                ("NotSyncedNoPeers" = (value = json!({"error": "not synced, peer count 0 < 1"}))),
            )),
    )
)]
async fn readyz(State(state): State<AppState>) -> AppResult<()> {
    let reply: RpcReadinessCheckResponse = rpc_request!(state, RpcRequest::ReadinessCheck)?;
    reply.map_err(AppError::ServiceUnavailable)
}

/// Trigger heartbeat
#[utoipa::path(
    post,
    path = "/make_heartbeat",
    tag = "status",
    responses(
        // inline: type alias for Option<T> would register as "Option"
        (status = 200, description = "Heartbeat triggered successfully", body = inline(RpcHeartbeatGetResponse))
    )
)]
async fn make_heartbeat(State(state): State<AppState>) -> AppResult<Json<RpcHeartbeatGetResponse>> {
    jsonify_rpc!(state, RpcRequest::HeartbeatGet)
}

// The private StatusGet computes peer/pool/resource diagnostics. Coalesce refreshes
// across every connection and cache failures too, so neither HEAD, concurrent
// requests nor an unavailable node can cause a traversal per public request.
const PUBLIC_STATUS_TTL: std::time::Duration = std::time::Duration::from_secs(5);
#[derive(Default)]
pub(crate) struct PublicStatusCache {
    entry: Option<(std::time::Instant, Option<PublicNodeStatus>)>,
}

/// Fixed-size public data only. Do not embed private RPC structs here: adding a
/// field to the operator status must never expand the public response.
#[derive(Clone, serde::Serialize, utoipa::ToSchema)]
struct PublicNodeStatus {
    chain_id: Option<String>,
    transition_frontier: PublicFrontier,
}

#[derive(Clone, serde::Serialize, utoipa::ToSchema)]
struct PublicFrontier {
    best_tip: Option<PublicTip>,
    sync: PublicSync,
}

#[derive(Clone, serde::Serialize, utoipa::ToSchema)]
struct PublicTip {
    hash: mina_p2p_messages::v2::StateHash,
    height: u32,
    timestamp: Option<redux::Timestamp>,
}

#[derive(Clone, serde::Serialize, utoipa::ToSchema)]
struct PublicSync {
    phase: PublicSyncPhase,
    time: Option<redux::Timestamp>,
}

#[derive(Clone, serde::Serialize, utoipa::ToSchema)]
enum PublicSyncPhase {
    Bootstrap,
    Catchup,
    Synced,
    Pending,
}

impl From<RpcNodeStatus> for PublicNodeStatus {
    fn from(status: RpcNodeStatus) -> Self {
        let frontier = status.transition_frontier;
        Self {
            // Chain IDs are protocol hex digests, never arbitrary operator text.
            chain_id: status
                .chain_id
                .filter(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())),
            transition_frontier: PublicFrontier {
                best_tip: frontier.best_tip.map(|tip| PublicTip {
                    hash: tip.hash,
                    height: tip.height,
                    timestamp: tip.timestamp,
                }),
                sync: PublicSync {
                    phase: match frontier.sync.phase.as_str() {
                        "Bootstrap" => PublicSyncPhase::Bootstrap,
                        "Catchup" => PublicSyncPhase::Catchup,
                        "Synced" => PublicSyncPhase::Synced,
                        _ => PublicSyncPhase::Pending,
                    },
                    time: frontier.sync.time,
                },
            },
        }
    }
}

#[utoipa::path(
    get,
    path = "/status",
    tag = "status",
    responses((status = 200, body = PublicNodeStatus), (status = 503))
)]
async fn public_status(State(state): State<AppState>) -> AppResult<Json<PublicNodeStatus>> {
    let mut cache = state.public_status.lock().await;
    if cache
        .entry
        .as_ref()
        .is_none_or(|(at, _)| at.elapsed() >= PUBLIC_STATUS_TTL)
    {
        // Record the attempt before awaiting, including cancelled requests.
        cache.entry = Some((std::time::Instant::now(), None));
        let reply = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            state
                .rpc_sender()
                .oneshot_request::<RpcStatusGetResponse>(RpcRequest::StatusGet),
        )
        .await
        .ok()
        .flatten()
        .flatten();
        cache.entry = Some((std::time::Instant::now(), reply.map(PublicNodeStatus::from)));
    }
    cache
        .entry
        .as_ref()
        .and_then(|(_, status)| status.clone())
        .map(Json)
        .ok_or_else(|| AppError::ServiceUnavailable("status unavailable".into()))
}
