//! Route handlers for the axum HTTP server.
//!
//! Each submodule groups related endpoints by functionality.

use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

use super::{openapi, AppState};

pub mod discovery;
pub mod graphql;
mod graphql_body;
mod graphql_limits;
mod graphql_ocaml;
mod graphql_public;
pub mod ledger;
pub mod light;
pub mod report;
pub mod scan_state;
pub mod snark_pool;
pub mod snarker;
pub mod state;
pub mod stats;
pub mod status;
pub mod transaction;

#[cfg(feature = "p2p-webrtc")]
pub mod webrtc;

/// Builds the OpenAPI router with all documented routes.
///
/// This is used by both the HTTP server and for generating the OpenAPI spec.
pub fn openapi_router() -> OpenApiRouter<AppState> {
    let router = OpenApiRouter::with_openapi(openapi::ApiDoc::openapi())
        .merge(status::routes())
        .merge(state::routes())
        .merge(stats::routes())
        .merge(scan_state::routes())
        .merge(snark_pool::routes())
        .merge(snarker::routes())
        .merge(transaction::routes())
        .merge(discovery::routes())
        // LN2 light-client reads: on the operator router, so they follow the
        // HTTP bind address (127.0.0.1 unless the operator opts in).
        .merge(light::routes())
        // Operator-only: R34 public_router must never merge these routes.
        .merge(ledger::routes())
        .merge(report::routes());

    #[cfg(feature = "p2p-webrtc")]
    let router = router.merge(webrtc::routes());

    router
}

/// Minimal public surface. GraphQL is registered separately with its policy.
/// Never merge the operator router here: new private routes stay private.
pub(super) fn public_router() -> OpenApiRouter<AppState> {
    status::public_routes()
}

/// Returns the OpenAPI specification for the HTTP API.
///
/// This builds the same spec that the running server uses,
/// without requiring a running server or AppState instance.
pub fn openapi_spec() -> utoipa::openapi::OpenApi {
    let (_router, api) = openapi_router().split_for_parts();
    api
}
