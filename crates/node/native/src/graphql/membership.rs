//! LN2: Merkle paths over GraphQL, as OCaml serves them: `Account { index
//! leafHash merklePath }` against the best tip's staged ledger, and
//! `encodedSnarkedLedgerAccountMembership` against a block's snarked ledger.
//! Both read through the light account RPC, on the ledger thread.

use std::{collections::HashMap, sync::Arc};

use dataloader::non_cached::Loader;
use juniper::{FieldError, GraphQLInputObject, GraphQLObject};
use ledger::AccountId;
use mina_node::rpc::{
    LightLedgerKind, LightMerklePathElement, RpcLightAccount, RpcLightAccountEntry,
    RpcLightAccountQuery, RpcLightAccountResponse, RpcLightError, RpcRequest,
    MAX_LIGHT_ACCOUNTS_PER_REQUEST,
};
use mina_node_common::rpc::RpcSender;
use mina_p2p_messages::{binprot::BinProtWrite, v2::StateHash};

use super::{scalars::FieldElemScalar, Context, PublicKeyScalar, TokenIdScalar};
use crate::{
    http_server::routes::light::{admitted, light_rpc},
    input_limits::{self as limits, bounded},
};

/// The most accounts one `encodedSnarkedLedgerAccountMembership` reads, in
/// one ledger call. OCaml has no limit at the pinned commit
/// (`resolve_membership`, mina_graphql.ml#L2422-L2462).
pub const MAX_MEMBERSHIP_ACCOUNTS: usize = MAX_LIGHT_ACCOUNTS_PER_REQUEST;

/// Light account reads the `Account` path fields of one GraphQL request
/// batch into one ledger call.
pub(crate) type LightPathLoader =
    Loader<AccountId, Result<Arc<RpcLightAccount>, RpcLightError>, LightPathBatcher>;

pub(crate) struct LightPathBatcher {
    rpc_sender: RpcSender,
}

impl dataloader::BatchFn<AccountId, Result<Arc<RpcLightAccount>, RpcLightError>>
    for LightPathBatcher
{
    async fn load(
        &mut self,
        keys: &[AccountId],
    ) -> HashMap<AccountId, Result<Arc<RpcLightAccount>, RpcLightError>> {
        let response = light_accounts(
            &self.rpc_sender,
            LightLedgerKind::Staged,
            keys.to_vec(),
            None,
        )
        .await;
        match response {
            Ok(entries) => keys
                .iter()
                .cloned()
                .zip(entries.into_iter().map(|entry| entry.map(Arc::from)))
                .collect(),
            Err(error) => keys
                .iter()
                .map(|key| (key.clone(), Err(error.clone())))
                .collect(),
        }
    }
}

/// One loader per GraphQL request: at most [`MAX_LIGHT_ACCOUNTS_PER_REQUEST`]
/// accounts per ledger call.
pub(crate) fn create_light_path_loader(rpc_sender: RpcSender) -> LightPathLoader {
    Loader::new(LightPathBatcher { rpc_sender })
        .with_max_batch_size(MAX_LIGHT_ACCOUNTS_PER_REQUEST)
        .with_yield_count(25)
}

/// Accounts and their paths in one ledger, through the light request limit
/// `/light/*` uses (refused as busy beyond it) and its deadline: one RPC,
/// one ledger call, one snapshot.
pub(crate) async fn light_accounts(
    rpc_sender: &RpcSender,
    ledger: LightLedgerKind,
    account_ids: Vec<AccountId>,
    block: Option<StateHash>,
) -> Result<Vec<RpcLightAccountEntry>, RpcLightError> {
    let request = RpcRequest::LightAccount(RpcLightAccountQuery {
        ledger,
        account_ids,
        block,
    });
    let response: RpcLightAccountResponse =
        admitted(light_rpc(rpc_sender.clone(), request)).await?;
    response
}

/// One element of a Merkle path: `left` set means the path node is a left
/// child and `left` is the hash of its sibling, `right` the reverse.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L208-L223>
#[derive(GraphQLObject, Debug, Clone)]
#[graphql(name = "MerklePathElement")]
pub struct GraphQLMerklePathElement {
    pub left: Option<FieldElemScalar>,
    pub right: Option<FieldElemScalar>,
}

