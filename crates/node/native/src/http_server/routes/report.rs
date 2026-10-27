//! The node's bug report, for the dashboard's Report page (O3):
//!
//! - `POST /report` - the report `mina report` collects, from the same code
//!   and with the same redaction (`mina-node-report`), with its pre-filled
//!   GitHub issue link
//! - `POST /report/crash` - whether the node crashed lately: when, and the
//!   panic's first line, for the overview's notice
//!
//! The report holds the node's logs and crash text: redacted, but private.
//! Both routes are guarded like worker control
//! ([`crate::http_server::control_origin`]): a POST with the
//! [`REPORT_HEADER`] header and a JSON body, which a page of another site
//! can't send without a CORS preflight that the node refuses; and a page's
//! `Origin` must be this machine's, one listed with
//! `--worker-control-origin`, or the node's own dashboard opened at an IP
//! address. No other origin gets CORS headers on these paths
//! ([`crate::http_server::types::cors_layer`]), so no other page can read an
//! answer. They are private routes: never on a `--public-graphql` node.
//!
//! A build that serves pages loading scripts from CDNs (the developer UIs:
//! GraphiQL, Scalar, Stoplight) refuses them while it does: such a script
//! runs on the node's own origin, which these routes trust, as worker
//! control does, which is why those pages are off while it is on.
//!
//! Collecting is bounded ([`crate::http_server::flight`]): one collection
//! at a time, with a deadline, its answer shared by every request for the
//! same report while it runs (up to a few) and kept for a moment; any other
//! request is refused at once, never queued.

use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use utoipa_axum::{router::OpenApiRouter, routes};

use mina_node::rpc::{RpcRequest, RpcSnarkerConfigGetResponse, RpcStatusGetResponse};
use mina_node_report::{
    bundle::NodeInfo,
    crash::{self, CrashNotice},
    issue, StoreCheckMode,
};

use crate::http_server::{
    flight::{Flight, FlightError},
    AppError, AppResult, AppState,
};

/// The header a report request must carry, like
/// [`WORKER_CONTROL_HEADER`](crate::http_server::control_origin::WORKER_CONTROL_HEADER).
pub const REPORT_HEADER: &str = "x-mina-report";

/// The paths this module serves, for the CORS rule.
pub const REPORT_PATH: &str = "/report";

/// A collected report is reused for this long: a page that asks again at
/// once gets the same one, and a page that asks in a loop can't keep the
/// node collecting.
pub const CACHE_FOR: Duration = Duration::from_secs(10);
/// A collection that takes longer is given up: its requests get an error.
pub const DEADLINE: Duration = Duration::from_secs(20);
/// The crash notice is kept, and given up, after less.
pub const CRASH_CACHE_FOR: Duration = Duration::from_secs(5);
pub const CRASH_DEADLINE: Duration = Duration::from_secs(5);

/// Why the routes refuse in a build that serves pages loading CDN scripts.
pub const REFUSED_WITH_CDN_PAGES: &str =
    "the bug report is off while this node serves pages that load scripts from CDNs \
     (the developer UIs GraphiQL, Scalar, Stoplight): those scripts run on the node's own \
     origin, which could then read the report. Start the node with --allow-worker-control, \
     which turns those pages off, or run `mina report` on its machine";

/// The warning, error and panic lines a report includes, as `mina report`'s
/// default.
const LOG_LINES: usize = 100;

/// The longest "what happened" the page may send.
const MAX_DESCRIPTION_CHARS: usize = 2000;

/// Where the report reads from, whether it is refused, and the bounded
/// collections.
pub struct ReportState {
    work_dir: Option<PathBuf>,
    refused: Option<&'static str>,
    reports: Arc<Flight<ReportRequest, ReportAnswer>>,
    crashes: Arc<Flight<(), Option<CrashNotice>>>,
}

impl ReportState {
    /// `work_dir`: the node's work directory (`last-start.json`, crash
    /// files, logs); `None` when the node doesn't say, and then there is no
    /// report. `refused`: why the routes refuse every request, if they do.
    pub fn new(work_dir: Option<PathBuf>, refused: Option<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            work_dir,
            refused,
            reports: Flight::new(CACHE_FOR, DEADLINE),
            crashes: Flight::new(CRASH_CACHE_FOR, CRASH_DEADLINE),
        })
    }
}

