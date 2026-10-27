use std::{collections::HashMap, sync::Arc};

use dataloader::non_cached::Loader;
use juniper::{graphql_object, FieldResult, GraphQLInputObject, GraphQLObject};
use ledger::{
    scan_state::currency::{Balance, Magnitude, Slot},
    Account, AccountId, FpExt, Timing,
};
use mina_node::{
    account::AccountPublicKey,
    rpc::{
        AccountQuery, DelegatorsQuery, PooledCommandsQuery, RpcLightAccount, RpcLightError,
        RpcPooledUserCommandsResponse, RpcPooledZkappCommandsResponse, RpcRequest,
        MAX_LAST_EPOCH_DELEGATORS,
    },
};
use mina_node_common::rpc::RpcSender;
use mina_p2p_messages::{
    string::{TokenSymbol, ZkAppUri},
    v2::{
        MinaBaseAccountUpdateUpdateTimingInfoStableV1, MinaBaseVerificationKeyWireStableV1,
        ReceiptChainHash, TokenIdKeyHash,
    },
};
use mina_signer::CompressedPubKey;

use super::{
    membership::GraphQLMerklePathElement, scalars::FieldElemScalar, Context, ConversionError,
};

pub(crate) type AccountLoader =
    Loader<AccountId, Result<GraphQLAccount, Arc<ConversionError>>, AccountBatcher>;

pub(crate) struct AccountBatcher {
    rpc_sender: RpcSender,
}

impl dataloader::BatchFn<AccountId, Result<GraphQLAccount, Arc<ConversionError>>>
    for AccountBatcher
{
    async fn load(
        &mut self,
        keys: &[AccountId],
    ) -> HashMap<AccountId, Result<GraphQLAccount, Arc<ConversionError>>> {
        self.rpc_sender
            .oneshot_request::<Vec<Account>>(RpcRequest::LedgerAccountsGet(
                AccountQuery::MultipleIds(keys.to_vec()),
            ))
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|account| (account.id(), account.try_into().map_err(Arc::new)))
            .collect()
    }
}

pub(crate) fn create_account_loader(rpc_sender: RpcSender) -> AccountLoader {
    // TODO(adonagy): is 25 enough?
    Loader::new(AccountBatcher { rpc_sender }).with_yield_count(25)
}

#[derive(Debug, Clone)]
pub(crate) struct GraphQLAccount {
    inner: Account,
    // A command can name an account that is absent from the best ledger.
    is_partial: bool,
    // OCaml shows `stakingActive` true for every account of a delegator list.
    listed_as_delegator: bool,
    public_key: String,
    token_id: String,
    token: String,
    token_symbol: String,
    // balance: GraphQLBalance,
    nonce: String,
    receipt_chain_hash: String,
    // Storing the key for later
    delegate_key: Option<CompressedPubKey>,
    voting_for: String,
    timing: GraphQLTiming,
    permissions: GraphQLPermissions,
    // can we flatten?
    // pub zkapp: Option<GraphQLZkAppAccount>,
    zkapp_state: Option<Vec<String>>,
    verification_key: Option<GraphQLVerificationKey>,
    action_state: Option<Vec<String>>,
    proved_state: Option<bool>,
    zkapp_uri: Option<String>,
    // The account's index, leaf hash and path in the best tip's staged
    // ledger, from one snapshot, read once for all three fields.
    staged_path: Arc<tokio::sync::OnceCell<Result<Arc<RpcLightAccount>, RpcLightError>>>,
}

impl GraphQLAccount {
    /// The account OCaml shows for a public key that has none in the ledger
    /// (`Partial_account.of_pk`): a partial MINA account with that key and
    /// null ledger fields, where a field must name an account, such as a
    /// command's receiver.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1236-L1272>
    pub(crate) fn empty_for(public_key: CompressedPubKey) -> Result<Self, ConversionError> {
        let mut account: Self = Account::create_with(
            AccountId::new_with_default_token(public_key),
            Balance::zero(),
        )
        .try_into()?;
        account.is_partial = true;
        account.delegate_key = None;
        Ok(account)
    }

