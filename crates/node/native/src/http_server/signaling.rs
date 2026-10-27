//! A separate public listener containing only WebRTC signaling routes.

use axum::{
    extract::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use mina_node_common::rpc::RpcSender;

use super::AppState;

// Keep axum's existing 2 MiB POST budget and the handler's field limits.
// Reading the body and waiting for the node's answer share one deadline.
const BODY_LIMIT: usize = 2 * 1024 * 1024;
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub fn router(rpc_sender: RpcSender) -> axum::Router {
    super::routes::webrtc::routes()
        .split_for_parts()
        .0
        .layer(axum::extract::DefaultBodyLimit::max(BODY_LIMIT))
        .layer(axum::middleware::from_fn(deadline))
        .route_layer(super::types::cors_layer(
            Default::default(),
            super::control_origin::ReadOrigins::any(),
        ))
        .with_state(AppState::new(rpc_sender))
}

async fn deadline(request: Request, next: Next) -> Response {
    with_deadline(next.run(request), REQUEST_TIMEOUT).await
}

async fn with_deadline(
    response: impl std::future::Future<Output = Response>,
    timeout: std::time::Duration,
) -> Response {
    tokio::time::timeout(timeout, response)
        .await
        .unwrap_or_else(|_| {
            (
                axum::http::StatusCode::GATEWAY_TIMEOUT,
                axum::Json(mina_node::p2p::connection::P2pConnectionResponse::InternalError),
            )
                .into_response()
        })
}

/// The caller binds before starting the node, so a bind error fails startup.
pub async fn run(listener: tokio::net::TcpListener, rpc_sender: RpcSender) -> std::io::Result<()> {
    let address = listener.local_addr()?;
    tracing::info!(
        signaling_url = %format!("http://{address}/mina/webrtc/signal"),
        "WebRTC signaling listener ready; publish its reachable host and port (HTTPS through a TLS proxy)"
    );
    axum::serve(listener, router(rpc_sender)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn listener_exposes_only_signaling_and_browser_cors() {
        let rpc = mina_node_common::rpc::RpcService::new();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let sender = rpc.req_sender();
        let server = tokio::spawn(run(listener, sender));
        let client = reqwest::Client::new();
        for path in [
            "/",
            "/index.html",
            "/dashboard",
            "/status",
            "/state",
            "/graphql",
            "/api-docs/openapi.json",
            "/snarker/control",
            "/report",
            "/metrics",
        ] {
            for method in [reqwest::Method::GET, reqwest::Method::POST] {
                let response = client
                    .request(method, format!("{base}{path}"))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            }
            let response = client
                .request(reqwest::Method::OPTIONS, format!("{base}{path}"))
                .header("origin", "https://minanode.org")
                .header("access-control-request-method", "POST")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
        let response = client
            .get(format!("{base}/mina/webrtc/signal/invalid"))
            .header("origin", "https://minanode.org")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            "https://minanode.org"
        );
        let response = client
            .post(format!("{base}/mina/webrtc/signal"))
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let response = client
            .request(
                reqwest::Method::OPTIONS,
                format!("{base}/mina/webrtc/signal"),
            )
            .header("origin", "https://minanode.org")
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "content-type")
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            "https://minanode.org"
        );
        let response = client
            .post(format!("{base}/mina/webrtc/signal"))
            .body(" ".repeat(BODY_LIMIT + 1))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        // The original operator route remains available as well.
        let private = super::super::router(rpc.req_sender(), Default::default());
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let url = format!(
            "http://{}/mina/webrtc/signal/invalid",
            listener.local_addr().unwrap()
        );
        let compatibility = tokio::spawn(async move { axum::serve(listener, private).await });
        assert_eq!(
            client.get(url).send().await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        compatibility.abort();
        server.abort();
    }

    #[tokio::test]
    async fn unresponsive_requests_have_a_deadline() {
        let response =
            with_deadline(std::future::pending(), std::time::Duration::from_millis(1)).await;
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    }

    #[tokio::test]
    async fn signaling_preflight_matches_the_operator_http_route() {
        let rpc = mina_node_common::rpc::RpcService::new();
        let mut bases = Vec::new();
        let mut servers = Vec::new();
        for app in [
            router(rpc.req_sender()),
            super::super::router(rpc.req_sender(), Default::default()),
        ] {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            bases.push(format!("http://{}", listener.local_addr().unwrap()));
            servers.push(tokio::spawn(
                async move { axum::serve(listener, app).await },
            ));
        }
        let client = reqwest::Client::new();
        for (path, method) in [
            ("/mina/webrtc/signal", "POST"),
            ("/mina/webrtc/signal/invalid", "GET"),
        ] {
            let mut answers = Vec::new();
            for base in &bases {
                let response = client
                    .request(reqwest::Method::OPTIONS, format!("{base}{path}"))
                    .header("origin", "https://minanode.org")
                    .header("access-control-request-method", method)
                    .header(
                        "access-control-request-headers",
                        "content-type,user-agent,referer,sec-fetch-mode",
                    )
                    .send()
                    .await
                    .unwrap();
                assert!(response.status().is_success());
                let allowed = response.headers()["access-control-allow-headers"]
                    .to_str()
                    .unwrap();
                assert!(allowed.contains("user-agent") && allowed.contains("sec-fetch-mode"));
                let mut cors: Vec<_> = response
                    .headers()
                    .iter()
                    .filter(|(name, _)| {
                        name.as_str().starts_with("access-control-") || name.as_str() == "vary"
                    })
                    .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
                    .collect();
                cors.sort();
                answers.push((response.status(), cors));
            }
            assert_eq!(answers[0], answers[1], "{path}");
        }
        for server in servers {
            server.abort();
        }
    }
}