/// A flight's refusal as an answer: 503 for busy (try again), 504 past the
/// deadline, 500 for a failure.
fn flight_error(what: &str, error: FlightError) -> AppError {
    match error {
        FlightError::Busy(why) => {
            AppError::ServiceUnavailable(format!("{what}: busy ({why}); try again in a moment"))
        }
        FlightError::Timeout(after) => AppError::Json(
            axum::http::StatusCode::GATEWAY_TIMEOUT,
            serde_json::json!({
                "error": format!("{what} took longer than {} s and was given up", after.as_secs())
            }),
        ),
        FlightError::Failed(why) => AppError::Internal(format!("{what} failed: {why}")),
    }
}

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(report))
        .routes(routes!(crash_notice))
}

/// What the page asks for.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReportRequest {
    /// Keep IP addresses and peer ids (left out by default), as
    /// `mina report --include-network-ids`.
    #[serde(default)]
    pub include_network_ids: bool,
    /// What happened, in the reporter's words.
    #[serde(default)]
    pub description: Option<String>,
}

/// A collected, redacted report.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ReportAnswer {
    /// When it was collected (RFC 3339).
    pub generated_at: String,
    pub title: String,
    /// What makes reports of the same problem match, also in the title.
    pub fingerprint: String,
    /// The report, as the issue's body.
    pub markdown: String,
    pub include_network_ids: bool,
    pub issue: IssueLink,
    /// A name for the downloaded report: `report-<time>.md`.
    pub file_name: String,
}

/// The pre-filled GitHub issue, as `mina report` opens it.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct IssueLink {
    pub repo: String,
    pub url: String,
    /// Whether the whole report is in the link. If not, it was too long for
    /// one ([`issue::MAX_URL_LEN`]): the link has its summary and asks for
    /// the downloaded file to be attached.
    pub complete: bool,
}

/// The node's bug report
///
/// What `mina report` collects, with the same code and redaction: this
/// build, the machine, what the node says about itself, its newest crash
/// file, warnings, errors and panics from the end of its log, and the
/// command line it was started with (redacted when it started). Keys,
/// passwords, key files and their paths, and secret environment variables
/// are never included; IP addresses and peer ids only with
/// `include_network_ids`. The frontier store is not checked here (`mina
/// report` does). With it, the pre-filled GitHub issue link. One report is
/// collected at a time, within 20 s, shared by the requests for it and
/// reused for 10 s; a request for another one meanwhile gets 503.
///
/// Like worker control, the request needs `Content-Type: application/json`
/// and an `X-Mina-Report` header, and a page may send it only from this
/// machine, from the node's own dashboard opened at an IP address, or from
/// an origin given with `--worker-control-origin`.
#[utoipa::path(
    post,
    path = "/report",
    tag = "report",
    request_body = ReportRequest,
    responses(
        (status = 200, description = "The redacted report and its issue link", body = ReportAnswer),
        (status = 403, description = "Not the operator's request, or off while CDN pages are served"),
        (status = 415, description = "Not JSON"),
        (status = 503, description = "Busy, or the node doesn't know its work directory"),
        (status = 504, description = "Collecting took longer than 20 s")
    )
)]
async fn report(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> AppResult<Json<ReportAnswer>> {
    let mut request: ReportRequest =
        super::snarker::operator_json(&state, &headers, &body, REPORT_HEADER, "the bug report")?;
    request.description = request
        .description
        .map(|text| text.trim().chars().take(MAX_DESCRIPTION_CHARS).collect())
        .filter(|text: &String| !text.is_empty());
    let reports = state.report();
    let work_dir = available(&reports)?;
    let options = request.clone();
    let node_state = state.clone();
    reports
        .reports
        .get(request, move |blocking| async move {
            let started = std::time::Instant::now();
            let include_network_ids = options.include_network_ids;
            let node = node_info(&node_state).await;
            let answer = blocking
                .run(move || collect(&work_dir, node, options))
                .await;
            tracing::info!(
                kind = "BugReportCollected",
                took_ms = started.elapsed().as_millis() as u64,
                include_network_ids,
                ok = answer.is_ok(),
            );
            answer
        })
        .await
        .map(Json)
        .map_err(|error| flight_error("collecting the report", error))
}

/// What the overview's crash notice needs.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CrashAnswer {
    /// The newest crash, if the node panicked in this run or in the run just
    /// before it (it restarted after a panic).
    #[schema(value_type = Option<Object>)]
    pub crash: Option<CrashNotice>,
}

