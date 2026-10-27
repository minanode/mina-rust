//! WebRTC signaling endpoints (feature-gated).
//!
//! - `GET /mina/webrtc/signal/{offer}` - Handle WebRTC signaling (GET with base58 encoded offer)
//! - `POST /mina/webrtc/signal` - Handle WebRTC signaling (POST with JSON offer)

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use utoipa_axum::{router::OpenApiRouter, routes};

use mina_node::{
    p2p::{
        connection::{
            incoming::{IncomingSignalingMethod, P2pConnectionIncomingInitOpts},
            P2pConnectionResponse,
        },
        webrtc, PeerId,
    },
    rpc::RpcRequest,
};
use mina_node_common::rpc::RpcP2pConnectionIncomingResponse;

use crate::http_server::{types::AssumeJson, AppState};

/// WebRTC routes
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(signal_get))
        .routes(routes!(signal_post))
}

/// WebRTC signaling (base58 offer in path)
#[utoipa::path(
    get,
    path = "/mina/webrtc/signal/{offer}",
    tag = "webrtc",
    params(
        ("offer" = String, Path, description = "Base58 encoded WebRTC offer")
    ),
    responses(
        (status = 200, description = "Connection accepted or rejected"),
        (status = 400, description = "Bad offer or decryption failed"),
        (status = 500, description = "Internal error")
    )
)]
async fn signal_get(
    State(state): State<AppState>,
    Path(offer): Path<String>,
) -> (StatusCode, Json<P2pConnectionResponse>) {
    // TODO(axum-migration): Returns 400 for both bad base58 AND bad JSON schema inside.
    // This matches warp behavior but differs from signal_post which returns 422 for
    // bad JSON schema. Could split: 400 for bad base58, 422 for bad JSON schema.
    let decode_result = crate::input_limits::bounded(&offer, crate::input_limits::OFFER_BASE58)
        .ok()
        .and_then(|offer| bs58::decode(offer).into_vec().ok())
        .and_then(|json| serde_json::from_slice(&json).ok());

    match decode_result {
        None => (
            StatusCode::BAD_REQUEST,
            Json(P2pConnectionResponse::SignalDecryptionFailed),
        ),
        Some(crate::input_limits::Offer(offer)) => handle_offer(state, offer).await,
    }
}

/// WebRTC signaling (JSON offer in body)
#[utoipa::path(
    post,
    path = "/mina/webrtc/signal",
    tag = "webrtc",
    responses(
        (status = 200, description = "Connection accepted or rejected"),
        (status = 400, description = "Bad offer"),
        (status = 415, description = "Unsupported Content-Type"),
        (status = 422, description = "Malformed JSON"),
        (status = 500, description = "Internal error")
    )
)]
async fn signal_post(
    State(state): State<AppState>,
    AssumeJson(offer): AssumeJson<crate::input_limits::Offer>,
) -> (StatusCode, Json<P2pConnectionResponse>) {
    // TODO(axum-migration): Malformed JSON returns 422 (axum default) vs warp's 400.
    // Both are framework defaults, not explicit choices. 422 is arguably more correct
    // (valid JSON, wrong schema = "unprocessable entity"). Noted for awareness.
    handle_offer(state, offer.0).await
}

/// Shared handler for processing WebRTC offers.
async fn handle_offer(
    state: AppState,
    offer: Box<webrtc::Offer>,
) -> (StatusCode, Json<P2pConnectionResponse>) {
    let mut rx = state
        .rpc_sender()
        .multishot_request(
            2,
            RpcRequest::P2pConnectionIncoming(P2pConnectionIncomingInitOpts {
                peer_id: PeerId::from_public_key(offer.identity_pub_key.clone()),
                signaling: IncomingSignalingMethod::Http,
                offer,
            }),
        )
        .await;

    match rx.recv().await {
        Some(RpcP2pConnectionIncomingResponse::Answer(answer)) => {
            let status = match &answer {
                P2pConnectionResponse::Accepted(_) => StatusCode::OK,
                P2pConnectionResponse::Rejected(reason) => {
                    if reason.is_bad() {
                        StatusCode::BAD_REQUEST
                    } else {
                        StatusCode::OK
                    }
                }
                P2pConnectionResponse::SignalDecryptionFailed => StatusCode::BAD_REQUEST,
                P2pConnectionResponse::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, Json(answer))
        }
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(P2pConnectionResponse::InternalError),
        ),
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use crate::input_limits::{self as limits, tests::offer_fixture};
    use mina_node_common::rpc::RpcService;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn signaling_caps_keep_the_existing_error_envelopes() {
        let rpc = RpcService::new();
        let state = AppState::new(rpc.req_sender());
        for len in [limits::OFFER_BASE58 + 1, 65_000] {
            let input = "z".repeat(len);
            let start = Instant::now();
            let (status, Json(response)) = signal_get(State(state.clone()), Path(input)).await;
            let elapsed = start.elapsed();
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(matches!(
                response,
                P2pConnectionResponse::SignalDecryptionFailed
            ));
            assert!(elapsed < Duration::from_millis(1), "{elapsed:?}");
        }
        // The exact GET policy boundary passes the length gate and JSON/offer
        // decoding. Tune insignificant trailing JSON whitespace independently
        // of the JSON contents to hit exactly 8192 base58 characters.
        let mut offer = offer_fixture();
        offer["sdp"] = "v=0\r\n".into();
        let mut bytes = serde_json::to_vec(&offer).unwrap();
        // 5998 bytes with this JSON prefix occupy 8192 base58 digits.
        bytes.resize(5998, b' ');
        let encoded = bs58::encode(&bytes).into_string();
        assert_eq!(encoded.len(), limits::OFFER_BASE58);
        let decoded = bs58::decode(limits::bounded(&encoded, limits::OFFER_BASE58).unwrap())
            .into_vec()
            .unwrap();
        assert!(serde_json::from_slice::<limits::Offer>(&decoded).is_ok());
    }
}
