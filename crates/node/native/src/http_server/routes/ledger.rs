//! Ledger export, for `mina ledger export` and `mina fork export`.
//!
//! - `GET /ledger/export/{kind}` - a whole ledger at the best tip, in OCaml's
//!   runtime-config accounts JSON
//! - `GET /hardfork/config` - the next hard fork's runtime config and ledgers

use std::{convert::Infallible, sync::Arc};

use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{header, HeaderName, HeaderValue},
    response::{IntoResponse, Response},
};
use futures::stream;
use tokio::sync::{Semaphore, SemaphorePermit};
use utoipa_axum::{router::OpenApiRouter, routes};

use mina_node::{
    daemon_json::{fork_config_parts, HardForkManifest},
    rpc::{
        HardForkBlockSpec, LedgerExportKind, RpcHardForkExport, RpcHardForkExportResponse,
        RpcLedgerExport, RpcLedgerExportResponse, RpcRequest,
    },
};
use serde::Deserialize;

use crate::http_server::{AppError, AppResult, AppState, JsonErrorResponse};

/// One export at a time: a whole ledger is the heaviest read the node serves.
static EXPORTS: Semaphore = Semaphore::const_new(1);

#[cfg(test)]
pub(crate) static EXPORT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub const HEADER_LEDGER_KIND: &str = "x-mina-ledger-kind";
pub const HEADER_LEDGER_HASH: &str = "x-mina-ledger-hash";
pub const HEADER_NUM_ACCOUNTS: &str = "x-mina-num-accounts";
pub const HEADER_STATE_HASH: &str = "x-mina-state-hash";
pub const HEADER_BLOCKCHAIN_LENGTH: &str = "x-mina-blockchain-length";
pub const HEADER_GLOBAL_SLOT: &str = "x-mina-global-slot-since-genesis";
pub const HEADER_EPOCH: &str = "x-mina-epoch";

/// Ledger routes
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(export))
        .routes(routes!(hard_fork_config))
}

/// A whole ledger at the best tip
///
/// The body is JSON- and hash-compatible with OCaml's `mina ledger export <kind>`:
/// a JSON array of accounts in ledger index order, in the runtime-config layout
/// (balances in MINA, as decimal strings). `mina ledger hash --ledger-file`
/// recomputes its Merkle root. The headers say which ledger it is and at
/// which block. Whitespace differs from OCaml's pretty printer. Disabled
/// unless `--enable-ledger-export` is set on the operator node. One export
/// runs at a time, including response delivery.
#[utoipa::path(
    get,
    path = "/ledger/export/{kind}",
    tag = "ledger",
    params(
        ("kind" = String, Path,
            description = "staged-ledger, snarked-ledger, staking-epoch-ledger or next-epoch-ledger")
    ),
    responses(
        (status = 200, description = "Accounts JSON; x-mina-ledger-hash, x-mina-num-accounts, \
            x-mina-state-hash, x-mina-blockchain-length, x-mina-global-slot-since-genesis and \
            x-mina-epoch headers"),
        (status = 403, description = "Ledger export is disabled", body = JsonErrorResponse),
        (status = 400, description = "Unknown ledger kind", body = JsonErrorResponse),
        (status = 503, description = "Another export is running, or the ledger is not available",
            body = JsonErrorResponse)
    )
)]
async fn export(State(state): State<AppState>, Path(kind): Path<String>) -> AppResult<Response> {
    // Reject before parsing or dispatching any ledger RPC. This route is only
    // mounted by the private OpenAPI router, never R34's public_router.
    if !state.ledger_export() {
        return Err(AppError::Forbidden(
            "ledger export is disabled; start the operator node with --enable-ledger-export \
             and restrict access to its HTTP port"
                .into(),
        ));
    }
    let kind: LedgerExportKind = kind.parse().map_err(AppError::BadRequest)?;
    let permit = EXPORTS.try_acquire().map_err(|_| {
        AppError::ServiceUnavailable("another ledger export is running, try again later".into())
    })?;
    // Keep the permit if the HTTP handler is cancelled while the ledger thread
    // is still exporting. Dropping a JoinHandle detaches this bounded task;
    // its response (and permit) will be dropped when the RPC completes.
    tokio::spawn(async move {
        let response: RpcLedgerExportResponse = state
            .rpc_sender()
            .oneshot_request(RpcRequest::LedgerExport(kind))
            .await
            .ok_or(AppError::ChannelDropped)?;
        let export = response.map_err(AppError::ServiceUnavailable)?;
        Ok(export_response(export, permit))
    })
    .await
    .map_err(|err| AppError::Internal(format!("ledger export task failed: {err}")))?
}