    /// Loads the MINA account of a public key, or [`Self::empty_for`] it when
    /// the ledger has none, as OCaml's `AccountObj.get_best_ledger_account_pk`.
    pub(crate) async fn load_or_empty(
        context: &Context,
        public_key: CompressedPubKey,
    ) -> Result<Self, ConversionError> {
        let id = AccountId::new_with_default_token(public_key.clone());
        match context.load_account(id).await {
            Some(account) => Ok(account),
            None => Self::empty_for(public_key),
        }
    }

    /// An account of a delegator list, which OCaml marks as actively
    /// staking whatever its key.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1556-L1614>
    fn delegator(account: Account) -> Result<Self, ConversionError> {
        let mut account = Self::try_from(account)?;
        account.listed_as_delegator = true;
        Ok(account)
    }

    /// The account's index, leaf hash and Merkle path in the best tip's
    /// staged ledger, all from the same ledger snapshot, so the path proves
    /// the leaf. Read through the light request limit, batched with the
    /// other accounts of the request. Null, as in OCaml's `Mina_lib.best_tip`
    /// reads, for a partial account, while the node bootstraps, or if the
    /// account is not there; an error if the node is busy.
    async fn staged_path(&self, context: &Context) -> FieldResult<Option<&RpcLightAccount>> {
        if self.is_partial {
            return Ok(None);
        }
        let read = self
            .staged_path
            .get_or_init(|| context.load_light_path(self.inner.id()))
            .await;
        match read {
            Ok(path) => Ok(Some(path)),
            Err(
                RpcLightError::NotSynced
                | RpcLightError::AccountNotFound(_)
                | RpcLightError::LedgerUnavailable { .. }
                | RpcLightError::BlockNotInBestChain(_),
            ) => Ok(None),
            Err(error) => Err(juniper::FieldError::from(error.to_string())),
        }
    }

    fn min_balance(&self, global_slot: Option<u32>) -> Option<Balance> {
        global_slot.map(|slot| match self.inner.timing {
            Timing::Untimed => Balance::zero(),
            Timing::Timed { .. } => self.inner.min_balance_at_slot(Slot::from_u32(slot)),
        })
    }

    fn liquid_balance(&self, global_slot: Option<u32>) -> Option<Balance> {
        let min_balance = self.min_balance(global_slot);
        let total = self.inner.balance;
        min_balance.map(|mb| {
            if total > mb {
                total.checked_sub(&mb).expect("overflow")
            } else {
                Balance::zero()
            }
        })
    }
}

#[graphql_object(context = Context, name = "Account")]
#[graphql(description = "A Mina account")]
impl GraphQLAccount {
    fn public_key(&self) -> &str {
        &self.public_key
    }

    fn token_id(&self) -> &str {
        &self.token_id
    }

    fn token(&self) -> &str {
        &self.token
    }

    fn token_symbol(&self) -> Option<&str> {
        (!self.is_partial).then_some(self.token_symbol.as_str())
    }

    async fn balance(&self, context: &Context) -> GraphQLBalance {
        let best_tip = if self.is_partial {
            None
        } else {
            context.get_or_fetch_best_tip().await
        };
        let global_slot = best_tip.as_ref().map(|bt| bt.global_slot());

        GraphQLBalance {
            total: self.inner.balance.as_u64().to_string(),
            block_height: best_tip
                .as_ref()
                .map(|bt| bt.height())
                .unwrap_or_default()
                .to_string(),
            state_hash: best_tip.as_ref().map(|bt| bt.hash().to_string()),
            liquid: self
                .liquid_balance(global_slot)
                .map(|b| b.as_u64().to_string()),
            locked: self
                .min_balance(global_slot)
                .map(|b| b.as_u64().to_string()),
            unknown: self.inner.balance.as_u64().to_string(),
        }
    }

    fn nonce(&self) -> Option<&str> {
        (!self.is_partial).then_some(self.nonce.as_str())
    }

