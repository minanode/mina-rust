//! LN2: read-only endpoints a light client checks the chain with.
//!
//! - `GET /light/best-tip` - the best tip's header (protocol state and
//!   proof), the root's protocol state and the state body hashes linking the
//!   root to the tip; no block bodies
//! - `GET /light/account/{public_key}` - one account with its index, leaf
//!   hash and Merkle path in a named ledger of a best-chain block
//!
//! A client verifies the tip's proof against the network's verification key
//! and pinned genesis state hash, then checks that an account's path folds
//! to the ledger hash at `ledger_hash_field` of a protocol state it
//! verified. No answer claims more than where its hash sits: `trust` says
//! what that is worth for each ledger.
//!
//! Bounds:
//! - **HTTP and GraphQL.** At most [`MAX_IN_FLIGHT`] light requests at a
//!   time, shared with the GraphQL path fields ([`admitted`]). Beyond that
//!   the answer is 503 `busy`, with no queue. Each request gets
//!   [`RPC_TIMEOUT`] and keeps its permit through all its work, encoding
//!   included, even if the client goes away.
//! - **The node.** It keeps at most
//!   [`mina_node::rpc::MAX_PENDING_LIGHT_REQUESTS`] light requests, refuses
//!   more as busy, and answers and drops those the ledger read quota held
//!   back past their deadline. So requests that time out here can't pile up
//!   there.
//! - **Work per request.** One ledger call per request, for at most
//!   [`mina_node::rpc::MAX_LIGHT_ACCOUNTS_PER_REQUEST`] accounts. The best
//!   tip is encoded once per tip and root, single-flight, off the async
//!   runtime.
//! - **Answer size.** At most a few KB per account and about 32 KB per tip.

use std::{sync::Arc, time::Duration};

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use mina_node::{
    account::AccountPublicKey,
    core::block::ArcBlockWithHash,
    rpc::{
        LightLedgerKind, LightMerklePathElement, RpcLightAccount, RpcLightAccountQuery,
        RpcLightAccountResponse, RpcLightBestTipResponse, RpcLightError, RpcRequest,
    },
};
use mina_node_common::rpc::RpcSender;
use mina_p2p_messages::{
    binprot::BinProtWrite,
    list::List,
    v2::{MinaBaseStateBodyHashStableV1, StateHash, TokenIdKeyHash},
};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    http_server::{AppError, AppResult, AppState, JsonErrorResponse},
    input_limits::{self, bounded},
};

/// Light requests served at once, both routes together.
pub const MAX_IN_FLIGHT: usize = 16;
/// How long a light request waits for the state machine and ledger thread.
pub const RPC_TIMEOUT: Duration = Duration::from_secs(10);

pub const HEADER_STATE_HASH: &str = "x-mina-state-hash";
pub const BINPROT_CONTENT_TYPE: &str = "application/octet-stream";

/// What `trust` says of a block's staged ledger hash.
pub const TRUST_STAGED: &str = "committed by the block producer; the block proof does not check \
     it. Full nodes reject a block with a wrong one, so trusting it means trusting the \
     honest majority that built on this block.";
/// What `trust` says of a block's snarked ledger hash.
pub const TRUST_SNARKED: &str = "the target of the block's ledger proof statement, which the \
     blockchain SNARK is meant to check (LN1 section 1). Open review items may qualify that, \
     so this API claims no more than where the hash sits. It lags the tip by the scan state's \
     depth, about 2.5 days at low load.";
/// What `trust` says of a block's epoch ledger hashes.
pub const TRUST_EPOCH: &str = "in the block's consensus state; the blockchain SNARK derives it \
     from an earlier snarked ledger at an epoch boundary (LN1 section 1). Open review items \
     may qualify that, so this API claims no more than where the hash sits.";
/// What `trust` says of the best tip answer.
pub const TRUST_BEST_TIP: &str = "verify `header` (its proof, against the network's \
     verification key, and its genesis_state_hash, against the pinned one) and take every \
     value from it. The other fields are convenience copies. One node's best tip is not \
     proof that it is the best chain: compare tips from several nodes with consensus_take.";

pub fn trust(kind: LightLedgerKind) -> &'static str {
    match kind {
        LightLedgerKind::Staged => TRUST_STAGED,
        LightLedgerKind::Snarked => TRUST_SNARKED,
        LightLedgerKind::Staking | LightLedgerKind::Next => TRUST_EPOCH,
    }
}

static PERMITS: std::sync::LazyLock<Arc<Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(Semaphore::new(MAX_IN_FLIGHT)));

/// The encoded answer for the current best tip and root, recomputed when
/// either moves. Held across the encoding, so there is one encoding per tip:
/// the others wait for it and reuse it.
static BEST_TIP_CACHE: tokio::sync::Mutex<Option<Arc<EncodedBestTip>>> =
    tokio::sync::Mutex::const_new(None);

/// Best-tip encodings run so far, for tests.
pub(crate) static BEST_TIP_ENCODINGS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Light client routes
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(best_tip))
        .routes(routes!(account))
}

