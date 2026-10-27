//! Snarker endpoints.
//!
//! - `POST /snarker/job/commit` - Commit to a snark job
//! - `GET /snarker/job/spec` - Get snark job specification
//! - `GET /snarker/workers` - Get snarker workers
//! - `GET /snarker/config` - Get snarker configuration
//! - `GET /snarker/control` - The worker as the operator sees it (P5.4)
//! - `POST /snarker/control` - Start, re-price or stop the worker, if the
//!   node was started with `--allow-worker-control` (P5.4)
//! - `GET /snarker/included` - Our work that blocks included (P5.4)

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderMap, Response, StatusCode},
    Json,
};
use mina_p2p_messages::binprot::BinProtWrite;
use serde::{Deserialize, Serialize};
use utoipa_axum::{router::OpenApiRouter, routes};

use mina_node::{
    account::{parse_snarker, AccountPublicKey},
    external_snark_worker::{fee_rule_problem, RpcSnarkWorkerControl},
    rpc::{
        AccountQuery, RpcRequest, RpcSnarkWorkIncludedGetResponse,
        RpcSnarkWorkerControlSetResponse, RpcSnarkerConfigGetResponse, RpcSnarkerJobCommitResponse,
        RpcSnarkerJobSpecResponse, RpcSnarkerWorkersResponse, WORKER_CONTROL_NOT_ALLOWED,
    },
    snark_pool::ProverAccount,
    SnarkerSetting,
};

use crate::http_server::{AppError, AppResult, AppState};

/// Snarker routes
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(job_commit))
        .routes(routes!(job_spec))
        .routes(routes!(workers))
        .routes(routes!(config))
        .routes(routes!(control_get, control_set))
        .routes(routes!(included))
}

/// Commit to a snark job
#[utoipa::path(
    post,
    path = "/snarker/job/commit",
    tag = "snarker",
    responses(
        (status = 201, description = "Job committed"),
        (status = 400, description = "Invalid input or job error")
    )
)]
async fn job_commit(
    State(state): State<AppState>,
    body: String,
) -> AppResult<(StatusCode, Json<RpcSnarkerJobCommitResponse>)> {
    // TODO(binier): make endpoint only accessible locally.
    // TODO(axum-migration): Error returns bare JSON string for warp compatibility.
    // Migrate to structured error (e.g., `{"error": "...", "details": {...}}`).
    let job_id = crate::input_limits::job_id(&body)
        .map_err(|_| AppError::Json(StatusCode::BAD_REQUEST, serde_json::json!("invalid_input")))?;

    let resp: RpcSnarkerJobCommitResponse =
        rpc_request!(state, RpcRequest::SnarkerJobCommit { job_id })?;

    let status = match &resp {
        RpcSnarkerJobCommitResponse::Ok => StatusCode::CREATED,
        _ => StatusCode::BAD_REQUEST,
    };

    Ok((status, Json(resp)))
}

#[derive(Deserialize)]
struct JobSpecQuery {
    id: crate::input_limits::JobId,
}

/// Snark job specification
///
/// Supports JSON and binary (binprot) output based on Accept header.
#[utoipa::path(
    get,
    path = "/snarker/job/spec",
    tag = "snarker",
    params(
        ("id" = String, Query, description = "Snark job ID")
    ),
    responses(
        (status = 200, description = "JSON job spec", content_type = "application/json"),
        (status = 200, description = "Binprot job spec", content_type = "application/octet-stream"),
        (status = 400, description = "Job not found")
    )
)]
async fn job_spec(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(JobSpecQuery { id: job_id }): Query<JobSpecQuery>,
) -> AppResult<Response<Body>> {
    let resp: RpcSnarkerJobSpecResponse =
        rpc_request!(state, RpcRequest::SnarkerJobSpec { job_id: job_id.0 })?;

    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    match resp {
        RpcSnarkerJobSpecResponse::Ok(spec) if accept == "application/octet-stream" => {
            // Binary output (binprot format with length prefix)
            let mut vec = Vec::new();
            spec.binprot_write(&mut vec)
                .map_err(|e| AppError::Internal(format!("binprot serialization failed: {e}")))?;

            let mut result = Vec::with_capacity(vec.len() + std::mem::size_of::<u64>());
            result.extend((vec.len() as u64).to_le_bytes());
            result.extend(vec);

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from(result))
                .map_err(|e| AppError::Internal(e.to_string()))
        }
        RpcSnarkerJobSpecResponse::Ok(spec) => {
            // JSON output
            let body = serde_json::to_vec(&spec)
                .map_err(|e| AppError::Internal(format!("JSON serialization failed: {e}")))?;

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .map_err(|e| AppError::Internal(e.to_string()))
        }
        _ => {
            // Error response
            let body = serde_json::to_vec(&"error")
                .map_err(|e| AppError::Internal(format!("JSON serialization failed: {e}")))?;

            Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .map_err(|e| AppError::Internal(e.to_string()))
        }
    }
}

