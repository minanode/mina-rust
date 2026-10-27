//! Axum-based HTTP server for the Mina node RPC API.
//!
//! This module provides REST endpoints for node status, state inspection,
//! snark pool management, and transaction handling.

#[macro_use]
mod macros;
pub mod control_origin;
#[cfg(feature = "dashboard")]
pub mod dashboard;
pub mod flight;
pub mod metrics;
mod openapi;
pub(crate) mod routes;
#[cfg(feature = "p2p-webrtc")]
pub mod signaling;
mod types;

pub use routes::openapi_spec;
pub use types::{
    AppError, AppResult, AppState, JsonErrorResponse, MAX_STATE_HASH_CHARS, TX_SUBMISSION_DISABLED,
};

use mina_node_common::rpc::RpcSender;
use std::net::IpAddr;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use types::cors_layer;

#[cfg(feature = "swagger-ui")]
use utoipa_swagger_ui::SwaggerUi;

#[cfg(feature = "scalar")]
use utoipa_scalar::{Scalar, Servable};

/// What the HTTP server allows.
#[derive(Clone, Debug, Default)]
pub struct HttpServerOptions {
    /// Accept transactions: `POST /send-payment` and the GraphQL mutations
    /// (`sendPayment`, `sendDelegation`, `sendZkapp`). Off by default, so a
    /// relay doesn't take submissions from anyone who can reach its port.
    pub tx_submission: bool,
    /// Answer only the read-only GraphQL subset of
    /// `routes/graphql_public.rs`, for a node whose GraphQL port anyone can
    /// reach, plus health probes and cached public status. All other HTTP routes
    /// (including submission, state inspection and API UIs) are absent.
    pub public_graphql: bool,
    /// Allow whole-ledger exports on the private operator HTTP router.
    /// Off by default: enable only when access to the HTTP port is restricted.
    pub ledger_export: bool,
    /// Allow `POST /snarker/control`, which starts, stops and re-prices the
    /// SNARK worker (P5.4). Off by default, and never with `public_graphql`:
    /// it changes where fees go.
    pub worker_control: bool,
    /// Origins besides this machine's whose pages may change the worker
    /// (`--worker-control-origin`, [`control_origin`]).
    pub worker_control_origins: control_origin::ControlOrigins,
    /// Origins besides this machine's whose pages may read the API
    /// (`--http-cors-origin`, [`control_origin::ReadOrigins`]).
    pub read_origins: control_origin::ReadOrigins,
    /// Serve the node dashboard at `/` (F-console; off with
    /// `--no-dashboard`). Only with the `dashboard` feature, and never with
    /// `public_graphql`.
    pub dashboard: bool,
    /// The node's work directory, for `POST /report` (its start record,
    /// crash files and logs). `None`: the node has no report.
    pub work_dir: Option<std::path::PathBuf>,
}

/// Runs the HTTP server on the specified port.
///
/// Returns an error if binding to the port fails or the server encounters an I/O error.
pub async fn run(
    addr: IpAddr,
    port: u16,
    rpc_sender: RpcSender,
    options: HttpServerOptions,
) -> std::io::Result<()> {
    let listener = bind_http_listener(addr, port).await?;
    run_on_listener(listener, rpc_sender, options).await
}

/// Serve an already-bound listener. The CLI reserves its operator and
/// signaling sockets before spawning either server, so bind failures stop
/// startup instead of leaving a node without its operator HTTP service.
pub async fn run_on_listener(
    listener: TcpListener,
    rpc_sender: RpcSender,
    options: HttpServerOptions,
) -> std::io::Result<()> {
    let app = router(rpc_sender, options.clone());
    let address = listener.local_addr()?;
    let (addr, port) = (address.ip(), address.port());
    let dashboard_address = format!("http://{address}");
    tracing::info!(
        %dashboard_address,
        tx_submission = options.tx_submission && !options.public_graphql,
        public_graphql = options.public_graphql,
        ledger_export = options.ledger_export && !options.public_graphql,
        worker_control = options.worker_control && !options.public_graphql,
        dashboard = dashboard_state(&options),
        "HTTP server listening"
    );
    if addr.is_loopback() {
        tracing::info!("{}", loopback_dashboard_hint(addr, port));
    }
    if let Some(warning) = public_worker_control_warning(addr, &options) {
        tracing::warn!("{warning}");
    }
    if !serves_cdn_pages(&options) && !CDN_PAGES.is_empty() {
        tracing::info!(
            pages = CDN_PAGES.join(", "),
            "worker control is on, so pages that load scripts from CDNs are not served: \
             a script on this node's own address could change the SNARK worker; \
             start without --allow-worker-control to have them"
        );
    }
    if serves_any_cdn_page(&options) {
        tracing::info!(
            pages = CDN_PAGES.join(", "),
            "this build serves pages that load scripts from CDNs, so the bug report \
             (POST /report) is off: a script on this node's own address could read it; \
             start with --allow-worker-control to turn those pages off and have it"
        );
    }
    axum::serve(listener, app).await
}

