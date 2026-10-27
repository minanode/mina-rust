//! Transaction endpoints.
//!
//! - `GET /transaction-pool` - Get transaction pool
//! - `GET /accounts` - Get all accounts
//! - `POST /send-payment` - Send a payment transaction
//! - `GET /best-chain-user-commands` - Get user commands from best chain

use axum::{extract::State, Json};
use utoipa_axum::{router::OpenApiRouter, routes};

use mina_node::rpc::{
    RpcLedgerSlimAccountsResponse, RpcTransactionInjectResponse, RpcTransactionPoolResponse,
    RpcTransitionFrontierUserCommandsResponse,
};

use crate::http_server::{types::TX_SUBMISSION_DISABLED, AppError, AppResult, AppState};

/// Transaction routes
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(transaction_pool))
        .routes(routes!(best_chain_user_commands))
        .routes(routes!(accounts))
        .routes(routes!(send_payment))
}

/// Transaction pool
#[utoipa::path(
    get,
    path = "/transaction-pool",
    tag = "transaction",
    responses(
        (status = 200, description = "Transaction pool")
    )
)]
async fn transaction_pool(
    State(state): State<AppState>,
) -> AppResult<Json<RpcTransactionPoolResponse>> {
    state
        .rpc_sender()
        .transaction_pool()
        .get()
        .await
        .map(Json)
        .ok_or(AppError::ChannelDropped)
}

/// All accounts from latest ledger
#[utoipa::path(
    get,
    path = "/accounts",
    tag = "transaction",
    responses(
        (status = 200, description = "All accounts")
    )
)]
async fn accounts(State(state): State<AppState>) -> AppResult<Json<RpcLedgerSlimAccountsResponse>> {
    state
        .rpc_sender()
        .ledger()
        .latest()
        .accounts()
        .all()
        .await
        .map(Json)
        .ok_or(AppError::ChannelDropped)
}

/// Send payment transactions
#[utoipa::path(
    post,
    path = "/send-payment",
    tag = "transaction",
    responses(
        (status = 200, description = "Payment result"),
        (status = 403, description = "Transaction submission is off on this node (start it with --enable-tx-submission)")
    )
)]
async fn send_payment(
    State(state): State<AppState>,
    Json(payments): Json<Vec<crate::input_limits::Payment>>,
) -> AppResult<Json<RpcTransactionInjectResponse>> {
    if !state.tx_submission() {
        return Err(AppError::Forbidden(TX_SUBMISSION_DISABLED.to_owned()));
    }
    match state
        .rpc_sender()
        .transaction_pool()
        .inject()
        .payment(payments.into_iter().map(|p| p.0).collect())
        .await
    {
        Err(err) => Err(AppError::Internal(err)),
        Ok(None) => Err(AppError::ChannelDropped),
        Ok(Some(resp)) => Ok(Json(resp)),
    }
}

/// User commands from best chain
#[utoipa::path(
    get,
    path = "/best-chain-user-commands",
    tag = "transaction",
    responses(
        (status = 200, description = "User commands from best chain")
    )
)]
async fn best_chain_user_commands(
    State(state): State<AppState>,
) -> AppResult<Json<RpcTransitionFrontierUserCommandsResponse>> {
    state
        .rpc_sender()
        .transition_frontier()
        .best_chain()
        .user_commands()
        .await
        .map(Json)
        .ok_or(AppError::ChannelDropped)
}
