//! MINAWEB-RPC1: compact best-chain and sync reads, with the JSON the web
//! node's `RpcSender` answers, so one client parser serves both.
//!
//! - `GET /best-chain/headers` - headers and counts, newest first
//!   (optional `limit` and `max_height`)
//! - `GET /best-chain/block/{id}` - one best-chain block by height or state
//!   hash, with what it applied; `null` if the frontier hasn't got it
//! - `GET /stats/sync/progress` - a sync's progress, ledger by ledger and
//!   block by block

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use utoipa_axum::{router::OpenApiRouter, routes};

use mina_node::rpc::{
    RpcBestChainBlockGetResponse, RpcBestChainHeaders, RpcBestChainHeadersQuery, RpcBlockDetail,
    RpcRequest, RpcSyncProgress,
};
use mina_node_common::rpc::transition_frontier::parse_block_id;

use crate::http_server::{AppError, AppResult, AppState, JsonErrorResponse};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(headers))
        .routes(routes!(block))
        .routes(routes!(sync_progress))
}

#[derive(Deserialize, Default)]
struct HeadersParams {
    #[serde(default, deserialize_with = "crate::input_limits::optional_usize")]
    limit: Option<usize>,
    #[serde(default, deserialize_with = "crate::input_limits::optional_usize")]
    max_height: Option<usize>,
}

/// Best-chain headers
///
/// Each block's height, hashes, slot, time, producer, and its command,
/// zkApp and SNARK work counts and fees, newest first, without the blocks.
#[utoipa::path(
    get,
    path = "/best-chain/headers",
    tag = "chain",
    params(
        ("limit" = Option<usize>, Query, description = "Headers to return, at most 300 (default 20)"),
        ("max_height" = Option<u32>, Query, description = "Only blocks at this height or below")
    ),
    responses(
        (status = 200, description = "Best-chain headers", body = RpcBestChainHeaders),
        (status = 400, description = "Invalid query", body = JsonErrorResponse)
    )
)]
async fn headers(
    State(state): State<AppState>,
    Query(params): Query<HeadersParams>,
) -> AppResult<Json<RpcBestChainHeaders>> {
    let max_height = params
        .max_height
        .map(u32::try_from)
        .transpose()
        .map_err(|_| AppError::BadRequest("'max_height' is too large".to_owned()))?;
    jsonify_rpc!(
        state,
        RpcRequest::BestChainHeadersGet(RpcBestChainHeadersQuery {
            limit: params.limit,
            max_height,
        })
    )
}

/// One best-chain block
///
/// Its header, commands, fee transfers, coinbase and SNARK work; `null` if
/// the node's frontier hasn't got it.
#[utoipa::path(
    get,
    path = "/best-chain/block/{id}",
    tag = "chain",
    params(("id" = String, Path, description = "A height or a 3N... state hash")),
    responses(
        (status = 200, description = "The block, or null", body = Option<RpcBlockDetail>),
        (status = 400, description = "Not a height or a state hash", body = JsonErrorResponse)
    )
)]
async fn block(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<RpcBestChainBlockGetResponse>> {
    let id = crate::input_limits::bounded(&id, crate::input_limits::STATE_HASH)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let query = parse_block_id(id).map_err(AppError::BadRequest)?;
    jsonify_rpc!(state, RpcRequest::BestChainBlockGet(query))
}

/// Sync progress
///
/// The ledgers a sync fetches (staking epoch, next epoch, the root's snarked
/// and staged), the blocks it fetches and applies, one cell a block, and
/// consensus `k`.
#[utoipa::path(
    get,
    path = "/stats/sync/progress",
    tag = "stats",
    responses((status = 200, description = "Sync progress", body = RpcSyncProgress))
)]
async fn sync_progress(State(state): State<AppState>) -> AppResult<Json<RpcSyncProgress>> {
    jsonify_rpc!(state, RpcRequest::SyncProgressGet)
}