async fn bind_http_listener(addr: IpAddr, port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind((addr, port)).await
}

fn loopback_dashboard_hint(addr: IpAddr, port: u16) -> String {
    let host = match addr {
        IpAddr::V4(_) => addr.to_string(),
        IpAddr::V6(_) => format!("[{addr}]"),
    };
    format!(
        "on a server, view it with: ssh -N -L {port}:{host}:{port} you@server, \
         or start with --http-addr 0.0.0.0 on a private network"
    )
}

fn public_worker_control_warning(
    addr: IpAddr,
    options: &HttpServerOptions,
) -> Option<&'static str> {
    (options.worker_control && !addr.is_loopback()).then_some(
        "--allow-worker-control is enabled on a non-loopback HTTP address; the port must be private",
    )
}

/// The URI as the request log shows it at info level. A signalling GET's
/// path is the visitor's WebRTC offer, their ICE candidates' addresses
/// among them (W1): it is logged at debug only, and shown here without it.
fn logged_uri(uri: &axum::http::Uri) -> String {
    if uri.path().starts_with(SIGNAL_OFFER_PATH) {
        format!("{SIGNAL_OFFER_PATH}<offer>")
    } else {
        uri.to_string()
    }
}

const SIGNAL_OFFER_PATH: &str = "/mina/webrtc/signal/";

/// Where the OpenAPI spec of the HTTP API is served, in every private
/// router.
pub const OPENAPI_JSON_PATH: &str = "/api-docs/openapi.json";

/// The pages that load scripts from CDNs: GraphiQL (esm.sh), Scalar
/// (jsDelivr) and Stoplight Elements (unpkg). On the node's own origin such
/// a script could change the SNARK worker (F-console fix round), so the
/// default build has none of them: their features (`graphiql`, `scalar`,
/// `stoplight-elements`) are for developer builds, and even there they are
/// not served while worker control is on. Here, the ones this build has.
pub const CDN_PAGES: &[&str] = &[
    #[cfg(feature = "graphiql")]
    "/graphiql",
    #[cfg(feature = "scalar")]
    "/api-docs/scalar",
    #[cfg(feature = "stoplight-elements")]
    "/api-docs/stoplight",
];

/// Whether the node serves [`CDN_PAGES`]: not while the worker can be
/// changed over HTTP. A read-only node has nothing for such a script to
/// change.
fn serves_cdn_pages(options: &HttpServerOptions) -> bool {
    !(options.worker_control && !options.public_graphql)
}

/// Whether the node serves a page that loads scripts from CDNs: this build
/// has one ([`CDN_PAGES`]) and [`serves_cdn_pages`]. Never in the default
/// build, which has none.
fn serves_any_cdn_page(options: &HttpServerOptions) -> bool {
    !CDN_PAGES.is_empty() && !options.public_graphql && serves_cdn_pages(options)
}

/// Whether the node serves its dashboard: compiled in, not turned off with
/// `--no-dashboard`, and not a public node.
fn serves_dashboard(options: &HttpServerOptions) -> bool {
    cfg!(feature = "dashboard") && options.dashboard && !options.public_graphql
}

