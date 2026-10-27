//! State inspection endpoints.
//!
//! - `GET /state` - Get node state with optional JSONPath filter
//! - `POST /state` - Get node state with JSONPath filter in body
//! - `GET /state/peers` - Get connected peers
//! - `GET /state/message-progress` - Get message progress
//!
//! The JSONPath filter implementation is in `impl mina_node::rpc_effectful::RpcService for NodeService`.

use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use utoipa_axum::{router::OpenApiRouter, routes};

use mina_node::rpc::{RpcMessageProgressResponse, RpcPeersGetResponse, RpcRequest};
use mina_node_common::rpc::RpcStateGetResponse;

use crate::http_server::{AppError, AppResult, AppState, JsonErrorResponse};

/// State routes
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(state_get, state_post))
        .routes(routes!(peers))
        .routes(routes!(message_progress))
}

#[derive(Deserialize, Default)]
struct StateQueryParams {
    /// Optional JSONPath filter expression.
    filter: Option<String>,
}

/// Node state with optional JSONPath filter (query param)
#[utoipa::path(
    get,
    path = "/state",
    tag = "state",
    params(
        ("filter" = Option<String>, Query, description = "JSONPath filter expression")
    ),
    responses(
        (status = 200, description = "Node state"),
        (status = 400, description = "Invalid filter expression", body = JsonErrorResponse,
            example = json!({"error": "failed to parse filter expression: unexpected token"}))
    )
)]
async fn state_get(
    State(state): State<AppState>,
    Query(params): Query<StateQueryParams>,
) -> AppResult<Json<serde_json::Value>> {
    state_handler(state, params.filter).await
}

/// Node state with JSONPath filter in body
#[utoipa::path(
    post,
    path = "/state",
    tag = "state",
    responses(
        (status = 200, description = "Node state"),
        (status = 400, description = "Invalid filter expression", body = JsonErrorResponse,
            example = json!({"error": "failed to parse filter expression: unexpected token"}))
    )
)]
async fn state_post(
    State(state): State<AppState>,
    Json(params): Json<StateQueryParams>,
) -> AppResult<Json<serde_json::Value>> {
    state_handler(state, params.filter).await
}

/// Shared handler for state requests
async fn state_handler(
    state: AppState,
    filter: Option<String>,
) -> AppResult<Json<serde_json::Value>> {
    let result: Option<RpcStateGetResponse> = state
        .rpc_sender()
        .oneshot_request(RpcRequest::StateGet(filter))
        .await;

    match result {
        None => Err(AppError::ChannelDropped),
        Some(Ok(value)) => Ok(Json(value)),
        Some(Err(err)) => Err(AppError::BadRequest(err.to_string())),
    }
}

/// Connected peers
#[utoipa::path(
    get,
    path = "/state/peers",
    tag = "state",
    responses(
        // inline: RpcPeersGetResponse is a type alias for Vec<T>, which would register as "Vec"
        (status = 200, description = "Connected peers", body = inline(RpcPeersGetResponse))
    )
)]
async fn peers(State(state): State<AppState>) -> AppResult<Json<RpcPeersGetResponse>> {
    jsonify_rpc!(state, RpcRequest::PeersGet)
}

/// Message progress
#[utoipa::path(
    get,
    path = "/state/message-progress",
    tag = "state",
    responses(
        (status = 200, description = "Message progress information", body = RpcMessageProgressResponse)
    )
)]
async fn message_progress(
    State(state): State<AppState>,
) -> AppResult<Json<RpcMessageProgressResponse>> {
    jsonify_rpc!(state, RpcRequest::MessageProgressGet)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, http::StatusCode, response::IntoResponse};
    use mina_node::{core::channels::oneshot, rpc::RpcStateGetError};
    use mina_node_common::rpc::RpcService;

    #[tokio::test]
    async fn state_filter_errors_are_http_400_for_get_and_post() {
        let mut rpc = RpcService::new();
        let state = AppState::new(rpc.req_sender());
        for post in [false, true] {
            let filter = "$.transition_frontier.best_chain[0,-1].block";
            let request = async {
                let params = StateQueryParams {
                    filter: Some(filter.to_owned()),
                };
                if post {
                    state_post(State(state.clone()), Json(params)).await
                } else {
                    state_get(State(state.clone()), Query(params)).await
                }
            };
            // The common crate tests the actual evaluator. Here exercise the
            // channel and HTTP mapping without running a networked node.
            let respond = async {
                let request = rpc.req_receiver().recv().await.unwrap();
                assert!(matches!(request.req, RpcRequest::StateGet(Some(f)) if f == filter));
                request
                    .responder
                    .downcast::<oneshot::Sender<RpcStateGetResponse>>()
                    .unwrap()
                    .send(Err(RpcStateGetError::FilterError(
                        "unsupported filter operation".to_owned(),
                    )))
                    .unwrap();
            };
            let (response, ()) = tokio::join!(request, respond);
            let response = response.into_response();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                body["error"],
                "failed to parse filter expression: unsupported filter operation"
            );
        }
    }
}
