//! Statistics endpoints.
//!
//! - `GET /stats/actions` - Get action statistics (optional `id` query param)
//! - `GET /stats/sync` - Get sync statistics (optional `limit` query param)
//! - `GET /stats/block_producer` - Get block producer statistics
//! - `GET /stats/memory/proofs` - Count the proofs the node holds, by holder

use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use utoipa_axum::{router::OpenApiRouter, routes};

use mina_node::rpc::{
    ActionStatsQuery, RpcActionStatsGetResponse, RpcBlockProducerStatsGetResponse, RpcRequest,
    RpcSyncStatsGetResponse, SyncStatsQuery,
};

use crate::http_server::{AppError, AppResult, AppState, JsonErrorResponse};

/// Stats routes
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(sync))
        .routes(routes!(verification))
        .routes(routes!(actions))
        .routes(routes!(block_producer))
        .routes(routes!(snark_work_coverage))
        .routes(routes!(proof_memory))
}

#[derive(Deserialize, Default)]
struct ActionQueryParams {
    /// Optional filter: "latest" for latest block, or a block ID (u64).
    id: Option<String>,
}

/// Action statistics
#[utoipa::path(
    get,
    path = "/stats/actions",
    tag = "stats",
    params(
        ("id" = Option<String>, Query, description = "\"latest\" for latest block, or numeric block ID")
    ),
    responses(
        // inline: type alias for Option<T> would register as "Option"
        (status = 200, description = "Action statistics", body = inline(RpcActionStatsGetResponse)),
        (status = 400, description = "Invalid id parameter", body = JsonErrorResponse,
            example = json!({"error": "'id' must be an u64 integer: invalid digit found in string, instead passed: foo"}))
    )
)]
async fn actions(
    State(state): State<AppState>,
    Query(params): Query<ActionQueryParams>,
) -> AppResult<Json<RpcActionStatsGetResponse>> {
    let query = match params.id.as_deref() {
        None => ActionStatsQuery::SinceStart,
        Some("latest") => ActionStatsQuery::ForLatestBlock,
        Some(id) => {
            let id = crate::input_limits::bounded(id, crate::input_limits::U64_DECIMAL)
                .map_err(|e| AppError::BadRequest(e.to_string()))?;
            let id: u64 = id.parse().map_err(|err| {
                AppError::BadRequest(format!(
                    "'id' must be an u64 integer: {err}, instead passed: {id}"
                ))
            })?;
            ActionStatsQuery::ForBlockWithId(id)
        }
    };

    jsonify_rpc!(state, RpcRequest::ActionStatsGet(query))
}

#[derive(Deserialize, Default)]
struct SyncQueryParams {
    /// Optional limit on the number of sync snapshots to return.
    #[serde(default, deserialize_with = "crate::input_limits::optional_usize")]
    limit: Option<usize>,
}

/// Sync statistics
#[utoipa::path(
    get,
    path = "/stats/sync",
    tag = "stats",
    params(
        ("limit" = Option<usize>, Query, description = "Max number of sync snapshots to return (newest first, at most 16 retained)")
    ),
    responses(
        // inline: type alias for Option<Vec<T>> would register as "Option"
        (status = 200, description = "Sync statistics", body = inline(RpcSyncStatsGetResponse))
    )
)]
async fn sync(
    State(state): State<AppState>,
    Query(SyncQueryParams { limit }): Query<SyncQueryParams>,
) -> AppResult<Json<RpcSyncStatsGetResponse>> {
    jsonify_rpc!(state, RpcRequest::SyncStatsGet(SyncStatsQuery { limit }))
}

/// Block producer statistics
#[utoipa::path(
    get,
    path = "/stats/block_producer",
    tag = "stats",
    responses(
        // inline: type alias for Option<T> would register as "Option"
        (status = 200, description = "Block producer statistics", body = inline(RpcBlockProducerStatsGetResponse))
    )
)]
async fn block_producer(
    State(state): State<AppState>,
) -> AppResult<Json<RpcBlockProducerStatsGetResponse>> {
    jsonify_rpc!(state, RpcRequest::BlockProducerStatsGet)
}