/// Which block `GET /hardfork/config` continues from. Neither: the newest
/// best-chain block before `slot_tx_end`, as OCaml's
/// `generate-hardfork-config` does.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct HardForkQuery {
    /// A best-chain block by state hash
    pub state_hash: Option<String>,
    /// A best-chain block by height
    pub height: Option<u32>,
    /// Diagnostic/rehearsal override: the fork block may still change
    #[serde(default)]
    pub allow_before_stop: bool,
}

impl HardForkQuery {
    fn spec(&self) -> Result<HardForkBlockSpec, String> {
        match (&self.state_hash, self.height) {
            (None, None) => Ok(HardForkBlockSpec::StopSlot),
            (Some(hash), None) => hash
                .parse()
                .map(HardForkBlockSpec::StateHash)
                .map_err(|e| format!("invalid state hash {hash}: {e}")),
            (None, Some(height)) => Ok(HardForkBlockSpec::Height(height)),
            (Some(_), Some(_)) => Err("give a state hash or a height, not both".to_owned()),
        }
    }
}

pub const HARD_FORK_CONTENT_TYPE: &str = "application/x-mina-hardfork-export";

/// The next hard fork's runtime config, from this node's own state
///
/// The fork block is the newest best-chain block before `slot_tx_end` (or the
/// one named), as in OCaml 4.0.2's `mina advanced generate-hardfork-config`.
/// The body's first line is a JSON manifest: the fork inputs, each ledger's
/// root and migrated root, and the hash-only `daemon.json` (stable and
/// migrated). The rest is `fork_config.json`, every account inline, in the
/// layout of OCaml's GraphQL `fork_config`. `manifest.inputs.stop_reached` is
/// false while a block before `slot_tx_end` may still arrive; such requests
/// require the explicit `allow_before_stop=true` diagnostic override for every
/// block selector. Disabled unless `--enable-ledger-export` is set; one export
/// at a time, including response delivery.
#[utoipa::path(
    get,
    path = "/hardfork/config",
    tag = "ledger",
    params(HardForkQuery),
    responses(
        (status = 200, description = "Manifest line, then fork_config.json"),
        (status = 403, description = "Ledger export is disabled", body = JsonErrorResponse),
        (status = 400, description = "Both a state hash and a height", body = JsonErrorResponse),
        (status = 503, description = "Another export is running, or no fork block or ledger",
            body = JsonErrorResponse)
    )
)]
async fn hard_fork_config(
    State(state): State<AppState>,
    Query(query): Query<HardForkQuery>,
) -> AppResult<Response> {
    if !state.ledger_export() {
        return Err(AppError::Forbidden(
            "ledger export is disabled; start the operator node with --enable-ledger-export \
             and restrict access to its HTTP port"
                .into(),
        ));
    }
    let spec = query.spec().map_err(AppError::BadRequest)?;
    let permit = EXPORTS.try_acquire().map_err(|_| {
        AppError::ServiceUnavailable("another ledger export is running, try again later".into())
    })?;
    tokio::spawn(async move {
        let response: RpcHardForkExportResponse = state
            .rpc_sender()
            .oneshot_request(RpcRequest::HardForkConfig(spec))
            .await
            .ok_or(AppError::ChannelDropped)?;
        let export = response.map_err(AppError::ServiceUnavailable)?;
        if !export.inputs.stop_reached && !query.allow_before_stop {
            return Err(AppError::ServiceUnavailable(format!(
                "best tip slot {} has not reached slot_tx_end {:?}; wait or explicitly set allow_before_stop=true for a rehearsal",
                export.inputs.best_tip.curr_global_slot_since_hard_fork,
                export.inputs.stop_slots.slot_tx_end,
            )));
        }
        Ok(hard_fork_response(export, permit))
    })
    .await
    .map_err(|err| AppError::Internal(format!("hard fork export task failed: {err}")))?
}

enum Segment {
    Owned(Bytes),
    Ledger(usize),
}