/// Whether the node crashed lately
///
/// The newest crash file in the node's work directory, if the node panicked
/// in this run or in the run just before its current start (at most a day
/// before): when, and the first line of the panic message, short and
/// redacted. Guarded like `POST /report`, and bounded the same way: one read
/// at a time, within 5 s, kept for 5 s.
#[utoipa::path(
    post,
    path = "/report/crash",
    tag = "report",
    responses(
        (status = 200, description = "The latest crash, or null", body = CrashAnswer),
        (status = 403, description = "Not the operator's request, or off while CDN pages are served"),
        (status = 415, description = "Not JSON"),
        (status = 503, description = "Busy, or the node doesn't know its work directory"),
        (status = 504, description = "Reading the crash files took longer than 5 s")
    )
)]
async fn crash_notice(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> AppResult<Json<CrashAnswer>> {
    let _: serde_json::Value = super::snarker::operator_json(
        &state,
        &headers,
        if body.is_empty() { b"{}" } else { &body },
        REPORT_HEADER,
        "the crash notice",
    )?;
    let reports = state.report();
    let work_dir = available(&reports)?;
    let crash = reports
        .crashes
        .get((), move |blocking| {
            blocking.run(move || crash::crash_notice(&work_dir, &crash::node_redactor(false)))
        })
        .await
        .map_err(|error| flight_error("reading the crash files", error))?;
    Ok(Json(CrashAnswer { crash }))
}

/// The work directory, if the routes serve: refused in a build that serves
/// CDN pages, unavailable when the node doesn't know its work directory.
fn available(reports: &ReportState) -> AppResult<PathBuf> {
    if let Some(reason) = reports.refused {
        return Err(AppError::Forbidden(reason.to_owned()));
    }
    reports.work_dir.clone().ok_or_else(|| {
        AppError::ServiceUnavailable(
            "this node doesn't know its work directory, so it has no report: run `mina report`"
                .to_owned(),
        )
    })
}

/// What the node says about itself: the answers `mina report` reads from
/// `/status`, `/build_env` and `/snarker/config`, from the same requests.
async fn node_info(state: &AppState) -> Result<NodeInfo, String> {
    let status: RpcStatusGetResponse = rpc_request!(state, RpcRequest::StatusGet)
        .map_err(|_| "the node didn't answer".to_owned())?;
    let status = status.ok_or_else(|| "the node has no status yet".to_owned())?;
    let status = serde_json::to_value(status).map_err(|error| error.to_string())?;
    let build = serde_json::to_value(mina_node::BuildEnv::get()).ok();
    let snarker: Option<RpcSnarkerConfigGetResponse> =
        rpc_request!(state, RpcRequest::SnarkerConfig).ok();
    let snarker = snarker.and_then(|config| serde_json::to_value(config).ok());
    Ok(NodeInfo::from_json(
        &status,
        build.as_ref(),
        snarker.as_ref(),
        crash::now_ms(),
    ))
}

/// Collects and builds the report: reads files, so off the async threads.
fn collect(
    work_dir: &std::path::Path,
    node: Result<NodeInfo, String>,
    request: ReportRequest,
) -> ReportAnswer {
    let mut redactor = crash::node_redactor(request.include_network_ids);
    let inputs = mina_node_report::collect(
        mina_node_report::Sources {
            work_dir,
            node_url: "the node itself, through its dashboard".to_owned(),
            node,
            logs: &[],
            log_lines: LOG_LINES,
            store_check: StoreCheckMode::Off,
            store_off_reason: "the dashboard's report doesn't check it; run `mina report` on the \
                               node's machine for a report that does",
            description: request.description,
            progress: &|_| {},
        },
        &redactor,
    );
    let report = mina_node_report::build(&inputs, &mut redactor, LOG_LINES);
    let stamp: String = inputs
        .generated_at
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let file_name = format!("report-{stamp}.md");
    let place = format!("is in the dashboard's download, `{file_name}`");
    let link = issue::link(issue::DEFAULT_REPO, &report.title, &report.markdown, || {
        (issue::attach_note(&place), report.summary.clone())
    });
    ReportAnswer {
        generated_at: inputs.generated_at,
        title: report.title,
        fingerprint: report.fingerprint,
        markdown: report.markdown,
        include_network_ids: request.include_network_ids,
        issue: IssueLink {
            repo: issue::DEFAULT_REPO.to_owned(),
            complete: matches!(link, issue::Link::Full(_)),
            url: link.url().to_owned(),
        },
        file_name,
    }
}