    /// Like `nonce`, but counting the account's commands in the transaction
    /// pool: one more than the highest nonce among the pooled commands it
    /// pays the fee of, or the account's nonce when it has none there.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L782-L805>
    async fn inferred_nonce(&self, context: &Context) -> Option<String> {
        // Fee payers pay in MINA, so only MINA accounts have pooled commands.
        if !self.inner.token_id.is_default() {
            return self.nonce().map(str::to_owned);
        }
        let public_key = || Some(AccountPublicKey::from(self.inner.public_key.clone()));
        // The pool filters both lists by fee payer.
        let user_commands: RpcPooledUserCommandsResponse = context
            .rpc_sender
            .oneshot_request(RpcRequest::PooledUserCommands(PooledCommandsQuery {
                public_key: public_key(),
                hashes: None,
                ids: None,
            }))
            .await
            .unwrap_or_default();
        let zkapp_commands: RpcPooledZkappCommandsResponse = context
            .rpc_sender
            .oneshot_request(RpcRequest::PooledZkappCommands(PooledCommandsQuery {
                public_key: public_key(),
                hashes: None,
                ids: None,
            }))
            .await
            .unwrap_or_default();
        let pooled = user_commands
            .iter()
            .map(|cmd| cmd.payload.common.nonce.as_u32())
            .chain(
                zkapp_commands
                    .iter()
                    .map(|cmd| cmd.fee_payer.body.nonce.as_u32()),
            )
            .max();
        match pooled {
            Some(nonce) => Some(nonce.saturating_add(1).to_string()),
            None => self.nonce().map(str::to_owned),
        }
    }

    /// The public key this account delegates to
    fn delegate(&self) -> Option<String> {
        self.delegate_key.as_ref().map(|key| key.into_address())
    }

    fn receipt_chain_hash(&self) -> Option<&str> {
        (!self.is_partial).then_some(self.receipt_chain_hash.as_str())
    }

    async fn delegate_account(
        &self,
        context: &Context,
    ) -> FieldResult<Option<Box<GraphQLAccount>>> {
        // If we have a delegate key
        if let Some(delegate_key) = self.delegate_key.as_ref() {
            // A delegate always has the default token id
            let delegate_id = AccountId::new_with_default_token(delegate_key.clone());
            // Use the loader to fetch the delegate account
            Ok(context.load_account(delegate_id).await.map(Box::new))
        } else {
            // No delegate
            Ok(None)
        }
    }

    /// The account this account delegates to in the staking ledger of the
    /// best tip's epoch, as it is in the best tip's ledger: null when the
    /// account is not in that staking ledger, delegates to nobody there, or
    /// the node has no best tip yet. The staking ledger can name a delegate
    /// other than `delegate`, which is the best tip's.
    ///
    /// OCaml looks the account up in `Consensus.Hooks.get_epoch_ledger` of
    /// the best tip, its staking epoch snapshot or, in the first `k` blocks of
    /// an epoch, its next epoch snapshot: either way the ledger whose hash is
    /// the best tip's `stakingEpochData.ledger.hash`.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1489-L1534>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L2488-L2516>
    async fn epoch_delegate_account(
        &self,
        context: &Context,
    ) -> FieldResult<Option<Box<GraphQLAccount>>> {
        let staking: Vec<Account> = context
            .rpc_sender
            .oneshot_request(RpcRequest::LedgerAccountsGet(
                AccountQuery::StakingEpochIds(vec![self.inner.id()]),
            ))
            .await
            .unwrap_or_default();
        let Some(delegate) = staking.into_iter().next().and_then(|a| a.delegate) else {
            return Ok(None);
        };
        Ok(Some(Box::new(
            GraphQLAccount::load_or_empty(context, delegate).await?,
        )))
    }

    pub async fn delegators(&self, context: &Context) -> FieldResult<Vec<GraphQLAccount>> {
        if let Some(best_tip) = context.get_or_fetch_best_tip().await {
            let staking_ledger_hash = best_tip.staking_epoch_ledger_hash();

            let id = self.inner.id();
            let delegators = context
                .fetch_delegators(
                    DelegatorsQuery::Ledger(staking_ledger_hash.clone()),
                    id.clone(),
                )
                .await
                .unwrap_or_default();

            Ok(delegators
                .into_iter()
                .map(GraphQLAccount::delegator)
                .collect::<Result<Vec<_>, _>>()?)
        } else {
            Ok(vec![])
        }
    }