/// Snarker workers
#[utoipa::path(
    get,
    path = "/snarker/workers",
    tag = "snarker",
    responses(
        (status = 200, description = "Snarker workers")
    )
)]
async fn workers(State(state): State<AppState>) -> AppResult<Json<RpcSnarkerWorkersResponse>> {
    jsonify_rpc!(state, RpcRequest::SnarkerWorkers)
}

/// Snarker configuration
#[utoipa::path(
    get,
    path = "/snarker/config",
    tag = "snarker",
    responses(
        (status = 200, description = "Snarker configuration")
    )
)]
async fn config(State(state): State<AppState>) -> AppResult<Json<RpcSnarkerConfigGetResponse>> {
    jsonify_rpc!(state, RpcRequest::SnarkerConfig)
}

/// The worker as the operator sees it, with what OCaml's snark pool would
/// say of its key and fee.
#[derive(Serialize)]
struct ControlResponse {
    #[serde(flatten)]
    control: RpcSnarkWorkerControl,
    /// Whether the worker key's account exists at the best tip; `null` if
    /// the node can't tell yet.
    account_exists: Option<bool>,
    /// Why work at this fee from this key would be refused, if it would.
    fee_problem: Option<String>,
}

/// How long to wait for the key's account.
const ACCOUNT_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The key's default-token account at the best tip, as the snark pool sees a
/// prover; `None` if the node has no best tip yet or doesn't answer in time.
async fn prover_account(
    state: &AppState,
    control: &RpcSnarkWorkerControl,
    key: &AccountPublicKey,
) -> Option<ProverAccount> {
    // Without a best tip the query would wait for one.
    control.best_tip_height?;
    let query = RpcRequest::LedgerAccountsGet(AccountQuery::PubKeyWithTokenId(
        key.clone(),
        mina_p2p_messages::v2::TokenIdKeyHash::default(),
    ));
    let accounts: Vec<ledger::Account> = tokio::time::timeout(
        ACCOUNT_LOOKUP_TIMEOUT,
        state.rpc_sender().oneshot_request(query),
    )
    .await
    .ok()??;
    Some(ProverAccount::from_account(accounts.first()))
}

async fn control_response(state: &AppState, control: RpcSnarkWorkerControl) -> ControlResponse {
    let (account_exists, fee_problem) = match (&control.public_key, control.fee) {
        (Some(key), Some(fee)) => match prover_account(state, &control, key).await {
            Some(account) => (
                Some(matches!(account, ProverAccount::Present { .. })),
                fee_rule_problem(fee, account),
            ),
            None => (None, None),
        },
        _ => (None, None),
    };
    ControlResponse {
        control,
        account_exists,
        fee_problem,
    }
}

/// The SNARK worker as the operator sees it
///
/// Whether control is allowed, the key and fee (nanomina), the worker's
/// state and current job, its latest outcomes with their times, and whether
/// the key's account exists (OCaml asks at least 1 MINA of a key without
/// one).
#[utoipa::path(
    get,
    path = "/snarker/control",
    tag = "snarker",
    responses(
        (status = 200, description = "The SNARK worker and its setting")
    )
)]
async fn control_get(State(state): State<AppState>) -> AppResult<Json<serde_json::Value>> {
    let control: RpcSnarkWorkerControl = rpc_request!(state, RpcRequest::SnarkWorkerControlGet)?;
    let response = control_response(&state, control).await;
    serde_json::to_value(response)
        .map(Json)
        .map_err(|error| AppError::Internal(error.to_string()))
}

/// Refuses a request that a page of another site could have sent (P5.4's
/// review, [`crate::http_server::control_origin`]; the node's own page at an
/// IP address is not another site, F-console), and reads its body.
fn operator_request(
    state: &AppState,
    headers: &HeaderMap,
    body: &[u8],
) -> AppResult<ControlRequest> {
    use crate::http_server::control_origin::WORKER_CONTROL_HEADER;
    operator_json(
        state,
        headers,
        body,
        WORKER_CONTROL_HEADER,
        "worker control",
    )
    .map_err(|error| match error {
        AppError::BadRequest(message) => AppError::BadRequest(message.replacen(
            "invalid worker control request",
            "invalid worker change",
            1,
        )),
        error => error,
    })
}