/// Whether `/` serves the dashboard, for the start-up log.
fn dashboard_state(options: &HttpServerOptions) -> &'static str {
    if !options.dashboard || options.public_graphql {
        return "off";
    }
    #[cfg(feature = "dashboard")]
    {
        if dashboard::built() {
            "on"
        } else {
            "not built (run make dashboard)"
        }
    }
    #[cfg(not(feature = "dashboard"))]
    {
        "not compiled in"
    }
}

/// Every route the HTTP server serves.
pub fn router(rpc_sender: RpcSender, options: HttpServerOptions) -> axum::Router {
    let cdn_pages = serves_cdn_pages(&options);
    let state = AppState::new(rpc_sender)
        .with_tx_submission(options.tx_submission && !options.public_graphql)
        .with_public_graphql(options.public_graphql)
        .with_ledger_export(options.ledger_export && !options.public_graphql)
        .with_worker_control(options.worker_control && !options.public_graphql)
        // The node's own page at an IP address may change the worker only
        // when that page is the dashboard (F-console fix round).
        .with_control_origins(
            options
                .worker_control_origins
                .clone()
                .with_own_pages(serves_dashboard(&options)),
        )
        // The bug report and a page loading CDN scripts exclude each other,
        // as worker control and such pages do: that script would run on the
        // node's own origin, which the report trusts.
        .with_report(
            options.work_dir.clone(),
            serves_any_cdn_page(&options).then_some(routes::report::REFUSED_WITH_CDN_PAGES),
        );

    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(|request: &axum::http::Request<_>| {
            if request.uri().path().starts_with(SIGNAL_OFFER_PATH) {
                tracing::debug!(uri = %request.uri(), "WebRTC signalling request");
            }
            let uri = logged_uri(request.uri());
            tracing::info_span!("http_request", method = %request.method(), uri = %uri)
        })
        .on_response(
            |response: &axum::http::Response<_>, latency, span: &tracing::Span| {
                let status = response.status();
                // A dashboard polls several routes a second: only failures
                // reach the default level (O1).
                if status.is_server_error() {
                    tracing::error!(parent: span, status = %status, latency = ?latency, "request failed");
                } else if status.is_client_error() {
                    tracing::debug!(parent: span, status = %status, latency = ?latency, "request refused");
                } else {
                    tracing::debug!(parent: span, status = %status, latency = ?latency, "request completed");
                }
            },
        );

    let app = if options.public_graphql {
        routes::public_router().split_for_parts().0
    } else {
        let (app, _api) = routes::openapi_router().split_for_parts();

        // Archive controls are private even while the outbox is active.
        let mut app = app
            .route("/archive/status", axum::routing::get(archive_status))
            .route("/archive/resend", axum::routing::post(archive_resend));

        #[cfg(feature = "memory-debug")]
        if std::env::var_os("MINA_MEMORY_DEBUG").as_deref() == Some(std::ffi::OsStr::new("1")) {
            app = app.route(
                "/debug/memory",
                axum::routing::get(routes::status::memory_debug),
            );
        }

        // The OpenAPI spec is data, not a page: always served. Swagger UI, a
        // developer feature, serves it along with its page.
        #[cfg(feature = "swagger-ui")]
        let app =
            app.merge(SwaggerUi::new("/api-docs/swagger-ui").url(OPENAPI_JSON_PATH, _api.clone()));
        #[cfg(not(feature = "swagger-ui"))]
        let app = app.route(OPENAPI_JSON_PATH, openapi::spec_route(&_api));
        #[cfg(feature = "scalar")]
        let app = if cdn_pages {
            app.merge(Scalar::with_url("/api-docs/scalar", _api))
        } else {
            app
        };
        #[cfg(feature = "stoplight-elements")]
        let app = if cdn_pages {
            app.route(
                "/api-docs/stoplight",
                axum::routing::get(openapi::stoplight_elements),
            )
        } else {
            app
        };
        app
    };
    let app = routes::graphql::routes(
        app,
        !options.public_graphql && cdn_pages,
        options.public_graphql,
    );

    // Last, as the fallback: every route above wins over it.
    #[cfg(feature = "dashboard")]
    let app = if serves_dashboard(&options) {
        dashboard::routes(app)
    } else {
        app
    };

    // The public API answers every page; the operator's, only listed ones.
    let read_origins = if options.public_graphql {
        control_origin::ReadOrigins::any()
    } else {
        options.read_origins
    };
    let control_origins = options.worker_control_origins;
    let host_check = {
        let (read_origins, control_origins) = (read_origins.clone(), control_origins.clone());
        axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let refused = refused_host(&request, &read_origins, &control_origins);
                async move {
                    match refused {
                        Some(response) => response,
                        None => next.run(request).await,
                    }
                }
            },
        )
    };

    app.layer(host_check)
        .layer(trace_layer)
        .layer(cors_layer(control_origins, read_origins))
        .with_state(state)
}