/// A light error as JSON, with a stable `code` for clients.
fn light_error(error: RpcLightError) -> AppError {
    let (status, code) = match &error {
        RpcLightError::NotSynced => (StatusCode::SERVICE_UNAVAILABLE, "not_synced"),
        RpcLightError::BlockNotInBestChain(_) => (StatusCode::NOT_FOUND, "block_not_in_best_chain"),
        RpcLightError::LedgerUnavailable { .. } => {
            (StatusCode::SERVICE_UNAVAILABLE, "ledger_unavailable")
        }
        RpcLightError::AccountNotFound(_) => (StatusCode::NOT_FOUND, "account_not_found"),
        RpcLightError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        RpcLightError::Busy => (StatusCode::SERVICE_UNAVAILABLE, "busy"),
        RpcLightError::Timeout => (StatusCode::SERVICE_UNAVAILABLE, "timeout"),
        RpcLightError::TooManyAccounts(_) => (StatusCode::BAD_REQUEST, "too_many_accounts"),
    };
    let mut body = serde_json::json!({ "error": error.to_string(), "code": code });
    if let RpcLightError::LedgerUnavailable {
        kind: LightLedgerKind::Snarked,
        ..
    } = error
    {
        // The node keeps the snarked ledger of its best chain's root, as
        // OCaml does, and others only while they are the root's.
        body["hint"] = SNARKED_HINT.into();
    }
    AppError::Json(status, body)
}

/// Where to find a snarked ledger the node holds.
pub const SNARKED_HINT: &str = "this node holds the snarked ledger of its best chain's root: ask \
     with block=<root.state_hash> from /light/best-tip and check the path against the root's \
     protocol state, which the body hashes link to the tip";

fn bad_request(message: impl Into<String>) -> AppError {
    AppError::Json(
        StatusCode::BAD_REQUEST,
        serde_json::json!({ "error": message.into(), "code": "bad_request" }),
    )
}

/// Runs `work` under one of the [`MAX_IN_FLIGHT`] light permits, shared by
/// `/light/*` and the GraphQL path fields, or refuses at once with
/// [`RpcLightError::Busy`]: there is no queue. The work runs in its own task,
/// which keeps the permit until the work ends, even if the client goes away,
/// so a client that disconnects cannot start more work than the permits. The
/// node bounds its own side too ([`mina_node::rpc::MAX_PENDING_LIGHT_REQUESTS`]),
/// so a request that times out here cannot pile up there.
pub(crate) async fn admitted<T: Send + 'static>(
    work: impl std::future::Future<Output = Result<T, RpcLightError>> + Send + 'static,
) -> Result<T, RpcLightError> {
    let permit = PERMITS
        .clone()
        .try_acquire_owned()
        .map_err(|_| RpcLightError::Busy)?;
    tokio::spawn(async move {
        let result = work.await;
        drop(permit);
        result
    })
    .await
    .map_err(|error| RpcLightError::Internal(format!("light request task failed: {error}")))?
}

/// Sends a light RPC and waits at most [`RPC_TIMEOUT`] for its answer.
pub(crate) async fn light_rpc<T: Send + serde::Serialize + 'static>(
    sender: RpcSender,
    request: RpcRequest,
) -> Result<T, RpcLightError> {
    tokio::time::timeout(RPC_TIMEOUT, sender.oneshot_request::<T>(request))
        .await
        .map_err(|_| RpcLightError::Timeout)?
        .ok_or_else(|| RpcLightError::Internal("the node dropped the request".to_owned()))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct BestTipQuery {
    /// `json` (default) or `binprot`: the binprot of `(tip header, root
    /// protocol state, body hashes)`, as OCaml types
    /// `Mina_block.Header.t * Mina_state.Protocol_state.Value.t *
    /// State_body_hash.t list`.
    pub format: Option<String>,
}

/// The best tip, as a light client verifies it
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LightBestTip {
    pub state_hash: String,
    /// The genesis state hash the tip carries; a client compares it with
    /// the one it pins.
    pub genesis_state_hash: String,
    pub height: u32,
    pub global_slot_since_genesis: u32,
    pub ledger_hashes: LightLedgerHashes,
    /// Base64 of the tip header's binprot (`MinaBlockHeaderStableV2`):
    /// protocol state, proof, delta chain proof and protocol versions.
    pub header: String,
    pub root: LightRoot,
    /// Base64 of the binprot list of state body hashes from the root's child
    /// to the tip: folding `state_hash = H(previous, body)` from the root
    /// reaches the tip.
    pub body_hashes: String,
    pub body_hash_count: usize,
    pub trust: String,
}

/// The tip's ledger hashes, copied from its protocol state.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LightLedgerHashes {
    pub staged: String,
    pub snarked: String,
    pub staking: String,
    pub next: String,
}

/// The root of the best chain (the oldest block the node keeps).
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LightRoot {
    pub state_hash: String,
    pub height: u32,
    /// Base64 of the root's protocol state binprot.
    pub protocol_state: String,
}