/// The manifest line, then `fork_config.json` with the three account arrays
/// streamed from the shared RPC answer, 64 KiB at a time.
fn hard_fork_body(export: Arc<RpcHardForkExport>, permit: SemaphorePermit<'static>) -> Body {
    let mut manifest = serde_json::to_vec(&HardForkManifest::of_export(&export))
        .expect("serializing to a Vec cannot fail");
    manifest.push(b'\n');
    let [p0, p1, p2, p3] = fork_config_parts(&export).map(Bytes::from);
    let segments = vec![
        Segment::Owned(manifest.into()),
        Segment::Owned(p0),
        Segment::Ledger(0),
        Segment::Owned(p1),
        Segment::Ledger(1),
        Segment::Owned(p2),
        Segment::Ledger(2),
        Segment::Owned(p3),
    ];
    Body::from_stream(stream::unfold(
        (export, segments, 0usize, 0usize, permit),
        |(export, segments, mut segment, mut offset, permit)| async move {
            loop {
                let data: &[u8] = match segments.get(segment)? {
                    Segment::Owned(bytes) => bytes,
                    Segment::Ledger(i) => [&export.ledger, &export.staking, &export.next][*i]
                        .accounts_json
                        .as_bytes(),
                };
                if offset < data.len() {
                    let end = (offset + EXPORT_CHUNK_BYTES).min(data.len());
                    let chunk = Bytes::copy_from_slice(&data[offset..end]);
                    return Some((
                        Ok::<_, Infallible>(chunk),
                        (export, segments, segment, end, permit),
                    ));
                }
                segment += 1;
                offset = 0;
            }
        },
    ))
}

fn hard_fork_response(
    export: Arc<RpcHardForkExport>,
    permit: SemaphorePermit<'static>,
) -> Response {
    let header = |value: String| HeaderValue::from_str(&value).expect("ASCII header value");
    let fork = &export.inputs.fork;
    let headers = [
        (header::CONTENT_TYPE, header(HARD_FORK_CONTENT_TYPE.into())),
        (
            HeaderName::from_static(HEADER_STATE_HASH),
            header(fork.state_hash.to_string()),
        ),
        (
            HeaderName::from_static(HEADER_BLOCKCHAIN_LENGTH),
            header(fork.blockchain_length.to_string()),
        ),
    ];
    (headers, hard_fork_body(export, permit)).into_response()
}

/// Chunk the shared RPC allocation without cloning the whole String. There is
/// one ledger-sized JSON buffer, plus at most 64 KiB per body poll and the
/// transport buffers. The body owns the admission permit until EOF or drop.
const EXPORT_CHUNK_BYTES: usize = 64 * 1024;

fn export_body<T: AsRef<[u8]> + Send + 'static>(data: T, permit: SemaphorePermit<'static>) -> Body {
    Body::from_stream(stream::unfold(
        (data, 0, permit),
        |(data, offset, permit)| async move {
            let bytes = data.as_ref();
            if offset == bytes.len() {
                return None;
            }
            let end = (offset + EXPORT_CHUNK_BYTES).min(bytes.len());
            let chunk = Bytes::copy_from_slice(&bytes[offset..end]);
            Some((Ok::<_, Infallible>(chunk), (data, end, permit)))
        },
    ))
}

struct ExportJson(Arc<RpcLedgerExport>);

impl AsRef<[u8]> for ExportJson {
    fn as_ref(&self) -> &[u8] {
        self.0.accounts_json.as_bytes()
    }
}