    /// The accounts that delegated to this account in the staking ledger of
    /// the epoch before the frontier root's, as OCaml's daemon keeps them: for
    /// its own block producer key only, so empty for any other key, and null
    /// until the root has crossed an epoch while the node ran (or when the
    /// node did not see that ledger). The accounts are as they were in that
    /// ledger, in ledger order (OCaml's are in hash table order). Private mode
    /// returns the full list as OCaml; public mode returns at most eight
    /// entries to bound nested ledger lookups within the public request cost.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1586-L1614>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L1102-L1127>
    async fn last_epoch_delegators(
        &self,
        context: &Context,
    ) -> FieldResult<Option<Vec<GraphQLAccount>>> {
        let limit = context.public_graphql.then_some(MAX_LAST_EPOCH_DELEGATORS);
        let Some(delegators) = context
            .fetch_delegators(DelegatorsQuery::LastEpoch { limit }, self.inner.id())
            .await
        else {
            return Ok(None);
        };
        Ok(Some(
            delegators
                .into_iter()
                .take(limit.unwrap_or(usize::MAX))
                .map(GraphQLAccount::delegator)
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }

    /// Whether this node produces blocks with this account's key: a MINA
    /// account whose key is the node's block producer key. True for every
    /// account of a delegator list, as OCaml.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1295-L1315>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1621-L1627>
    async fn staking_active(&self, context: &Context) -> bool {
        if self.listed_as_delegator {
            return true;
        }
        if !self.inner.token_id.is_default() {
            // Non-default token accounts cannot stake.
            return false;
        }
        let producer = context
            .get_or_fetch_status()
            .await
            .and_then(|status| status.block_producer);
        producer.is_some_and(|key| key == AccountPublicKey::from(self.inner.public_key.clone()))
    }

    fn voting_for(&self) -> Option<&str> {
        (!self.is_partial).then_some(self.voting_for.as_str())
    }

    fn timing(&self) -> &GraphQLTiming {
        &self.timing
    }

    fn permissions(&self) -> Option<&GraphQLPermissions> {
        (!self.is_partial).then_some(&self.permissions)
    }

    fn zkapp_state(&self) -> &Option<Vec<String>> {
        &self.zkapp_state
    }

    fn verification_key(&self) -> &Option<GraphQLVerificationKey> {
        &self.verification_key
    }

    fn action_state(&self) -> &Option<Vec<String>> {
        &self.action_state
    }

    fn proved_state(&self) -> &Option<bool> {
        &self.proved_state
    }

    /// The index of this account in the ledger, or null if this account
    /// does not yet have a known position in the best tip ledger
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1638-L1644>
    async fn index(&self, context: &Context) -> FieldResult<Option<i32>> {
        Ok(self
            .staged_path(context)
            .await?
            .and_then(|path| i32::try_from(path.index).ok()))
    }

    /// The base58Check-encoded hash of this account to bootstrap the
    /// merklePath
    ///
    /// The hash of the account as the same ledger read as `merklePath` has
    /// it, which may be newer than this object's other fields.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1703-L1711>
    async fn leaf_hash(&self, context: &Context) -> FieldResult<Option<FieldElemScalar>> {
        Ok(self
            .staged_path(context)
            .await?
            .map(|path| FieldElemScalar(path.leaf_hash.to_decimal())))
    }

    /// Merkle path is a list of path elements that are either the left or
    /// right hashes up to the root
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1712-L1727>
    async fn merkle_path(
        &self,
        context: &Context,
    ) -> FieldResult<Option<Vec<GraphQLMerklePathElement>>> {
        Ok(self
            .staged_path(context)
            .await?
            .map(|path| path.merkle_path.iter().map(Into::into).collect()))
    }

    fn zkapp_uri(&self) -> &Option<String> {
        &self.zkapp_uri
    }
}

#[derive(GraphQLObject, Debug, Clone)]
pub struct GraphQLDelegateAccount {
    pub public_key: String,
}

/// Slots are strings, as OCaml's `Globalslot` and `GlobalSlotSpan` scalars.
#[derive(GraphQLObject, Debug, Clone)]
#[graphql(name = "AccountTiming")]
pub struct GraphQLTiming {
    // pub is_timed: bool,
    pub initial_minimum_balance: Option<String>,
    pub cliff_time: Option<String>,
    pub cliff_amount: Option<String>,
    pub vesting_period: Option<String>,
    pub vesting_increment: Option<String>,
}

#[derive(GraphQLInputObject, Debug, Clone)]
pub struct InputGraphQLTiming {
    // pub is_timed: bool,
    pub initial_minimum_balance: String,
    pub cliff_time: i32,
    pub cliff_amount: String,
    pub vesting_period: i32,
    pub vesting_increment: String,
}

impl From<MinaBaseAccountUpdateUpdateTimingInfoStableV1> for GraphQLTiming {
    fn from(value: MinaBaseAccountUpdateUpdateTimingInfoStableV1) -> Self {
        Self {
            initial_minimum_balance: Some(value.initial_minimum_balance.0.as_u64().to_string()),
            cliff_time: Some(value.cliff_time.as_u32().to_string()),
            cliff_amount: Some(value.cliff_amount.as_u64().to_string()),
            vesting_period: Some(value.vesting_period.as_u32().to_string()),
            vesting_increment: Some(value.vesting_increment.0.as_u64().to_string()),
        }
    }
}

#[derive(GraphQLObject, Debug, Clone)]
pub struct GraphQLPermissions {
    pub edit_state: String,
    pub access: String,
    pub send: String,
    pub receive: String,
    pub set_delegate: String,
    pub set_permissions: String,
    pub set_verification_key: GraphQLSetVerificationKey,
    pub set_zkapp_uri: String,
    pub edit_action_state: String,
    pub set_token_symbol: String,
    pub increment_nonce: String,
    pub set_voting_for: String,
    pub set_timing: String,
}

#[derive(GraphQLObject, Debug, Clone)]
pub struct GraphQLSetVerificationKey {
    pub auth: String,
    pub txn_version: String,
}

#[derive(GraphQLObject, Debug, Clone)]
pub struct GraphQLBalance {
    pub total: String,
    pub block_height: String,
    pub state_hash: Option<String>,
    pub liquid: Option<String>,
    pub locked: Option<String>,
    pub unknown: String,
}

// #[derive(GraphQLObject, Debug)]
// pub struct GraphQLZkAppAccount {
//     pub app_state: Vec<String>,
//     pub verification_key: Option<GraphQLVerificationKey>,
//     pub zkapp_version: i32,
//     pub action_state: Vec<String>,
//     pub last_action_slot: i32,
//     pub proved_state: bool,
//     pub zkapp_uri: String,
// }

/// Serves both OCaml's `AccountVerificationKeyWithHash` (`verificationKey`,
/// `hash`) and the account update's `VerificationKeyWithHash` (`data`,
/// `hash`).
#[derive(GraphQLObject, Debug, Clone)]
pub struct GraphQLVerificationKey {
    // pub max_proofs_verified: String,
    // pub actual_wrap_domain_size: String,
    // pub wrap_index: String,
    pub verification_key: String,
    /// The same as `verificationKey`, under the account update's name
    pub data: String,
    pub hash: String,
}

impl From<ledger::SetVerificationKey<ledger::AuthRequired>> for GraphQLSetVerificationKey {
    fn from(value: ledger::SetVerificationKey<ledger::AuthRequired>) -> Self {
        Self {
            auth: value.auth.to_string(),
            txn_version: value.txn_version.as_u32().to_string(),
        }
    }
}

impl From<ledger::Permissions<ledger::AuthRequired>> for GraphQLPermissions {
    fn from(value: ledger::Permissions<ledger::AuthRequired>) -> Self {
        Self {
            edit_state: value.edit_state.to_string(),
            access: value.access.to_string(),
            send: value.send.to_string(),
            receive: value.receive.to_string(),
            set_delegate: value.set_delegate.to_string(),
            set_permissions: value.set_permissions.to_string(),
            set_verification_key: GraphQLSetVerificationKey::from(value.set_verification_key),
            set_zkapp_uri: value.set_zkapp_uri.to_string(),
            edit_action_state: value.edit_action_state.to_string(),
            set_token_symbol: value.set_token_symbol.to_string(),
            increment_nonce: value.increment_nonce.to_string(),
            set_voting_for: value.set_voting_for.to_string(),
            set_timing: value.set_timing.to_string(),
        }
    }
}

impl From<ledger::Timing> for GraphQLTiming {
    fn from(value: ledger::Timing) -> Self {
        match value {
            ledger::Timing::Untimed => Self {
                initial_minimum_balance: None,
                vesting_period: None,
                cliff_time: None,
                cliff_amount: None,
                vesting_increment: None,
            },
            ledger::Timing::Timed {
                initial_minimum_balance,
                cliff_time,
                cliff_amount,
                vesting_period,
                vesting_increment,
            } => Self {
                initial_minimum_balance: Some(initial_minimum_balance.as_u64().to_string()),
                cliff_time: Some(cliff_time.as_u32().to_string()),
                cliff_amount: Some(cliff_amount.as_u64().to_string()),
                vesting_period: Some(vesting_period.as_u32().to_string()),
                vesting_increment: Some(vesting_increment.as_u64().to_string()),
            },
        }
    }
}

impl TryFrom<ledger::Account> for GraphQLAccount {
    type Error = ConversionError;

    fn try_from(value: ledger::Account) -> Result<Self, Self::Error> {
        // Process the verification_key with proper error handling
        let verification_key = value
            .zkapp
            .clone()
            .and_then(|zkapp| {
                zkapp.verification_key.map(|vk| {
                    let ser = MinaBaseVerificationKeyWireStableV1::from(vk.vk()).to_base64()?;

                    Ok(GraphQLVerificationKey {
                        verification_key: ser.clone(),
                        data: ser,
                        hash: vk.hash().to_decimal(),
                    }) as Result<GraphQLVerificationKey, Self::Error>
                })
            })
            .transpose()?; // Transpose Option<Result<...>> to Result<Option<...>>

        Ok(Self {
            inner: value.clone(),
            is_partial: false,
            listed_as_delegator: false,
            public_key: value.public_key.into_address(),
            token_id: TokenIdKeyHash::from(value.token_id.clone()).to_string(),
            token: TokenIdKeyHash::from(value.token_id).to_string(),
            token_symbol: TokenSymbol::from(&value.token_symbol).to_string(),
            // balance: GraphQLBalance::from(value.balance),
            nonce: value.nonce.as_u32().to_string(),
            receipt_chain_hash: ReceiptChainHash::from(value.receipt_chain_hash).to_string(),
            delegate_key: value.delegate,
            voting_for: value.voting_for.to_base58check_graphql(),
            timing: GraphQLTiming::from(value.timing),
            permissions: GraphQLPermissions::from(value.permissions),
            // zkapp: value.zkapp.map(GraphQLZkAppAccount::from),
            // TODO: keep as array?
            zkapp_state: value.zkapp.clone().map(|zkapp| {
                zkapp
                    .app_state
                    .into_iter()
                    .map(|v| v.to_decimal())
                    .collect::<Vec<_>>()
            }),
            verification_key,
            action_state: value.zkapp.clone().map(|zkapp| {
                zkapp
                    .action_state
                    .into_iter()
                    .map(|v| v.to_decimal())
                    .collect::<Vec<_>>()
            }),
            proved_state: value.zkapp.clone().map(|zkapp| zkapp.proved_state),
            zkapp_uri: value
                .zkapp
                .map(|zkapp| ZkAppUri::from(&zkapp.zkapp_uri).to_string()),
            staged_path: Default::default(),
        })
    }
}