struct EncodedBestTip {
    tip: StateHash,
    root: StateHash,
    json: Bytes,
    binprot: Bytes,
}

fn binprot<T: BinProtWrite>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    value
        .binprot_write(&mut bytes)
        .expect("writing to a Vec cannot fail");
    bytes
}

/// Hashes the bodies and encodes both forms: about 290 Poseidon hashes, off
/// the async runtime.
fn encode_best_tip(chain: &[ArcBlockWithHash]) -> Result<EncodedBestTip, String> {
    BEST_TIP_ENCODINGS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let (root, tip) = match chain {
        [root, .., tip] => (root, tip),
        [only] => (only, only),
        [] => return Err("empty best chain".to_owned()),
    };
    let body_hashes = chain[1..]
        .iter()
        .map(|block| {
            block
                .header()
                .protocol_state
                .body
                .try_hash()
                .map_err(|error| format!("hashing block {}: {error:?}", block.hash()))
        })
        .collect::<Result<List<_>, _>>()?;
    let header = binprot(tip.header());
    let root_state = binprot(&root.header().protocol_state);
    let body_bytes = binprot(&body_hashes);
    let mut bin = Vec::with_capacity(header.len() + root_state.len() + body_bytes.len());
    bin.extend_from_slice(&header);
    bin.extend_from_slice(&root_state);
    bin.extend_from_slice(&body_bytes);
    let json = LightBestTip {
        state_hash: tip.hash().to_string(),
        genesis_state_hash: tip
            .header()
            .protocol_state
            .body
            .genesis_state_hash
            .to_string(),
        height: tip.height(),
        global_slot_since_genesis: tip.global_slot_since_genesis(),
        ledger_hashes: LightLedgerHashes {
            staged: tip.merkle_root_hash().to_string(),
            snarked: tip.snarked_ledger_hash().to_string(),
            staking: tip.staking_epoch_ledger_hash().to_string(),
            next: tip.next_epoch_ledger_hash().to_string(),
        },
        header: STANDARD.encode(&header),
        root: LightRoot {
            state_hash: root.hash().to_string(),
            height: root.height(),
            protocol_state: STANDARD.encode(&root_state),
        },
        body_hashes: STANDARD.encode(&body_bytes),
        body_hash_count: body_hashes.len(),
        trust: TRUST_BEST_TIP.to_owned(),
    };
    Ok(EncodedBestTip {
        tip: tip.hash().clone(),
        root: root.hash().clone(),
        json: serde_json::to_vec(&json)
            .map_err(|error| error.to_string())?
            .into(),
        binprot: bin.into(),
    })
}

