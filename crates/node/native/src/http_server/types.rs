//! Shared types for the axum HTTP server.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use tower_http::cors::CorsLayer;

use mina_node_common::rpc::RpcSender;

/// Application state shared across all axum handlers.
#[derive(Clone)]
pub struct AppState {
    rpc_sender: RpcSender,
    tx_submission: bool,
    public_graphql: bool,
    pub(super) public_status:
        std::sync::Arc<tokio::sync::Mutex<super::routes::status::PublicStatusCache>>,
    #[cfg(feature = "memory-debug")]
    pub(super) memory_debug_last: std::sync::Arc<tokio::sync::Mutex<Option<std::time::Instant>>>,
    #[cfg(feature = "memory-debug")]
    pub(super) memory_debug_permit: std::sync::Arc<tokio::sync::Semaphore>,
    ledger_export: bool,
    worker_control: bool,
    control_origins: super::control_origin::ControlOrigins,
    /// The bug report's work directory and its one collection at a time.
    report: std::sync::Arc<super::routes::report::ReportState>,
}

impl AppState {
    /// Creates a new application state with the given RPC sender, with
    /// transaction submission off.
    pub fn new(rpc_sender: RpcSender) -> Self {
        Self {
            rpc_sender,
            tx_submission: false,
            public_graphql: false,
            public_status: Default::default(),
            #[cfg(feature = "memory-debug")]
            memory_debug_last: Default::default(),
            #[cfg(feature = "memory-debug")]
            memory_debug_permit: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            ledger_export: false,
            worker_control: false,
            control_origins: Default::default(),
            report: super::routes::report::ReportState::new(None, None),
        }
    }

    /// The node's work directory, which `POST /report` reads, and why the
    /// report routes refuse, if they do.
    pub fn with_report(
        mut self,
        work_dir: Option<std::path::PathBuf>,
        refused: Option<&'static str>,
    ) -> Self {
        self.report = super::routes::report::ReportState::new(work_dir, refused);
        self
    }

    /// The bug report's state.
    pub fn report(&self) -> std::sync::Arc<super::routes::report::ReportState> {
        self.report.clone()
    }

    /// The origins besides this machine's whose pages may change the worker.
    pub fn with_control_origins(mut self, origins: super::control_origin::ControlOrigins) -> Self {
        self.control_origins = origins;
        self
    }

    pub fn control_origins(&self) -> &super::control_origin::ControlOrigins {
        &self.control_origins
    }

    /// Allows changing the SNARK worker (P5.4). Keep the HTTP port private
    /// when enabled.
    pub fn with_worker_control(mut self, enabled: bool) -> Self {
        self.worker_control = enabled;
        self
    }

    /// Whether `POST /snarker/control` may change the SNARK worker.
    pub fn worker_control(&self) -> bool {
        self.worker_control
    }

    /// Turns transaction submission (`POST /send-payment` and the GraphQL
    /// mutations) on or off.
    pub fn with_tx_submission(mut self, enabled: bool) -> Self {
        self.tx_submission = enabled;
        self
    }

    /// Enables operator ledger exports. Keep the HTTP port private when enabled.
    pub fn with_ledger_export(mut self, enabled: bool) -> Self {
        self.ledger_export = enabled;
        self
    }

    pub fn ledger_export(&self) -> bool {
        self.ledger_export
    }

    /// Returns a reference to the RPC sender.
    pub fn rpc_sender(&self) -> &RpcSender {
        &self.rpc_sender
    }

    /// Whether this node accepts transactions over HTTP and GraphQL.
    pub fn tx_submission(&self) -> bool {
        self.tx_submission
    }

    /// Turns the public GraphQL policy on or off.
    pub fn with_public_graphql(mut self, enabled: bool) -> Self {
        self.public_graphql = enabled;
        self
    }

    /// Whether GraphQL requests go through the public policy.
    pub fn public_graphql(&self) -> bool {
        self.public_graphql
    }
}

/// The message for a submission when submission is off.
pub const TX_SUBMISSION_DISABLED: &str = "transaction submission is disabled on this node";

/// The longest string a route parses as a state hash. A state hash is 52
/// base58 characters; base58 decoding is quadratic in the length, and a
/// 65,000-character string takes about a second on the single-threaded HTTP
/// runtime, so longer strings are rejected before decoding.
pub const MAX_STATE_HASH_CHARS: usize = 64;

/// Result type alias for HTTP handlers.
pub type AppResult<T> = Result<T, AppError>;

/// JSON error response body.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct JsonErrorResponse {
    /// Error message
    pub error: String,
}

impl JsonErrorResponse {
    /// Creates a new error response with the given message.
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
        }
    }
}

