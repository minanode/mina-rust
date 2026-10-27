//! Scan state endpoints.
//!
//! - `GET /scan-state/summary` - Get scan state summary for best tip
//! - `GET /scan-state/summary/{block}` - Get scan state summary for specific block

use crate::input_limits::{self as limits, bounded};

use std::str::FromStr;

use axum::{
    extract::{Path, State},
    Json,
};
use utoipa_axum::{router::OpenApiRouter, routes};

use mina_node::rpc::{
    RpcRequest, RpcScanStateSummary, RpcScanStateSummaryGetQuery, RpcScanStateSummaryGetResponse,
};
use mina_p2p_messages::v2::StateHash;

use crate::http_server::{AppError, AppResult, AppState, JsonErrorResponse, MAX_STATE_HASH_CHARS};

/// Block identifier for scan state queries.
///
/// Can be "latest" for best tip, a block height (u32), or a block hash.
#[derive(Debug, Clone, utoipa::ToSchema)]
#[allow(unused, reason = "schema type for block identifier query param")]
enum BlockIdentifier {
    /// Use best tip block
    Latest,
    /// Block height (e.g., 490467)
    Height(u32),
    /// Block hash (e.g., 3NLrbJrSvDVEqnMMEeWvk1TiCmcDpnUiHZqdEKVEZcqieKu1TBkS)
    Hash(StateHash),
}

impl FromStr for BlockIdentifier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        bounded(s, limits::STATE_HASH).map_err(|e| e.to_string())?;
        if s == "latest" {
            Ok(BlockIdentifier::Latest)
        } else if let Some(height) = bounded(s, limits::U32_DECIMAL)
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
        {
            Ok(BlockIdentifier::Height(height))
        } else {
            s.parse::<StateHash>()
                .map(BlockIdentifier::Hash)
                .map_err(|_| "invalid arg! Expected 'latest', block height, or block hash".into())
        }
    }
}

/// Scan state routes
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(summary))
        .routes(routes!(summary_for_block))
}

/// Scan state summary for best tip
#[utoipa::path(
    get,
    path = "/scan-state/summary",
    tag = "scan-state",
    responses(
        (status = 200, description = "Scan state summary", body = RpcScanStateSummary),
        (status = 500, description = "Target block not found", body = JsonErrorResponse,
            example = json!({"error": "target block not found"}))
    )
)]
async fn summary(State(state): State<AppState>) -> AppResult<Json<RpcScanStateSummary>> {
    // TODO: "target block not found" should arguably be 404, not 500
    let result: Option<RpcScanStateSummaryGetResponse> = state
        .rpc_sender()
        .oneshot_request(RpcRequest::ScanStateSummaryGet(
            RpcScanStateSummaryGetQuery::ForBestTip,
        ))
        .await;

    match result {
        None => Err(AppError::ChannelDropped),
        Some(Ok(data)) => Ok(Json(data)),
        Some(Err(err)) => Err(AppError::Internal(err)),
    }
}

/// Scan state summary for specific block
#[utoipa::path(
    get,
    path = "/scan-state/summary/{block}",
    tag = "scan-state",
    params(
        ("block" = BlockIdentifier, Path, description = "\"latest\" for best tip, block height, or block hash")
    ),
    responses(
        (status = 200, description = "Scan state summary", body = RpcScanStateSummary),
        (status = 400, description = "Invalid block identifier", body = JsonErrorResponse,
            example = json!({"error": "invalid arg! Expected block hash or height"})),
        (status = 500, description = "Target block not found", body = JsonErrorResponse,
            example = json!({"error": "target block not found"}))
    )
)]
async fn summary_for_block(
    State(state): State<AppState>,
    Path(block): Path<String>,
) -> AppResult<Json<RpcScanStateSummary>> {
    bounded(&block, limits::STATE_HASH).map_err(|e| AppError::BadRequest(e.to_string()))?;
    // Try parsing as height first, then as hash
    let query = if let Some(height) = bounded(&block, limits::U32_DECIMAL)
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
    {
        RpcScanStateSummaryGetQuery::ForBlockWithHeight(height)
    } else {
        // Checked before decoding: see MAX_STATE_HASH_CHARS.
        match (block.len() <= MAX_STATE_HASH_CHARS).then(|| block.parse()) {
            Some(Ok(hash)) => RpcScanStateSummaryGetQuery::ForBlockWithHash(hash),
            _ => {
                return Err(AppError::BadRequest(
                    "invalid arg! Expected block hash or height".to_string(),
                ))
            }
        }
    };

    let result: Option<RpcScanStateSummaryGetResponse> = state
        .rpc_sender()
        .oneshot_request(RpcRequest::ScanStateSummaryGet(query))
        .await;

    match result {
        None => Err(AppError::ChannelDropped),
        Some(Ok(data)) => Ok(Json(data)),
        Some(Err(err)) => Err(AppError::Internal(err)),
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use mina_node_common::rpc::RpcService;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn state_hash_length_rejects_before_rpc_in_under_one_ms() {
        let rpc = RpcService::new();
        let state = AppState::new(rpc.req_sender());
        for len in [limits::STATE_HASH + 1, 65_000] {
            let block = "z".repeat(len);
            let start = Instant::now();
            let result = summary_for_block(State(state.clone()), Path(block)).await;
            let elapsed = start.elapsed();
            assert!(
                matches!(result, Err(AppError::BadRequest(ref e)) if e.contains("maximum length"))
            );
            assert!(elapsed < Duration::from_millis(1), "{elapsed:?}");
            println!("REST {len}-byte state hash rejected in {elapsed:?}");
        }
        assert!(crate::input_limits::tests::STATE
            .parse::<BlockIdentifier>()
            .is_ok());
        assert!("latest".parse::<BlockIdentifier>().is_ok());
        assert!("4294967295".parse::<BlockIdentifier>().is_ok());
    }
}
