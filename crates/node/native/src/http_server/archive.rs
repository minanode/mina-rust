//! The archive feed's operator routes. They are on the private router only
//! (never with `--public-graphql`), and each answers 404 when the node
//! doesn't feed an archive process (`--archive-archiver-process`):
//!
//! - `GET /archive/status`: the outbox and its delivery.
//! - `GET /archive/blocks`: pending, in-flight and retained entries, newest
//!   first, from the outbox's index. Metadata only: no diff is read from
//!   disk. Filters: `height`, `state_hash`, `state` (`pending` or
//!   `retained`), and `limit` (default [`DEFAULT_LIST_LIMIT`], at most
//!   [`MAX_LIST_LIMIT`]).
//! - `POST /archive/resend`: requeues the retained diff of one block,
//!   `{"state_hash": "3N..."}`, or every retained diff, `{"all": true}`. The
//!   OCaml archive is idempotent by state hash, so a block it has is a no-op
//!   there.
//!
//! Replay is guarded like worker control ([`super::control_origin`]): the
//! request carries [`ARCHIVE_CONTROL_HEADER`] and a JSON body, which a page
//! of another site can't send without a CORS preflight that the node
//! refuses. A page's `Origin` must be this machine's (`localhost`,
//! `127.0.0.1`, `[::1]`), one listed with `--worker-control-origin`, or the
//! node's own dashboard opened at an IP address. A program sends no
//! `Origin` and is let through. Only the origins that may change the worker
//! get CORS headers for it ([`super::types::cors_layer`]).

use axum::{
    body::Bytes,
    extract::{rejection::QueryRejection, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

use mina_node_common::archive::{
    delivery::SharedOutbox,
    outbox::{ListQuery, ResendOutcome, StateFilter},
};

use super::{routes::snarker::operator_json, AppError, AppState, MAX_STATE_HASH_CHARS};

/// The header a replay request must carry.
pub const ARCHIVE_CONTROL_HEADER: &str = "x-mina-archive-control";

/// The path whose requests the origin rules apply to.
pub const ARCHIVE_RESEND_PATH: &str = "/archive/resend";

/// Entries `GET /archive/blocks` lists without a `limit`.
pub const DEFAULT_LIST_LIMIT: usize = 50;

/// The most entries one `GET /archive/blocks` lists.
pub const MAX_LIST_LIMIT: usize = 500;

/// How a program asks for a replay, for the refusals it gets without the
/// header or JSON.
const CURL_HINT: &str = "send it as: curl -X POST -H 'x-mina-archive-control: 1' \
     -H 'content-type: application/json' -d '{\"state_hash\": \"3N...\"}' \
     (or -d '{\"all\": true}') <node>/archive/resend";

pub fn routes(router: axum::Router<AppState>) -> axum::Router<AppState> {
    router
        .route("/archive/status", axum::routing::get(status))
        .route("/archive/blocks", axum::routing::get(blocks))
        .route(ARCHIVE_RESEND_PATH, axum::routing::post(resend))
}

fn not_an_archive_node() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": "this node does not feed an archive process (--archive-archiver-process)"
        })),
    )
        .into_response()
}

/// `GET /archive/status`: the outbox's pending entries, delivery failures
/// and history gaps.
async fn status(State(state): State<AppState>) -> Response {
    match state.archive_outbox() {
        Some(outbox) => Json(outbox.status()).into_response(),
        None => not_an_archive_node(),
    }
}

#[derive(Debug, Deserialize)]
struct ListParams {
    limit: Option<usize>,
    height: Option<u32>,
    state_hash: Option<String>,
    state: Option<StateFilter>,
}