impl From<&LightMerklePathElement> for GraphQLMerklePathElement {
    fn from(value: &LightMerklePathElement) -> Self {
        match value {
            LightMerklePathElement::Left(hash) => Self {
                left: Some(FieldElemScalar(hash.to_decimal())),
                right: None,
            },
            LightMerklePathElement::Right(hash) => Self {
                left: None,
                right: Some(FieldElemScalar(hash.to_decimal())),
            },
        }
    }
}

/// An account with a public key and a token id
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L2582-L2595>
#[derive(GraphQLInputObject, Debug)]
#[graphql(name = "AccountInput")]
pub struct AccountInput {
    /// Public key of the account
    pub public_key: PublicKeyScalar,
    /// Token id of the account
    pub token: Option<TokenIdScalar>,
}

/// An account, as binprot in base64, with its path in a snarked ledger.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L857-L880>
#[derive(GraphQLObject, Debug)]
#[graphql(name = "EncodedAccount")]
pub struct GraphQLEncodedAccount {
    /// Base64 encoded account as binable wire type
    pub account: String,
    /// Membership proof in the snarked ledger
    pub merkle_path: Vec<GraphQLMerklePathElement>,
}

impl Context {
    /// An account's path in the best tip's staged ledger, batched with the
    /// other `Account` path fields of this request.
    pub(crate) async fn load_light_path(
        &self,
        account_id: AccountId,
    ) -> Result<Arc<RpcLightAccount>, RpcLightError> {
        self.light_path_loader
            .try_load(account_id)
            .await
            .map_err(|error| RpcLightError::Internal(error.to_string()))?
    }
}

/// `encodedSnarkedLedgerAccountMembership`: each account, in binprot, with
/// its path in the snarked ledger of the block `state_hash`, a best-chain
/// block here (OCaml takes any frontier block). At most
/// [`MAX_MEMBERSHIP_ACCOUNTS`], read in one ledger call under the light
/// request limit. Fails as a whole, with OCaml's messages, if a ledger or an
/// account is missing.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L2422-L2500>
pub(crate) async fn encoded_snarked_ledger_account_membership(
    context: &Context,
    account_infos: Vec<AccountInput>,
    state_hash: String,
) -> juniper::FieldResult<Vec<GraphQLEncodedAccount>> {
    if account_infos.len() > MAX_MEMBERSHIP_ACCOUNTS {
        return Err(FieldError::from(format!(
            "at most {MAX_MEMBERSHIP_ACCOUNTS} accountInfos per request"
        )));
    }
    let state_hash = bounded(&state_hash, limits::STATE_HASH)
        .ok()
        .and_then(|hash| hash.parse::<StateHash>().ok())
        .ok_or_else(|| FieldError::from(format!("Invalid state hash: {state_hash}")))?;
    let mut account_ids = Vec::with_capacity(account_infos.len());
    for info in account_infos {
        let public_key =
            super::public_key_argument(&info.public_key, "encodedSnarkedLedgerAccountMembership")?;
        let token = match &info.token {
            None => Default::default(),
            Some(token) => {
                super::token_id_argument(token, "token", "encodedSnarkedLedgerAccountMembership")?
            }
        };
        account_ids.push(AccountId {
            public_key: public_key
                .try_into()
                .map_err(|_| FieldError::from("Invalid public key"))?,
            token_id: ledger::TokenId::try_from(token)
                .map_err(|_| FieldError::from("Invalid token id"))?,
        });
    }
    if account_ids.is_empty() {
        return Ok(Vec::new());
    }
    // One ledger call for all of them, under the light request limit.
    let entries = light_accounts(
        &context.rpc_sender,
        LightLedgerKind::Snarked,
        account_ids,
        Some(state_hash),
    )
    .await
    .map_err(|error| match error {
        RpcLightError::Busy | RpcLightError::Timeout => FieldError::from(error.to_string()),
        error => FieldError::from(format!("Failed to get snarked ledger: {error}")),
    })?;
    let mut memberships = Vec::with_capacity(entries.len());
    for entry in entries {
        let light = entry.map_err(|error| match error {
            RpcLightError::AccountNotFound(_) => {
                FieldError::from("Account not found in snarked ledger")
            }
            error => FieldError::from(format!("Failed to get snarked ledger: {error}")),
        })?;
        let mut account = Vec::new();
        light
            .account
            .binprot_write(&mut account)
            .map_err(|error| FieldError::from(error.to_string()))?;
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        memberships.push(GraphQLEncodedAccount {
            account: STANDARD.encode(account),
            merkle_path: light.merkle_path.iter().map(Into::into).collect(),
        });
    }
    Ok(memberships)
}