/// The best tip without block bodies
///
/// What a light client needs to verify the best tip: the tip's header (its
/// protocol state and proof), the state hash, the genesis state hash the
/// tip carries, and the root's protocol state with the body hashes linking it
/// to the tip. A client runs `consensus_take` against another node's tip
/// with the consensus states and state hashes of the two headers. Answers
/// while the node is not bootstrapping, block catch-up and root moves
/// included.
#[utoipa::path(
    get,
    path = "/light/best-tip",
    tag = "light",
    params(BestTipQuery),
    responses(
        (status = 200, description = "JSON (default), or binprot with format=binprot; \
            x-mina-state-hash header", body = LightBestTip),
        (status = 400, description = "Unknown format", body = JsonErrorResponse),
        (status = 503, description = "not_synced, busy or timeout", body = JsonErrorResponse)
    )
)]
async fn best_tip(
    State(state): State<AppState>,
    Query(query): Query<BestTipQuery>,
) -> AppResult<Response> {
    let binprot = match query.format.as_deref() {
        None | Some("json") => false,
        Some("binprot") => true,
        Some(other) => {
            return Err(bad_request(format!(
                "unknown format '{}', expected json or binprot",
                other.chars().take(16).collect::<String>()
            )))
        }
    };
    let sender = state.rpc_sender().clone();
    // The RPC and any encoding run under one permit.
    let encoded = admitted(async move {
        let chain: RpcLightBestTipResponse = light_rpc(sender, RpcRequest::LightBestTip).await?;
        encoded_best_tip(chain?).await
    })
    .await
    .map_err(light_error)?;
    let (content_type, body) = if binprot {
        (BINPROT_CONTENT_TYPE, encoded.binprot.clone())
    } else {
        ("application/json", encoded.json.clone())
    };
    let headers = [
        (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
        (
            HeaderName::from_static(HEADER_STATE_HASH),
            HeaderValue::from_str(&encoded.tip.to_string()).expect("base58 is ASCII"),
        ),
    ];
    Ok((headers, body).into_response())
}

/// The encoded answer for `chain`: the cached one if it is for the same tip
/// and root, else one encoding (off the async runtime) that concurrent
/// callers wait for.
async fn encoded_best_tip(
    chain: Vec<ArcBlockWithHash>,
) -> Result<Arc<EncodedBestTip>, RpcLightError> {
    let (Some(root), Some(tip)) = (
        chain.first().map(|block| block.hash().clone()),
        chain.last().map(|block| block.hash().clone()),
    ) else {
        return Err(RpcLightError::NotSynced);
    };
    let mut cached = BEST_TIP_CACHE.lock().await;
    if let Some(encoded) = cached
        .as_ref()
        .filter(|encoded| encoded.tip == tip && encoded.root == root)
    {
        return Ok(encoded.clone());
    }
    let encoded = tokio::task::spawn_blocking(move || encode_best_tip(&chain))
        .await
        .map_err(|error| RpcLightError::Internal(format!("encoding task failed: {error}")))?
        .map(Arc::new)
        .map_err(RpcLightError::Internal)?;
    *cached = Some(encoded.clone());
    Ok(encoded)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct AccountQueryParams {
    /// staged, snarked, staking or next
    pub ledger: Option<String>,
    /// Token id; MINA if absent
    pub token: Option<String>,
    /// A best-chain block by state hash; the best tip if absent
    pub block: Option<String>,
}

/// An account and the Merkle path tying it to a block's ledger hash
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LightAccount {
    /// staged, snarked, staking or next
    pub ledger: String,
    /// The root `merkle_path` folds to.
    pub ledger_hash: String,
    /// Where `ledger_hash` sits in the block's protocol state.
    pub ledger_hash_field: String,
    /// What a client can rely on for this ledger.
    pub trust: String,
    /// The block whose ledger was read.
    pub block: LightBlock,
    pub public_key: String,
    pub token_id: String,
    /// The account's index in the ledger (its leaf, left to right).
    pub index: u64,
    /// The account's hash, the path's leaf, as a decimal field element.
    pub leaf_hash: String,
    /// Leaf first, 35 elements, in OCaml GraphQL's `MerklePathElement`
    /// shape: `left` set means the node is a left child and `left` is its
    /// right sibling's hash, `right` the reverse.
    pub merkle_path: Vec<LightPathElement>,
    /// Base64 of the account's binprot (`MinaBaseAccountBinableArgStableV2`):
    /// hash it to get `leaf_hash`.
    pub account: String,
    /// Convenience copies of account fields; read them from `account`.
    pub summary: LightAccountSummary,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LightBlock {
    pub state_hash: String,
    pub height: u32,
    pub global_slot_since_genesis: u32,
}

/// One Merkle path element, as OCaml's GraphQL `MerklePathElement`.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema, PartialEq)]
pub struct LightPathElement {
    pub left: Option<String>,
    pub right: Option<String>,
}

impl From<&LightMerklePathElement> for LightPathElement {
    fn from(value: &LightMerklePathElement) -> Self {
        match value {
            LightMerklePathElement::Left(hash) => Self {
                left: Some(hash.to_decimal()),
                right: None,
            },
            LightMerklePathElement::Right(hash) => Self {
                left: None,
                right: Some(hash.to_decimal()),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LightAccountSummary {
    /// Nanomina
    pub balance: String,
    pub nonce: u32,
    pub delegate: Option<String>,
    pub has_zkapp: bool,
}

fn account_json(account: &RpcLightAccount) -> AppResult<LightAccount> {
    let full = ledger::Account::try_from(&account.account)
        .map_err(|error| AppError::Internal(format!("decoding account: {error:?}")))?;
    Ok(LightAccount {
        ledger: account.ledger.as_str().to_owned(),
        ledger_hash: account.ledger_hash.to_string(),
        ledger_hash_field: format!("protocol_state.{}", account.ledger.protocol_state_field()),
        trust: trust(account.ledger).to_owned(),
        block: LightBlock {
            state_hash: account.block.state_hash.to_string(),
            height: account.block.height,
            global_slot_since_genesis: account.block.global_slot_since_genesis,
        },
        public_key: full.public_key.into_address(),
        token_id: TokenIdKeyHash::from(full.token_id.clone()).to_string(),
        index: account.index,
        leaf_hash: account.leaf_hash.to_decimal(),
        merkle_path: account.merkle_path.iter().map(Into::into).collect(),
        account: STANDARD.encode(binprot(&account.account)),
        summary: LightAccountSummary {
            balance: full.balance.as_u64().to_string(),
            nonce: full.nonce.as_u32(),
            delegate: full.delegate.as_ref().map(|key| key.into_address()),
            has_zkapp: full.zkapp.is_some(),
        },
    })
}

/// Parses a light account request, refusing long input before decoding it.
pub fn light_account_query(
    public_key: &str,
    params: &AccountQueryParams,
) -> Result<RpcLightAccountQuery, String> {
    let ledger: LightLedgerKind = params
        .ledger
        .as_deref()
        .ok_or("give the ledger: ledger=staged|snarked|staking|next")?
        .parse()?;
    let public_key = bounded(public_key, input_limits::PUBLIC_KEY)
        .ok()
        .and_then(|key| key.parse::<AccountPublicKey>().ok())
        .and_then(|key| mina_signer::CompressedPubKey::try_from(key).ok())
        .ok_or("invalid public key")?;
    let token_id = match params.token.as_deref() {
        None => ledger::TokenId::default(),
        Some(token) => bounded(token, input_limits::TOKEN_ID)
            .ok()
            .and_then(|token| token.parse::<TokenIdKeyHash>().ok())
            .and_then(|token| ledger::TokenId::try_from(token).ok())
            .ok_or("invalid token id")?,
    };
    let block = params
        .block
        .as_deref()
        .map(|hash| {
            bounded(hash, input_limits::STATE_HASH)
                .ok()
                .and_then(|hash| hash.parse::<StateHash>().ok())
                .ok_or("invalid block state hash")
        })
        .transpose()?;
    Ok(RpcLightAccountQuery {
        ledger,
        account_ids: vec![ledger::AccountId::create(public_key, token_id)],
        block,
    })
}

/// One account with its Merkle path
///
/// The account, its index, leaf hash and Merkle path in the named ledger of
/// a best-chain block (the best tip unless `block` is given), with the ledger
/// hash the path folds to and the block it was read from. `ledger_hash_field`
/// says where that hash sits in the block's protocol state, and `trust` what
/// a client can rely on. A Merkle path cannot show that an account is
/// absent: `account_not_found` is the node's word only.
#[utoipa::path(
    get,
    path = "/light/account/{public_key}",
    tag = "light",
    params(
        ("public_key" = String, Path, description = "B62... public key"),
        AccountQueryParams
    ),
    responses(
        (status = 200, body = LightAccount),
        (status = 400, description = "bad_request: bad key, token, block or ledger",
            body = JsonErrorResponse),
        (status = 404, description = "account_not_found or block_not_in_best_chain",
            body = JsonErrorResponse),
        (status = 503, description = "not_synced, ledger_unavailable, busy or timeout",
            body = JsonErrorResponse)
    )
)]
async fn account(
    State(state): State<AppState>,
    Path(public_key): Path<String>,
    Query(params): Query<AccountQueryParams>,
) -> AppResult<Json<LightAccount>> {
    let query = light_account_query(&public_key, &params).map_err(bad_request)?;
    let sender = state.rpc_sender().clone();
    let response: RpcLightAccountResponse =
        admitted(light_rpc(sender, RpcRequest::LightAccount(query)))
            .await
            .map_err(light_error)?;
    let account = response
        .map_err(light_error)?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Internal("the node answered no account".to_owned()))?
        .map_err(light_error)?;
    account_json(&account).map(Json)
}

#[cfg(test)]
mod tests {
    use ledger::{BaseLedger, TreeVersion};
    use mina_node::{
        core::{
            block::BlockWithHash,
            channels::{mpsc, oneshot},
        },
        rpc::{LightBlockRef, RpcLightAccount},
    };
    use mina_node_common::rpc::{NodeRpcRequest, RpcSender};
    use mina_p2p_messages::{
        bigint::BigInt,
        binprot::BinProtRead,
        rpc::GetBestTipV2Response,
        rpc_kernel::ResponsePayload,
        v2::{
            LedgerHash, MinaBaseAccountBinableArgStableV2, MinaBlockHeaderStableV2,
            MinaStateProtocolStateValueStableV2,
        },
    };

    use super::*;

    const KEY: &str = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";

    fn node() -> (AppState, mpsc::Receiver<NodeRpcRequest>) {
        let (tx, rx) = mpsc::channel::<NodeRpcRequest>(16);
        (AppState::new(RpcSender::new(tx)), rx)
    }

    fn params(
        ledger: Option<&str>,
        token: Option<&str>,
        block: Option<&str>,
    ) -> AccountQueryParams {
        AccountQueryParams {
            ledger: ledger.map(Into::into),
            token: token.map(Into::into),
            block: block.map(Into::into),
        }
    }

    /// Answers the next RPC with `response`, returning the request.
    async fn answer<T: Send + 'static>(
        rx: &mut mpsc::Receiver<NodeRpcRequest>,
        response: T,
    ) -> RpcRequest {
        let request = rx.recv().await.unwrap();
        request
            .responder
            .downcast::<oneshot::Sender<T>>()
            .unwrap()
            .send(response)
            .ok()
            .unwrap();
        request.req
    }

    async fn status_and_body(error: AppError) -> (StatusCode, serde_json::Value) {
        let response = error.into_response();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    fn fixture() -> (
        MinaBlockBlockStableV2Pair,
        Vec<MinaBaseStateBodyHashStableV1>,
    ) {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin"
        );
        let bytes = std::fs::read(path).unwrap();
        let payload =
            ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut &bytes[..]).unwrap();
        let proof = payload.0.unwrap().0.unwrap();
        let tip = BlockWithHash::try_new(Arc::new(proof.data)).unwrap();
        let root = BlockWithHash::try_new(Arc::new(proof.proof.1)).unwrap();
        ((root, tip), proof.proof.0.into_iter().collect())
    }

    type MinaBlockBlockStableV2Pair = (ArcBlockWithHash, ArcBlockWithHash);

    #[tokio::test]
    async fn bad_requests_reach_no_rpc() {
        let _guard = super::super::ledger::EXPORT_TEST_LOCK.lock().await;
        let long = format!("B62{}", "q".repeat(10_000));
        for (key, params) in [
            ("nope", params(Some("staged"), None, None)),
            (long.as_str(), params(Some("staged"), None, None)),
            (KEY, params(None, None, None)),
            (KEY, params(Some("root"), None, None)),
            (KEY, params(Some("staged"), Some("not-a-token"), None)),
            (KEY, params(Some("staged"), Some(&long), None)),
            (KEY, params(Some("staged"), None, Some("3Nbad"))),
            (KEY, params(Some("staged"), None, Some(&long))),
        ] {
            let (state, mut rx) = node();
            let error = account(State(state), Path(key.to_owned()), Query(params))
                .await
                .unwrap_err();
            let (status, body) = status_and_body(error).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body["code"], "bad_request");
            assert!(rx.try_recv().is_err(), "a bad request reached the node");
        }
        let (state, mut rx) = node();
        let error = best_tip(
            State(state),
            Query(BestTipQuery {
                format: Some("xml".into()),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(status_and_body(error).await.0, StatusCode::BAD_REQUEST);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn light_errors_have_statuses_and_codes() {
        let _guard = super::super::ledger::EXPORT_TEST_LOCK.lock().await;
        for (error, status, code) in [
            (
                RpcLightError::NotSynced,
                StatusCode::SERVICE_UNAVAILABLE,
                "not_synced",
            ),
            (
                RpcLightError::AccountNotFound(LightLedgerKind::Staking),
                StatusCode::NOT_FOUND,
                "account_not_found",
            ),
            (
                RpcLightError::BlockNotInBestChain(StateHash::zero()),
                StatusCode::NOT_FOUND,
                "block_not_in_best_chain",
            ),
            (
                RpcLightError::LedgerUnavailable {
                    kind: LightLedgerKind::Next,
                    hash: LedgerHash::zero(),
                },
                StatusCode::SERVICE_UNAVAILABLE,
                "ledger_unavailable",
            ),
            // The node's own bounds.
            (RpcLightError::Busy, StatusCode::SERVICE_UNAVAILABLE, "busy"),
            (
                RpcLightError::Timeout,
                StatusCode::SERVICE_UNAVAILABLE,
                "timeout",
            ),
        ] {
            let (state, mut rx) = node();
            let request = account(
                State(state),
                Path(KEY.to_owned()),
                Query(params(Some("staking"), None, None)),
            );
            let response: RpcLightAccountResponse = Err(error);
            let (result, req) = tokio::join!(request, answer(&mut rx, response));
            let RpcRequest::LightAccount(query) = req else {
                panic!("{req:?}");
            };
            assert_eq!(query.account_ids.len(), 1);
            assert_eq!(query.ledger, LightLedgerKind::Staking);
            assert_eq!(query.block, None);
            let (got, body) = status_and_body(result.unwrap_err()).await;
            assert_eq!((got, body["code"].as_str().unwrap()), (status, code));
            assert!(body.get("hint").is_none());
        }
        // A missing snarked ledger says where a held one is.
        let (got, body) = status_and_body(light_error(RpcLightError::LedgerUnavailable {
            kind: LightLedgerKind::Snarked,
            hash: LedgerHash::zero(),
        }))
        .await;
        assert_eq!(got, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body["hint"]
            .as_str()
            .unwrap()
            .contains("block=<root.state_hash>"));
        // The best tip refuses the same way while the node bootstraps.
        let (state, mut rx) = node();
        let request = best_tip(State(state), Query(BestTipQuery { format: None }));
        let response: RpcLightBestTipResponse = Err(RpcLightError::NotSynced);
        let (result, _) = tokio::join!(request, answer(&mut rx, response));
        let (status, body) = status_and_body(result.unwrap_err()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "not_synced");
    }

    /// An answer built from an in-memory ledger: its JSON path, read the way
    /// a client reads it, folds to the ledger hash.
    #[tokio::test]
    async fn account_json_folds_to_its_ledger_hash() {
        let _guard = super::super::ledger::EXPORT_TEST_LOCK.lock().await;
        let key = mina_signer::CompressedPubKey::from_address(KEY).unwrap();
        let mut mask = ledger::Mask::create(35);
        for n in 0..6u64 {
            let account = ledger::Account::rand();
            mask.get_or_create_account(account.id(), account).unwrap();
            if n == 2 {
                let mut account = ledger::Account::create_with(
                    ledger::AccountId::new_with_default_token(key.clone()),
                    ledger::scan_state::currency::Balance::from_u64(123_456),
                );
                account.nonce = ledger::scan_state::currency::Nonce::from_u32(4);
                mask.get_or_create_account(account.id(), account).unwrap();
            }
        }
        let ledger_hash = LedgerHash::from_fp(mask.merkle_root());
        let address = mask
            .location_of_account(&ledger::AccountId::new_with_default_token(key))
            .unwrap();
        let held = mask.get(address.clone()).unwrap();
        let light = RpcLightAccount {
            ledger: LightLedgerKind::Snarked,
            ledger_hash: ledger_hash.clone(),
            block: LightBlockRef {
                state_hash: StateHash::zero(),
                height: 12,
                global_slot_since_genesis: 34,
            },
            index: address.to_index().as_u64(),
            leaf_hash: (&held.hash()).into(),
            account: (&*held).into(),
            merkle_path: mask
                .merkle_path(address)
                .iter()
                .map(LightMerklePathElement::from)
                .collect(),
        };
        let (state, mut rx) = node();
        let request = account(
            State(state),
            Path(KEY.to_owned()),
            Query(params(Some("snarked"), None, None)),
        );
        let response: RpcLightAccountResponse = Ok(vec![Ok(Box::new(light))]);
        let (result, _) = tokio::join!(request, answer(&mut rx, response));
        let json = serde_json::to_value(result.unwrap().0).unwrap();

        assert_eq!(json["ledger"], "snarked");
        assert_eq!(json["ledger_hash"], ledger_hash.to_string());
        assert_eq!(
            json["ledger_hash_field"],
            "protocol_state.body.blockchain_state.ledger_proof_statement.target.first_pass_ledger"
        );
        assert_eq!(json["block"]["height"], 12);
        assert_eq!(json["public_key"], KEY);
        assert_eq!(json["index"], 3);
        assert_eq!(json["summary"]["balance"], "123456");
        assert_eq!(json["summary"]["nonce"], 4);
        assert!(!json.to_string().to_lowercase().contains("proven"));

        // The client's check: hash the account, fold the path.
        let bytes = STANDARD.decode(json["account"].as_str().unwrap()).unwrap();
        let wire = MinaBaseAccountBinableArgStableV2::binprot_read(&mut &bytes[..]).unwrap();
        let leaf = ledger::Account::try_from(&wire).unwrap().hash();
        assert_eq!(
            BigInt::from(&leaf).to_decimal(),
            json["leaf_hash"].as_str().unwrap()
        );
        let index = json["index"].as_u64().unwrap();
        let path = json["merkle_path"].as_array().unwrap();
        assert_eq!(path.len(), 35);
        let root = path
            .iter()
            .enumerate()
            .fold(leaf, |node, (height, element)| {
                let is_right = (index >> height) & 1 == 1;
                let sibling = if is_right {
                    assert!(element["left"].is_null());
                    element["right"].as_str().unwrap()
                } else {
                    assert!(element["right"].is_null());
                    element["left"].as_str().unwrap()
                };
                let sibling = BigInt::from_decimal(sibling).unwrap().to_field().unwrap();
                let (l, r) = if is_right {
                    (sibling, node)
                } else {
                    (node, sibling)
                };
                <ledger::V2 as TreeVersion>::hash_node(height, l, r)
            });
        assert_eq!(LedgerHash::from_fp(root), ledger_hash);
    }

    /// The tip's body hash, as the server computes it, is the one OCaml's
    /// `get_best_tip` proof lists last; both encodings carry the same parts.
    #[tokio::test]
    async fn best_tip_encodes_header_root_and_ocamls_body_hashes() {
        let _guard = super::super::ledger::EXPORT_TEST_LOCK.lock().await;
        let ((root, tip), ocaml_body_hashes) = fixture();
        let chain = vec![root.clone(), tip.clone()];
        for format in [None, Some("binprot")] {
            let (state, mut rx) = node();
            let request = best_tip(
                State(state),
                Query(BestTipQuery {
                    format: format.map(Into::into),
                }),
            );
            let response: RpcLightBestTipResponse = Ok(chain.clone());
            let (result, req) = tokio::join!(request, answer(&mut rx, response));
            assert!(matches!(req, RpcRequest::LightBestTip));
            let response = result.unwrap();
            assert_eq!(
                response.headers()[HEADER_STATE_HASH],
                tip.hash().to_string().as_str()
            );
            let body = axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap();
            let (header, root_state, body_hashes) = if format.is_none() {
                let json: LightBestTip = serde_json::from_slice(&body).unwrap();
                assert_eq!(json.state_hash, tip.hash().to_string());
                assert_eq!(json.root.state_hash, root.hash().to_string());
                assert_eq!(
                    json.ledger_hashes.snarked,
                    tip.snarked_ledger_hash().to_string()
                );
                assert_eq!(json.body_hash_count, 1);
                let decode = |text: &str| STANDARD.decode(text).unwrap();
                (
                    decode(&json.header),
                    decode(&json.root.protocol_state),
                    decode(&json.body_hashes),
                )
            } else {
                let mut rest = &body[..];
                let header = MinaBlockHeaderStableV2::binprot_read(&mut rest).unwrap();
                let root_state =
                    MinaStateProtocolStateValueStableV2::binprot_read(&mut rest).unwrap();
                let body_hashes =
                    List::<MinaBaseStateBodyHashStableV1>::binprot_read(&mut rest).unwrap();
                assert!(rest.is_empty());
                (
                    binprot(&header),
                    binprot(&root_state),
                    binprot(&body_hashes),
                )
            };
            let header = MinaBlockHeaderStableV2::binprot_read(&mut &header[..]).unwrap();
            assert_eq!(&header.try_hash().unwrap(), tip.hash());
            let root_state =
                MinaStateProtocolStateValueStableV2::binprot_read(&mut &root_state[..]).unwrap();
            assert_eq!(&root_state.try_hash().unwrap(), root.hash());
            let body_hashes =
                List::<MinaBaseStateBodyHashStableV1>::binprot_read(&mut &body_hashes[..]).unwrap();
            assert_eq!(
                body_hashes.iter().collect::<Vec<_>>(),
                vec![ocaml_body_hashes.last().unwrap()]
            );
        }
    }

    /// A burst on a cold cache (a new tip) encodes once: every request holds
    /// its permit through the encoding, and waits for the one encoding
    /// rather than starting its own.
    #[tokio::test]
    async fn a_burst_on_a_new_tip_encodes_once() {
        let _guard = super::super::ledger::EXPORT_TEST_LOCK.lock().await;
        *BEST_TIP_CACHE.lock().await = None;
        let ((root, tip), _) = fixture();
        let chain = vec![root, tip.clone()];
        let (state, mut rx) = node();
        let before = BEST_TIP_ENCODINGS.load(std::sync::atomic::Ordering::SeqCst);
        let burst = MAX_IN_FLIGHT;
        let requests: Vec<_> = (0..burst)
            .map(|_| {
                let state = state.clone();
                tokio::spawn(async move {
                    best_tip(State(state), Query(BestTipQuery { format: None })).await
                })
            })
            .collect();
        // Every admitted request is in the node before any is answered, so
        // all of them find the cache cold.
        let mut pending = Vec::new();
        for _ in 0..burst {
            pending.push(rx.recv().await.unwrap());
        }
        assert_eq!(PERMITS.available_permits(), 0);
        // One more is refused rather than queued.
        let error = best_tip(State(state.clone()), Query(BestTipQuery { format: None }))
            .await
            .unwrap_err();
        assert_eq!(status_and_body(error).await.1["code"], "busy");
        for request in pending {
            let response: RpcLightBestTipResponse = Ok(chain.clone());
            request
                .responder
                .downcast::<oneshot::Sender<RpcLightBestTipResponse>>()
                .unwrap()
                .send(response)
                .ok()
                .unwrap();
        }
        for request in requests {
            let response = request.await.unwrap().unwrap();
            assert_eq!(
                response.headers()[HEADER_STATE_HASH],
                tip.hash().to_string().as_str()
            );
        }
        assert_eq!(
            BEST_TIP_ENCODINGS.load(std::sync::atomic::Ordering::SeqCst) - before,
            1,
            "one encoding for the burst"
        );
        assert_eq!(PERMITS.available_permits(), MAX_IN_FLIGHT);
    }

    #[tokio::test]
    async fn busy_beyond_the_in_flight_limit() {
        let _guard = super::super::ledger::EXPORT_TEST_LOCK.lock().await;
        let held: Vec<_> = (0..MAX_IN_FLIGHT)
            .map(|_| PERMITS.clone().try_acquire_owned().unwrap())
            .collect();
        let (state, mut rx) = node();
        let error = account(
            State(state.clone()),
            Path(KEY.to_owned()),
            Query(params(Some("staged"), None, None)),
        )
        .await
        .unwrap_err();
        let (status, body) = status_and_body(error).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::SERVICE_UNAVAILABLE, Some("busy"))
        );
        let error = best_tip(State(state), Query(BestTipQuery { format: None }))
            .await
            .unwrap_err();
        assert_eq!(status_and_body(error).await.1["code"], "busy");
        assert!(rx.try_recv().is_err());
        drop(held);
        assert_eq!(PERMITS.available_permits(), MAX_IN_FLIGHT);
    }

    /// A client that goes away does not free its permit before the node
    /// answers, so it cannot get more requests into the node than the limit.
    #[tokio::test]
    async fn a_dropped_request_keeps_its_permit_until_the_node_answers() {
        let _guard = super::super::ledger::EXPORT_TEST_LOCK.lock().await;
        let (state, mut rx) = node();
        let pending = tokio::spawn(account(
            State(state),
            Path(KEY.to_owned()),
            Query(params(Some("staged"), None, None)),
        ));
        let request = rx.recv().await.unwrap();
        pending.abort();
        let _ = pending.await;
        assert_eq!(PERMITS.available_permits(), MAX_IN_FLIGHT - 1);
        drop(request); // The node answers (here: drops the request).
        tokio::time::timeout(Duration::from_secs(3), async {
            while PERMITS.available_permits() < MAX_IN_FLIGHT {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