/// `GET /archive/blocks`: the outbox's entries, newest first.
async fn blocks(
    State(state): State<AppState>,
    params: Result<Query<ListParams>, QueryRejection>,
) -> Response {
    let Some(outbox) = state.archive_outbox() else {
        return not_an_archive_node();
    };
    let params = match params {
        Ok(Query(params)) => params,
        Err(rejection) => return AppError::BadRequest(rejection.body_text()).into_response(),
    };
    let state_hash = match params.state_hash.map(|hash| hash.trim().to_owned()) {
        Some(hash) if hash.chars().count() > MAX_STATE_HASH_CHARS => {
            return AppError::BadRequest(format!(
                "state_hash is longer than {MAX_STATE_HASH_CHARS} characters"
            ))
            .into_response()
        }
        Some(hash) if hash.is_empty() => None,
        hash => hash,
    };
    let query = ListQuery {
        height: params.height,
        state_hash,
        state: params.state,
        limit: params
            .limit
            .unwrap_or(DEFAULT_LIST_LIMIT)
            .min(MAX_LIST_LIMIT),
    };
    Json(outbox.list(&query)).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResendRequest {
    #[serde(default)]
    state_hash: Option<String>,
    #[serde(default)]
    all: bool,
}

/// What to replay.
enum Replay {
    Block(String),
    All,
}

fn replay_of(request: ResendRequest) -> Result<Replay, AppError> {
    match (request.state_hash, request.all) {
        (Some(hash), false) => {
            let hash = hash.trim().to_owned();
            if hash.is_empty() {
                Err(AppError::BadRequest("state_hash is empty".to_owned()))
            } else if hash.chars().count() > MAX_STATE_HASH_CHARS {
                Err(AppError::BadRequest(format!(
                    "state_hash is longer than {MAX_STATE_HASH_CHARS} characters"
                )))
            } else {
                Ok(Replay::Block(hash))
            }
        }
        (None, true) => Ok(Replay::All),
        _ => Err(AppError::BadRequest(
            "give {\"state_hash\": \"3N...\"} for one block, or {\"all\": true} for every \
             retained block"
                .to_owned(),
        )),
    }
}

/// `POST /archive/resend`: requeue one block's retained diff, or every
/// retained diff, and wake delivery.
async fn resend(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(outbox) = state.archive_outbox() else {
        return not_an_archive_node();
    };
    let request = operator_json::<ResendRequest>(
        &state,
        &headers,
        &body,
        ARCHIVE_CONTROL_HEADER,
        "archive replay",
    )
    .map_err(|error| with_curl_hint(error, &headers))
    .and_then(replay_of);
    let replay = match request {
        Ok(replay) => replay,
        Err(error) => return error.into_response(),
    };
    // Moving files and syncing directories must not block the runtime.
    match tokio::task::spawn_blocking(move || replay_now(&outbox, replay)).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => AppError::Internal(error.to_string()).into_response(),
        Err(error) => AppError::Internal(error.to_string()).into_response(),
    }
}

/// A program's refusal for a missing header or body says how to send it.
fn with_curl_hint(error: AppError, headers: &HeaderMap) -> AppError {
    if headers.contains_key(header::ORIGIN) {
        return error;
    }
    match error {
        AppError::Forbidden(message) => AppError::Forbidden(format!("{message}; {CURL_HINT}")),
        AppError::Json(status, _) if status == StatusCode::UNSUPPORTED_MEDIA_TYPE => {
            AppError::Json(
                status,
                serde_json::json!({
                    "error": format!("expected Content-Type: application/json; {CURL_HINT}")
                }),
            )
        }
        error => error,
    }
}

fn replay_now(outbox: &SharedOutbox, replay: Replay) -> std::io::Result<Response> {
    match replay {
        Replay::All => {
            let requeued = outbox.resend_retained()?;
            tracing::info!(requeued, "archive replay of every retained block requested");
            Ok(
                Json(serde_json::json!({ "result": "requeued", "requeued": requeued }))
                    .into_response(),
            )
        }
        Replay::Block(hash) => {
            let outcome = outbox.resend_block(&hash)?;
            tracing::info!(state_hash = %hash, outcome = ?outcome, "archive replay of one block requested");
            let mut body = serde_json::to_value(&outcome).map_err(std::io::Error::other)?;
            let ResendOutcome::NotRetained {
                oldest_retained,
                newest_retained,
                ..
            } = &outcome
            else {
                return Ok(Json(body).into_response());
            };
            let window = match (oldest_retained, newest_retained) {
                (Some(oldest), Some(newest)) => {
                    format!("it keeps heights {} to {}", oldest.height, newest.height)
                }
                _ => "it keeps no delivered blocks now".to_owned(),
            };
            body["error"] = format!("block {hash} is not retained by this node: {window}").into();
            Ok((StatusCode::NOT_FOUND, Json(body)).into_response())
        }
    }
}