#[derive(Deserialize, Default)]
struct VerificationQueryParams {
    #[serde(default, deserialize_with = "crate::input_limits::optional_usize")]
    limit: Option<usize>,
    #[serde(default, deserialize_with = "crate::input_limits::query_usize")]
    offset: usize,
    /// Comma-separated state hashes.
    hashes: Option<String>,
}

/// The longest `hashes` parameter: as many state hashes as a page holds,
/// each with a comma. Checked before the parameter is split.
const MAX_HASHES_PARAM: usize =
    mina_node::verification::MAX_RECEIPT_PAGE_SIZE * (crate::input_limits::STATE_HASH + 1);

impl VerificationQueryParams {
    fn into_query(self) -> AppResult<mina_node::verification::VerificationQuery> {
        use crate::input_limits::{bounded, STATE_HASH};
        use mina_node::verification::{VerificationQuery, MAX_RECEIPT_PAGE_SIZE};
        let hashes = bounded(self.hashes.as_deref().unwrap_or_default(), MAX_HASHES_PARAM)
            .map_err(|e| AppError::BadRequest(format!("'hashes': {e}")))?
            .split(',')
            .map(str::trim)
            .filter(|hash| !hash.is_empty())
            .collect::<Vec<_>>();
        if hashes.len() > MAX_RECEIPT_PAGE_SIZE {
            return Err(AppError::BadRequest(format!(
                "'hashes' takes at most {MAX_RECEIPT_PAGE_SIZE} state hashes, got {}",
                hashes.len()
            )));
        }
        // Each hash is held to a state hash's Base58Check length (52)
        // before decoding, as every other state hash input is
        // (`input_limits::STATE_HASH`).
        let hashes = hashes
            .into_iter()
            .map(|hash| {
                bounded(hash, STATE_HASH).map_err(|_| {
                    AppError::BadRequest(format!(
                        "'hashes': a state hash has at most {STATE_HASH} characters, got {}",
                        hash.len()
                    ))
                })?;
                hash.parse().map_err(|err| {
                    AppError::BadRequest(format!("'hashes': invalid state hash {hash}: {err}"))
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        Ok(VerificationQuery {
            limit: self.limit,
            offset: self.offset,
            hashes,
        })
    }
}

/// Bounded local block verification receipts and counters.
///
/// Receipts come newest first by first observation (`limit`, at most 128,
/// and `offset`), or, with `hashes`, for those blocks only. An answer to
/// `hashes` carries `lookup`; a hash missing from its receipts has no
/// retained receipt.
#[utoipa::path(
    get,
    path = "/stats/verification",
    tag = "stats",
    params(
        ("limit" = Option<usize>, Query, description = "Receipts per page, at most 128 (default 64; 0 for the summary only)"),
        ("offset" = Option<usize>, Query, description = "Receipts to skip, newest first"),
        ("hashes" = Option<String>, Query, description = "Comma-separated state hashes to look up instead of a page, at most 128")
    ),
    responses(
        (status = 200, description = "Local verification receipts", body = serde_json::Value),
        (status = 400, description = "Invalid query", body = JsonErrorResponse)
    )
)]
async fn verification(
    State(state): State<AppState>,
    Query(params): Query<VerificationQueryParams>,
) -> AppResult<Json<mina_node::rpc::RpcVerificationStatsGetResponse>> {
    let query = params.into_query()?;
    jsonify_rpc!(state, RpcRequest::VerificationStatsGet(query))
}

/// Cached read-only rehearsal of the next diff, sampled on the ledger worker.
#[utoipa::path(get, path = "/stats/snark-work-coverage", tag = "stats",
    responses((status = 200, description = "Required work coverage and expected diff", body = mina_node::snark_pool::WorkCoverageStatus)))]