fn export_response(export: Arc<RpcLedgerExport>, permit: SemaphorePermit<'static>) -> Response {
    let header = |value: String| HeaderValue::from_str(&value).expect("ASCII header value");
    let headers = [
        (header::CONTENT_TYPE, header("application/json".into())),
        (
            HeaderName::from_static(HEADER_LEDGER_KIND),
            header(export.kind.to_string()),
        ),
        (
            HeaderName::from_static(HEADER_LEDGER_HASH),
            header(export.ledger_hash.to_string()),
        ),
        (
            HeaderName::from_static(HEADER_NUM_ACCOUNTS),
            header(export.num_accounts.to_string()),
        ),
        (
            HeaderName::from_static(HEADER_STATE_HASH),
            header(export.best_tip.state_hash.to_string()),
        ),
        (
            HeaderName::from_static(HEADER_BLOCKCHAIN_LENGTH),
            header(export.best_tip.blockchain_length.to_string()),
        ),
        (
            HeaderName::from_static(HEADER_GLOBAL_SLOT),
            header(export.best_tip.global_slot_since_genesis.to_string()),
        ),
        (
            HeaderName::from_static(HEADER_EPOCH),
            header(export.best_tip.epoch.to_string()),
        ),
    ];
    (headers, export_body(ExportJson(export), permit)).into_response()
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use mina_node::core::channels::mpsc;
    use mina_node_common::rpc::{NodeRpcRequest, RpcSender};

    use super::*;

    fn rehearsal_export(stop_reached: bool) -> Arc<RpcHardForkExport> {
        let manifest: HardForkManifest = serde_json::from_str(include_str!(
            "../../../../../../tests/files/fork-export/fork/rust/manifest.json"
        ))
        .unwrap();
        let config: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../../tests/files/fork-export/fork/rust/fork_config.json"
        ))
        .unwrap();
        let ledger = |summary: mina_node::daemon_json::HardForkLedgerSummary,
                      accounts: &serde_json::Value| {
            mina_node::rpc::HardForkLedger {
                hash: summary.hash.parse().unwrap(),
                migrated_hash: summary.migrated_hash.parse().unwrap(),
                num_accounts: summary.num_accounts,
                accounts_json: accounts.to_string(),
            }
        };
        let mut inputs = manifest.inputs;
        inputs.stop_reached = stop_reached;
        Arc::new(RpcHardForkExport {
            inputs,
            chain_id: manifest.chain_id,
            ledger: ledger(manifest.ledgers.ledger, &config["ledger"]["accounts"]),
            staking: ledger(
                manifest.ledgers.staking,
                &config["epoch_data"]["staking"]["accounts"],
            ),
            next: ledger(
                manifest.ledgers.next,
                &config["epoch_data"]["next"]["accounts"],
            ),
        })
    }

    #[tokio::test]
    async fn hard_fork_requires_stop_for_every_selector_and_holds_body_permit() {
        use mina_node::core::channels::oneshot;
        let _guard = EXPORT_TEST_LOCK.lock().await;
        for selector in [
            serde_json::json!({}),
            serde_json::json!({"height":545439}),
            serde_json::json!({"state_hash":"3NLvdGJk7DjwHeMW9Hhmv2AMrRnaou5NXfU2KMfBF3MXx6TEQEui"}),
        ] {
            for (stopped, allow) in [(false, false), (false, true), (true, false)] {
                let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
                let state = AppState::new(RpcSender::new(tx)).with_ledger_export(true);
                let mut query = selector.clone();
                query["allow_before_stop"] = allow.into();
                let query: HardForkQuery = serde_json::from_value(query).unwrap();
                let request = hard_fork_config(State(state.clone()), Query(query));
                let respond = async {
                    let request = rx.recv().await.unwrap();
                    assert!(matches!(request.req, RpcRequest::HardForkConfig(_)));
                    request
                        .responder
                        .downcast::<oneshot::Sender<RpcHardForkExportResponse>>()
                        .unwrap()
                        .send(Ok(rehearsal_export(stopped)))
                        .unwrap();
                };
                let (response, ()) = tokio::join!(request, respond);
                if !stopped && !allow {
                    assert!(
                        matches!(response, Err(AppError::ServiceUnavailable(ref e)) if e.contains("allow_before_stop"))
                    );
                    assert_eq!(EXPORTS.available_permits(), 1);
                } else {
                    let mut body = response.unwrap().into_body().into_data_stream();
                    body.next().await.unwrap().unwrap();
                    let busy = export(State(state), Path("staged-ledger".into()))
                        .await
                        .unwrap_err();
                    assert!(matches!(busy, AppError::ServiceUnavailable(_)));
                    assert!(rx.try_recv().is_err());
                    drop(body);
                    assert_eq!(EXPORTS.available_permits(), 1);
                }
            }
        }
    }

    #[tokio::test]
    async fn hard_fork_is_disabled_before_rpc_even_with_override() {
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
        let query = serde_json::from_value(serde_json::json!({"allow_before_stop":true})).unwrap();
        let response =
            hard_fork_config(State(AppState::new(RpcSender::new(tx))), Query(query)).await;
        assert!(matches!(response, Err(AppError::Forbidden(_))));
        assert!(rx.try_recv().is_err());
    }

    fn semaphore() -> &'static Semaphore {
        Box::leak(Box::new(Semaphore::new(1)))
    }

    #[tokio::test]
    async fn export_permit_is_held_until_body_eof() {
        let semaphore = semaphore();
        let permit = semaphore.try_acquire().unwrap();
        let data = vec![b'x'; EXPORT_CHUNK_BYTES * 2 + 1];
        let mut body = export_body(data, permit).into_data_stream();
        // Returning the response and polling any data, even the last chunk,
        // must not admit another export while the body is still in flight.
        assert!(semaphore.try_acquire().is_err());
        for expected in [EXPORT_CHUNK_BYTES, EXPORT_CHUNK_BYTES, 1] {
            assert_eq!(body.next().await.unwrap().unwrap().len(), expected);
            assert!(semaphore.try_acquire().is_err());
        }
        assert!(body.next().await.is_none());
        assert_eq!(semaphore.available_permits(), 1);
    }

    #[tokio::test]
    async fn export_permit_is_released_on_body_disconnect() {
        let semaphore = semaphore();
        let permit = semaphore.try_acquire().unwrap();
        let mut body = export_body(vec![b'x'; EXPORT_CHUNK_BYTES * 2], permit).into_data_stream();
        body.next().await.unwrap().unwrap();
        assert!(semaphore.try_acquire().is_err());
        drop(body); // Hyper drops the response body on a client disconnect.
        assert_eq!(semaphore.available_permits(), 1);

        let body = export_body(b"[]", semaphore.try_acquire().unwrap());
        drop(body); // A disconnect before the first body poll also releases it.
        assert_eq!(semaphore.available_permits(), 1);
    }

    #[tokio::test]
    async fn export_handler_keeps_permit_in_successful_response_body() {
        use mina_node::{core::channels::oneshot, rpc::LedgerExportBestTip};
        use mina_p2p_messages::v2::StateHash;

        let _guard = EXPORT_TEST_LOCK.lock().await;
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
        let state = AppState::new(RpcSender::new(tx)).with_ledger_export(true);
        let pending = tokio::spawn(export(
            State(state.clone()),
            Path("staking-epoch-ledger".to_owned()),
        ));
        let request = rx.recv().await.unwrap();
        let responder = request
            .responder
            .downcast::<oneshot::Sender<RpcLedgerExportResponse>>()
            .unwrap();
        // Normalized OCaml account fixture and its independently measured root.
        let sample = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/daemon_json/testdata/ocaml-ledger-export-sample.json"
        ));
        let hash = "jwUATMQxpP7A3xwoW7uEwS3M83zGi3prDUPSqEMpqUzvssSMaW3";
        responder
            .send(Ok(Arc::new(RpcLedgerExport {
                kind: LedgerExportKind::StakingEpochLedger,
                ledger_hash: hash.parse().unwrap(),
                best_tip: LedgerExportBestTip {
                    state_hash: StateHash::zero(),
                    blockchain_length: 1,
                    global_slot_since_genesis: 0,
                    epoch: 0,
                },
                num_accounts: 17,
                accounts_json: sample.to_owned(),
            })))
            .unwrap();
        let response = pending.await.unwrap().unwrap();
        assert_eq!(response.headers()[HEADER_LEDGER_HASH], hash);
        let error = export(State(state), Path("next-epoch-ledger".to_owned()))
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::ServiceUnavailable(_)));
        assert!(
            rx.try_recv().is_err(),
            "delivery must not admit another RPC"
        );
        let body = axum::body::to_bytes(response.into_body(), sample.len())
            .await
            .unwrap();
        assert_eq!(body.as_ref(), sample.as_bytes());
        assert_eq!(EXPORTS.available_permits(), 1);
    }

    #[tokio::test]
    async fn export_keeps_permit_during_rpc_after_handler_cancellation() {
        let _guard = EXPORT_TEST_LOCK.lock().await;
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(16);
        let state = AppState::new(RpcSender::new(tx)).with_ledger_export(true);
        let pending = tokio::spawn(export(
            State(state.clone()),
            Path("staged-ledger".to_owned()),
        ));
        let request = rx.recv().await.unwrap();
        assert!(matches!(request.req, RpcRequest::LedgerExport(_)));
        pending.abort();
        let _ = pending.await;
        let error = export(State(state), Path("next-epoch-ledger".to_owned()))
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::ServiceUnavailable(_)));
        assert!(rx.try_recv().is_err(), "busy export must not dispatch RPC");
        drop(request); // The ledger RPC has now completed (channel dropped).
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while EXPORTS.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