/// An operator's request with a JSON body: refused if a page of another
/// site could have sent it ([`crate::http_server::control_origin`]: its
/// `Origin` must be this machine's, listed with `--worker-control-origin`,
/// or the node's own dashboard at an IP address), or if it lacks the
/// `header` (which a page can't add without a CORS preflight) or JSON.
/// `what` names it in the errors: "worker control", "the bug report".
pub(crate) fn operator_json<T: serde::de::DeserializeOwned>(
    state: &AppState,
    headers: &HeaderMap,
    body: &[u8],
    header: &str,
    what: &str,
) -> AppResult<T> {
    if let Some(origin) = headers.get(header::ORIGIN) {
        let host = headers
            .get(header::HOST)
            .and_then(|host| host.to_str().ok());
        let allowed = origin
            .to_str()
            .is_ok_and(|origin| state.control_origins().allows_request(origin, host));
        if !allowed {
            return Err(AppError::Forbidden(format!(
                "{what} from this page's origin is refused: allow it with \
                 --worker-control-origin"
            )));
        }
    }
    if !headers.contains_key(header) {
        return Err(AppError::Forbidden(format!(
            "{what} needs the {header} header"
        )));
    }
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        });
    if !json {
        return Err(AppError::Json(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            serde_json::json!({ "error": "expected Content-Type: application/json" }),
        ));
    }
    serde_json::from_slice(body)
        .map_err(|error| AppError::BadRequest(format!("invalid {what} request: {error}")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlRequest {
    /// `true` starts or re-prices the worker, `false` stops it.
    enabled: bool,
    /// The public key (B62...) that receives the fees.
    public_key: Option<String>,
    /// The fee per bundle in MINA, as `--snarker-fee` takes it (default 0.1).
    fee: Option<String>,
}

/// Start, re-price or stop the SNARK worker
///
/// Only if the node was started with `--allow-worker-control`; never on a
/// node with a public API. `{"enabled": true, "public_key": "B62...", "fee":
/// "0.004"}` starts the worker or changes its key or fee; `{"enabled":
/// false}` stops it. The request needs `Content-Type: application/json` and
/// an `X-Mina-Worker-Control` header, and a page may send it only from this
/// machine, from the node's own dashboard opened at an IP address, or from an
/// origin given with `--worker-control-origin`. The setting is
/// kept in the work directory: a restart brings it back unless
/// `--run-snarker` is given. A fee that OCaml's snark pool would refuse for
/// the key (below 1 MINA for a key without an account) is refused.
#[utoipa::path(
    post,
    path = "/snarker/control",
    tag = "snarker",
    responses(
        (status = 200, description = "The worker after the change"),
        (status = 400, description = "Invalid key or fee, or a fee OCaml would refuse"),
        (status = 403, description = "Worker control is off, or the request is not the operator's"),
        (status = 415, description = "Not JSON")
    )
)]
async fn control_set(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> AppResult<Json<serde_json::Value>> {
    if !state.worker_control() {
        tracing::warn!(
            kind = "SnarkWorkerControlRefused",
            reason = "control disabled"
        );
        return Err(AppError::Forbidden(WORKER_CONTROL_NOT_ALLOWED.to_owned()));
    }
    let request = operator_request(&state, &headers, &body).map_err(|error| {
        if matches!(error, AppError::Forbidden(_)) {
            tracing::warn!(
                kind = "SnarkWorkerControlRefused",
                reason = "operator request refused"
            );
        }
        error
    })?;
    let setting = if request.enabled {
        let (public_key, fee) =
            parse_snarker(request.public_key.as_deref(), request.fee.as_deref())
                .map_err(AppError::BadRequest)?
                .ok_or_else(|| {
                    AppError::BadRequest(
                        "a public key (B62...) is needed to start the worker".into(),
                    )
                })?;
        let control: RpcSnarkWorkerControl =
            rpc_request!(state, RpcRequest::SnarkWorkerControlGet)?;
        if let Some(problem) = prover_account(&state, &control, &public_key)
            .await
            .and_then(|account| fee_rule_problem(fee, account))
        {
            return Err(AppError::BadRequest(problem));
        }
        Some(SnarkerSetting { public_key, fee })
    } else {
        None
    };
    let result: RpcSnarkWorkerControlSetResponse =
        rpc_request!(state, RpcRequest::SnarkWorkerControlSet(setting))?;
    let control = result.map_err(AppError::Forbidden)?;
    let response = control_response(&state, control).await;
    serde_json::to_value(response)
        .map(Json)
        .map_err(|error| AppError::Internal(error.to_string()))
}

/// Our SNARK work that blocks included
///
/// Best-chain blocks that bought work from the worker's keys, newest first
/// (at most the last 500): height, state hash, time, producer, our bundles
/// and their fees, and whether the block is pending, final, dropped by a
/// reorg or unknown. Totals of final work since the log began, and of work
/// in pending blocks. With them the best tip's height (a block's
/// confirmations are the blocks on top of it) and the market from every
/// prover: what the best chain bought (the newest purchases, their fees) and
/// the completed work in the SNARK pool per prover (the ten cheapest others
/// and the worker's own keys, with a count and the top fee of all others),
/// built in time linear in the pool and bounded in size.
#[utoipa::path(
    get,
    path = "/snarker/included",
    tag = "snarker",
    responses(
        (status = 200, description = "The included-work log")
    )
)]
async fn included(
    State(state): State<AppState>,
) -> AppResult<Json<RpcSnarkWorkIncludedGetResponse>> {
    jsonify_rpc!(state, RpcRequest::SnarkWorkIncludedGet)
}