async fn snark_work_coverage(
    State(state): State<AppState>,
) -> AppResult<Json<mina_node::snark_pool::WorkCoverageStatus>> {
    jsonify_rpc!(state, RpcRequest::SnarkWorkCoverageGet)
}

/// PC1: the SNARK proofs the node holds, by holder, with their binprot bytes.
/// It walks every proof and waits for the ledger thread, so it is slow: a
/// diagnostic, not for polling.
#[utoipa::path(get, path = "/stats/memory/proofs", tag = "stats",
    responses((status = 200, description = "Proofs held, by holder", body = serde_json::Value)))]
async fn proof_memory(
    State(state): State<AppState>,
) -> AppResult<Json<mina_node::rpc::RpcProofMemoryGetResponse>> {
    jsonify_rpc!(state, RpcRequest::ProofMemoryGet)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(hashes: &str) -> VerificationQueryParams {
        VerificationQueryParams {
            hashes: Some(hashes.to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn hashes_are_parsed_and_bounded() {
        // A state hash the VRF evaluator's tests also parse.
        let hash = "3NKxUSAJE3wqJkrtBhMYhwzrMq3B5sKjPJQRyXz1YrPWA7761opD";
        let query = params(&format!(" {hash},,{hash} ")).into_query().unwrap();
        assert_eq!(query.hashes.len(), 2);
        assert_eq!(query.hashes[0].to_string(), hash);
        assert!(params(",").into_query().unwrap().hashes.is_empty());
        assert!(params("not-a-hash").into_query().is_err());
        let many = vec![hash; 129].join(",");
        assert!(params(&many).into_query().is_err());
    }

    #[test]
    fn long_hash_is_rejected_before_decoding() {
        // The length checks' errors, not the decoder's: nothing was decoded.
        // A long parameter is refused before it is split...
        let err = params(&"1".repeat(65_000)).into_query().unwrap_err();
        assert!(
            matches!(err, AppError::BadRequest(ref message) if message.contains("maximum length")),
            "{err:?}"
        );
        // ...and each hash is held to a state hash's 52 characters
        // (`input_limits::STATE_HASH`), as the other state hash inputs are.
        let hash = "3NKxUSAJE3wqJkrtBhMYhwzrMq3B5sKjPJQRyXz1YrPWA7761opD";
        assert_eq!(hash.len(), crate::input_limits::STATE_HASH);
        assert!(params(hash).into_query().is_ok());
        let err = params(&"1".repeat(crate::input_limits::STATE_HASH + 1))
            .into_query()
            .unwrap_err();
        assert!(
            matches!(err, AppError::BadRequest(ref message) if message.contains("at most 52 characters, got 53")),
            "{err:?}"
        );
        // A full page of hashes fits the parameter's bound.
        let page = vec![hash; mina_node::verification::MAX_RECEIPT_PAGE_SIZE].join(",");
        assert!(page.len() <= MAX_HASHES_PARAM);
        let query = params(&page).into_query().unwrap();
        assert_eq!(
            query.hashes.len(),
            mina_node::verification::MAX_RECEIPT_PAGE_SIZE
        );
    }

    #[test]
    fn limit_and_offset_are_bounded_before_parsing() {
        // The input caps' bounded integer parsing, kept with R26's `hashes`.
        let query = |q: &str| {
            let uri: axum::http::Uri = format!("http://node/stats/verification?{q}")
                .parse()
                .unwrap();
            Query::<VerificationQueryParams>::try_from_uri(&uri).map(|Query(params)| params)
        };
        let ok = query("limit=5&offset=7&hashes=")
            .unwrap()
            .into_query()
            .unwrap();
        assert_eq!((ok.limit, ok.offset), (Some(5), 7));
        assert!(ok.hashes.is_empty());
        let none = query("").unwrap().into_query().unwrap();
        assert_eq!((none.limit, none.offset), (None, 0));
        assert!(query(&format!("offset={}", "0".repeat(65_000))).is_err());
        assert!(query(&format!("limit={}", "0".repeat(65_000))).is_err());
    }
}