/// HTTP API error type that converts to appropriate HTTP responses.
#[derive(Debug, thiserror::Error, utoipa::ToSchema)]
pub enum AppError {
    /// The RPC channel was dropped before a response was received.
    #[error("response channel dropped, see error log for details")]
    ChannelDropped,

    /// An internal server error occurred.
    #[error("{0}")]
    Internal(String),

    /// A bad request was made by the client.
    #[error("{0}")]
    BadRequest(String),

    /// The service is temporarily unavailable (e.g., not ready).
    #[error("{0}")]
    ServiceUnavailable(String),

    /// The request is not allowed on this node (e.g. transaction submission
    /// is off).
    #[error("{0}")]
    Forbidden(String),

    /// JSON error response with custom status code.
    #[error("{0}: {1}")]
    Json(StatusCode, serde_json::Value),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            AppError::ChannelDropped => (
                StatusCode::INTERNAL_SERVER_ERROR,
                JsonErrorResponse::new("response channel dropped, see error log for details"),
            ),
            AppError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                JsonErrorResponse::new(msg),
            ),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, JsonErrorResponse::new(msg)),
            AppError::ServiceUnavailable(msg) => {
                (StatusCode::SERVICE_UNAVAILABLE, JsonErrorResponse::new(msg))
            }
            AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, JsonErrorResponse::new(msg)),
            AppError::Json(status, value) => return (status, Json(value)).into_response(),
        };
        (status, Json(body)).into_response()
    }
}

/// Creates the CORS layer. Pages of `read_origins` may read the API
/// ([`super::control_origin::ReadOrigins`]), and every page may use WebRTC
/// signalling. Only the origins that may change the worker get CORS headers
/// for changing it, so a browser sends other pages' requests to it no further
/// than the preflight (P5.4, [`super::control_origin`]). The bug report
/// (`/report`, `/report/crash`) is the same for every method, reads included:
/// it holds the node's logs, so no other page may read it.
pub fn cors_layer(
    control_origins: super::control_origin::ControlOrigins,
    read_origins: super::control_origin::ReadOrigins,
) -> CorsLayer {
    use axum::http::{header::*, Method};
    use tower_http::cors::AllowOrigin;

    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin, request| {
            let allowed = || {
                origin
                    .to_str()
                    .is_ok_and(|origin| control_origins.allows(origin))
            };
            let path = request.uri.path();
            if path == super::routes::report::REPORT_PATH
                || path.starts_with(&format!("{}/", super::routes::report::REPORT_PATH))
            {
                return allowed();
            }
            if is_signaling_path(path) {
                return true;
            }
            // Reading the worker's state is open to the pages that may read
            // `/status`, so a dashboard elsewhere can show a node whose
            // control is off; only changing it (`POST`, and its preflight) is
            // limited further.
            let readable = || {
                origin
                    .to_str()
                    .is_ok_and(|origin| read_origins.allows(origin, &control_origins))
            };
            if path != super::control_origin::WORKER_CONTROL_PATH || request.method == Method::GET {
                readable()
            } else {
                allowed()
            }
        }))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            USER_AGENT,
            REFERER,
            ORIGIN,
            ACCESS_CONTROL_REQUEST_METHOD,
            ACCESS_CONTROL_REQUEST_HEADERS,
            CONTENT_TYPE,
            HeaderName::from_static("sec-fetch-mode"),
            HeaderName::from_static(super::control_origin::WORKER_CONTROL_HEADER),
            HeaderName::from_static(super::routes::report::REPORT_HEADER),
        ])
}

/// WebRTC signalling, which browser nodes on any site use.
pub fn is_signaling_path(path: &str) -> bool {
    path.strip_prefix("/mina/webrtc/signal")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// JSON extractor that doesn't require Content-Type header.
/// - If Content-Type is present and not application/json → 415
/// - If Content-Type is missing or is application/json → parse as JSON
/// - Parse failure → 422 (axum default)
pub struct AssumeJson<T>(pub T);

impl<S, T> axum::extract::FromRequest<S> for AssumeJson<T>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned,
{
    type Rejection = (StatusCode, String);

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        use axum::{body::Bytes, extract::Json, http::header::CONTENT_TYPE};
        if let Some(content_type) = req.headers().get(CONTENT_TYPE) {
            let ct = content_type.to_str().unwrap_or("");
            if !ct.starts_with("application/json") {
                return Err((
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "expected Content-Type: application/json".into(),
                ));
            }
        }

        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|e| (e.status(), e.body_text()))?;
        Json::from_bytes(&bytes)
            .map(|Json(j)| AssumeJson(j))
            .map_err(|e| (e.status(), e.body_text()))
    }
}