/// A browser's request (one with `Origin` or `Sec-Fetch-Site`) whose `Host`
/// is a name nobody listed is refused ([`control_origin::ReadOrigins::allows_host`]).
/// Programs send neither header and are not refused. WebRTC signalling is
/// for every site, and worker control and the report check origins
/// themselves.
fn refused_host(
    request: &axum::extract::Request,
    read_origins: &control_origin::ReadOrigins,
    control_origins: &control_origin::ControlOrigins,
) -> Option<axum::response::Response> {
    use axum::response::IntoResponse;
    let headers = request.headers();
    if !headers.contains_key(axum::http::header::ORIGIN) && !headers.contains_key("sec-fetch-site")
    {
        return None;
    }
    let path = request.uri().path();
    if types::is_signaling_path(path)
        || path == control_origin::WORKER_CONTROL_PATH
        || path == routes::report::REPORT_PATH
        || path.starts_with(&format!("{}/", routes::report::REPORT_PATH))
    {
        return None;
    }
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|host| host.to_str().ok())
        .unwrap_or_default();
    if read_origins.allows_host(host, control_origins) {
        return None;
    }
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|origin| origin.to_str().ok());
    let message = host_refusal(host, origin);
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::warn!("{message}");
    }
    Some(types::AppError::Forbidden(message).into_response())
}

/// Why a page opened at `host` is refused, with the flag that lets it in:
/// the page's own origin when `Origin` names the same host, else
/// `http://` + `host`.
fn host_refusal(host: &str, origin: Option<&str>) -> String {
    let lowered = host.trim().to_ascii_lowercase();
    let origin = origin
        .filter(|origin| {
            origin
                .to_ascii_lowercase()
                .split_once("://")
                .is_some_and(|(_, rest)| rest == lowered)
        })
        .map(str::to_owned)
        .unwrap_or_else(|| format!("http://{}", host.trim()));
    format!(
        "a page opened at {host} may not use this node's API: start the node with \
         --http-cors-origin {origin}, or open the page at the node's IP address or localhost"
    )
}

/// `GET /archive/status`: the archive outbox's pending entries, delivery
/// failures and history gaps; 404 when the node doesn't feed an archive
/// process.
async fn archive_status() -> axum::response::Response {
    use axum::response::IntoResponse;
    match mina_node_common::archive::delivery::status() {
        Some(status) => axum::Json(status).into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": "this node does not feed an archive process (--archive-archiver-process)"
            })),
        )
            .into_response(),
    }
}

/// Requeue retained diffs without rebuilding ledger/archive data.
async fn archive_resend() -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse, Json};
    // Disk moves/fsyncs must not block the async runtime.
    match tokio::task::spawn_blocking(mina_node_common::archive::delivery::resend_retained).await {
        Ok(Some(Ok(requeued))) => Json(serde_json::json!({ "requeued": requeued })).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            "this node does not feed an archive process",
        )
            .into_response(),
        Ok(Some(Err(error))) => {
            (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
        }
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod logged_uri_tests {
    use super::logged_uri;

    #[test]
    fn signalling_offers_are_not_logged_at_info() {
        let uri = "/mina/webrtc/signal/3vQB7B6MrGQZaxCuFg4oh".parse().unwrap();
        assert_eq!(logged_uri(&uri), "/mina/webrtc/signal/<offer>");
        // The POST form carries its offer in the body.
        let uri = "/mina/webrtc/signal".parse().unwrap();
        assert_eq!(logged_uri(&uri), "/mina/webrtc/signal");
        let uri = "/stats/actions?id=latest".parse().unwrap();
        assert_eq!(logged_uri(&uri), "/stats/actions?id=latest");
    }
}
