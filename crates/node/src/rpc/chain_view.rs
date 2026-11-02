//! MINAWEB-RPC1: compact reads for the web node's pages, also on the native
//! HTTP API.
//!
//! - [`RpcRequest::BestChainHeadersGet`](super::RpcRequest::BestChainHeadersGet):
//!   the best chain's headers, newest first, with each block's command,
//!   zkApp and SNARK work counts and fees, without serialising the blocks.
//! - [`RpcRequest::BestChainBlockGet`](super::RpcRequest::BestChainBlockGet):
//!   one best-chain block's header, commands, fee transfers, coinbase and
//!   SNARK work.
//! - [`RpcRequest::SyncProgressGet`](super::RpcRequest::SyncProgressGet):
//!   how far a sync has got, ledger by ledger and block by block, and the
//!   frontier's `k`.
//!
//! All are answered from the state in the reducer and change nothing but
//! the RPC request table.

use ledger::{scan_state::transaction_logic::Transaction, Account, FpExt, Timing};
use mina_core::{
    block::{AppliedBlock, ArcBlockWithHash},
    constants::constraint_constants,
};
use mina_node_account::AccountPublicKey;
use mina_p2p_messages::v2::{
    LedgerHash, MinaBaseSignedCommandPayloadBodyStableV2, MinaBaseStakeDelegationStableV2,
    MinaBaseTransactionStatusStableV2, MinaBaseUserCommandStableV2, StateHash, TokenIdKeyHash,
    TransactionHash, TransactionSnarkWorkTStableV2Proofs,
};
use serde::{Deserialize, Serialize};

use crate::{
    p2p::channels::streaming_rpc::P2pStreamingRpcReceiveProgress,
    transition_frontier::sync::{
        ledger::{
            snarked::TransitionFrontierSyncLedgerSnarkedState, TransitionFrontierSyncLedgerState,
        },
        SyncPhase, TransitionFrontierSyncBlockState, TransitionFrontierSyncState,
    },
    State,
};

use super::{LedgerSyncProgress, RootLedgerSyncProgress, RootStagedLedgerSyncProgress};

/// Nanomina as a decimal string, as GraphQL sends amounts: a JSON number
/// loses precision past 2^53 (9,007,199 MINA), which a balance can pass.
/// Reads a string or a number.
mod nanomina {
    use serde::{Deserialize, Deserializer, Serializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        String(String),
        Number(u64),
    }

    pub fn serialize<S: Serializer>(value: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        match Either::deserialize(d)? {
            Either::String(s) => s.parse().map_err(serde::de::Error::custom),
            Either::Number(n) => Ok(n),
        }
    }

    pub mod opt {
        use serde::{Deserialize, Deserializer, Serializer};

        pub fn serialize<S: Serializer>(value: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
            match value {
                Some(v) => s.collect_str(v),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
            Option::<super::Either>::deserialize(d)?
                .map(|v| match v {
                    super::Either::String(s) => s.parse().map_err(serde::de::Error::custom),
                    super::Either::Number(n) => Ok(n),
                })
                .transpose()
        }
    }
}

/// The most headers one request returns: the whole frontier (k + 1 blocks
/// on mainnet and devnet) fits.
pub const MAX_BEST_CHAIN_HEADERS: usize = 300;
/// Headers a request without a limit returns.
pub const DEFAULT_BEST_CHAIN_HEADERS: usize = 20;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct RpcBestChainHeadersQuery {
    /// At most this many headers, capped at [`MAX_BEST_CHAIN_HEADERS`];
    /// [`DEFAULT_BEST_CHAIN_HEADERS`] if `None`.
    pub limit: Option<usize>,
    /// Only blocks at this height or below, to page down the chain.
    pub max_height: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcBestChainHeaders {
    /// Consensus `k`: the frontier holds k + 1 blocks, the root and k after.
    pub k: u32,
    /// The best chain's root and tip heights; `None` without a best chain.
    pub root_height: Option<u32>,
    pub best_tip_height: Option<u32>,
    /// Newest first.
    pub headers: Vec<RpcBlockHeader>,
}

/// A best-chain block's header and the counts and fees of its body.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcBlockHeader {
    pub height: u32,
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub hash: StateHash,
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub pred_hash: StateHash,
    /// The slot since the hard fork.
    pub global_slot: u32,
    pub global_slot_since_genesis: u32,
    pub epoch: u32,
    /// The slot within the epoch.
    pub slot: u32,
    /// The block's own time, milliseconds since the Unix epoch.
    pub timestamp_ms: u64,
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub producer: AccountPublicKey,
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub coinbase_receiver: AccountPublicKey,
    /// Payments and delegations.
    pub user_commands: u32,
    pub zkapp_commands: u32,
    /// Commands of either kind that failed and only paid their fee.
    pub failed_commands: u32,
    /// Completed SNARK work bundles the block bought.
    pub snark_jobs: u32,
    /// The commands' fees, in nanomina.
    #[serde(with = "nanomina")]
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub transaction_fees: u64,
    /// The SNARK work's fees, in nanomina.
    #[serde(with = "nanomina")]
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub snark_fees: u64,
    /// The block finished a staged ledger and emitted a ledger proof.
    pub proof_emitted: bool,
}

impl RpcBlockHeader {
    pub fn new(applied: &AppliedBlock) -> Self {
        let block = &applied.block;
        let consensus = block.consensus_state();
        let mut user_commands = 0u32;
        let mut zkapp_commands = 0u32;
        let mut failed_commands = 0u32;
        let mut transaction_fees = 0u64;
        for command in block.commands_iter() {
            match &command.data {
                MinaBaseUserCommandStableV2::SignedCommand(signed) => {
                    user_commands = user_commands.saturating_add(1);
                    transaction_fees =
                        transaction_fees.saturating_add(signed.payload.common.fee.as_u64());
                }
                MinaBaseUserCommandStableV2::ZkappCommand(zkapp) => {
                    zkapp_commands = zkapp_commands.saturating_add(1);
                    transaction_fees =
                        transaction_fees.saturating_add(zkapp.fee_payer.body.fee.as_u64());
                }
            }
            if matches!(command.status, MinaBaseTransactionStatusStableV2::Failed(_)) {
                failed_commands = failed_commands.saturating_add(1);
            }
        }
        let (snark_jobs, snark_fees) =
            block
                .completed_works_iter()
                .fold((0u32, 0u64), |(jobs, fees), work| {
                    (
                        jobs.saturating_add(1),
                        fees.saturating_add(work.fee.as_u64()),
                    )
                });
        Self {
            height: block.height(),
            hash: block.hash().clone(),
            pred_hash: block.pred_hash().clone(),
            global_slot: block.global_slot(),
            global_slot_since_genesis: block.global_slot_since_genesis(),
            epoch: consensus.epoch_count.as_u32(),
            slot: block.slot(),
            timestamp_ms: u64::from(block.timestamp()) / 1_000_000,
            producer: consensus.block_creator.clone().into(),
            coinbase_receiver: consensus.coinbase_receiver.clone().into(),
            user_commands,
            zkapp_commands,
            failed_commands,
            snark_jobs,
            transaction_fees,
            snark_fees,
            proof_emitted: applied.just_emitted_a_proof,
        }
    }
}

/// The headers [`RpcBestChainHeadersQuery`] asks for, from a best chain
/// ordered root first.
pub fn best_chain_headers(
    k: u32,
    best_chain: &[AppliedBlock],
    query: &RpcBestChainHeadersQuery,
) -> RpcBestChainHeaders {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_BEST_CHAIN_HEADERS)
        .min(MAX_BEST_CHAIN_HEADERS);
    let headers = best_chain
        .iter()
        .rev()
        .filter(|b| query.max_height.is_none_or(|max| b.block.height() <= max))
        .take(limit)
        .map(RpcBlockHeader::new)
        .collect();
    RpcBestChainHeaders {
        k,
        root_height: best_chain.first().map(|b| b.block.height()),
        best_tip_height: best_chain.last().map(|b| b.block.height()),
        headers,
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum RpcBestChainBlockQuery {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    Hash(StateHash),
    Height(u32),
}

pub type RpcBestChainBlockGetResponse = Option<RpcBlockDetail>;

/// One best-chain block: its header, then what it applied.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcBlockDetail {
    pub header: RpcBlockHeader,
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub snarked_ledger_hash: LedgerHash,
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub staged_ledger_hash: LedgerHash,
    /// The coinbase, in nanomina, summed over its parts; `None` if the
    /// block's diff couldn't be expanded into transactions.
    #[serde(with = "nanomina::opt")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub coinbase: Option<u64>,
    /// In the order the block applies them.
    pub commands: Vec<RpcBlockCommand>,
    /// In the order the block applies them; empty if `coinbase` is `None`.
    pub fee_transfers: Vec<RpcBlockFeeTransfer>,
    pub snark_work: Vec<RpcBlockSnarkWork>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RpcBlockCommandKind {
    Payment,
    Delegation,
    Zkapp,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcBlockCommand {
    pub kind: RpcBlockCommandKind,
    /// `None` if hashing failed.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub hash: Option<TransactionHash>,
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub fee_payer: AccountPublicKey,
    /// A payment's receiver or a delegation's new delegate.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub receiver: Option<AccountPublicKey>,
    /// A payment's amount, in nanomina.
    #[serde(with = "nanomina::opt")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub amount: Option<u64>,
    #[serde(with = "nanomina")]
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub fee: u64,
    pub nonce: u32,
    /// Base58Check, as GraphQL shows it.
    pub memo: String,
    /// A zkApp command's account updates, outermost only.
    pub account_updates: Option<u32>,
    /// The first failure, if the command failed.
    pub failure: Option<String>,
}

impl RpcBlockCommand {
    fn new(
        command: &MinaBaseUserCommandStableV2,
        status: &MinaBaseTransactionStatusStableV2,
    ) -> Self {
        let failure = match status {
            MinaBaseTransactionStatusStableV2::Failed(failures) => failures
                .0
                .iter()
                .flat_map(|failures| failures.iter())
                .next()
                .map(|failure| {
                    ledger::scan_state::transaction_logic::TransactionFailure::from(failure)
                        .to_string()
                })
                .or_else(|| Some("failed".to_owned())),
            MinaBaseTransactionStatusStableV2::Applied => None,
        };
        let hash = command.hash().ok();
        match command {
            MinaBaseUserCommandStableV2::SignedCommand(signed) => {
                let common = &signed.payload.common;
                let (kind, receiver, amount) = match &signed.payload.body {
                    MinaBaseSignedCommandPayloadBodyStableV2::Payment(payment) => (
                        RpcBlockCommandKind::Payment,
                        payment.receiver_pk.clone(),
                        Some(payment.amount.as_u64()),
                    ),
                    MinaBaseSignedCommandPayloadBodyStableV2::StakeDelegation(
                        MinaBaseStakeDelegationStableV2::SetDelegate { new_delegate },
                    ) => (RpcBlockCommandKind::Delegation, new_delegate.clone(), None),
                };
                Self {
                    kind,
                    hash,
                    fee_payer: common.fee_payer_pk.clone().into(),
                    receiver: Some(receiver.into()),
                    amount,
                    fee: common.fee.as_u64(),
                    nonce: common.nonce.as_u32(),
                    memo: common.memo.to_base58check(),
                    account_updates: None,
                    failure,
                }
            }
            MinaBaseUserCommandStableV2::ZkappCommand(zkapp) => {
                let body = &zkapp.fee_payer.body;
                Self {
                    kind: RpcBlockCommandKind::Zkapp,
                    hash,
                    fee_payer: body.public_key.clone().into(),
                    receiver: None,
                    amount: None,
                    fee: body.fee.as_u64(),
                    nonce: body.nonce.as_u32(),
                    memo: zkapp.memo.to_base58check(),
                    account_updates: Some(
                        u32::try_from(zkapp.account_updates.len()).unwrap_or(u32::MAX),
                    ),
                    failure,
                }
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcBlockFeeTransfer {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub receiver: AccountPublicKey,
    #[serde(with = "nanomina")]
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub fee: u64,
    /// Paid out of the coinbase (to the SNARK worker of a coinbase's work)
    /// rather than out of the block's fees.
    pub via_coinbase: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcBlockSnarkWork {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub prover: AccountPublicKey,
    #[serde(with = "nanomina")]
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub fee: u64,
    /// Proofs in the bundle: one or two.
    pub proofs: u8,
}

impl RpcBlockDetail {
    pub fn new(applied: &AppliedBlock) -> Self {
        let block = &applied.block;
        let commands = block
            .commands_iter()
            .map(|c| RpcBlockCommand::new(&c.data, &c.status))
            .collect();
        let snark_work = block
            .completed_works_iter()
            .map(|work| RpcBlockSnarkWork {
                prover: work.prover.clone().into(),
                fee: work.fee.as_u64(),
                proofs: match work.proofs {
                    TransactionSnarkWorkTStableV2Proofs::One(_) => 1,
                    TransactionSnarkWorkTStableV2Proofs::Two(_) => 2,
                },
            })
            .collect();
        let (coinbase, fee_transfers) = match coinbase_and_fee_transfers(block) {
            Some((coinbase, transfers)) => (Some(coinbase), transfers),
            None => (None, Vec::new()),
        };
        Self {
            header: RpcBlockHeader::new(applied),
            snarked_ledger_hash: block.snarked_ledger_hash().clone(),
            staged_ledger_hash: block.merkle_root_hash().clone(),
            coinbase,
            commands,
            fee_transfers,
            snark_work,
        }
    }
}

/// The coinbase and fee transfers a block's diff expands to, as the staged
/// ledger applies it and as GraphQL's `transactions` lists them (in the
/// apply order here, not GraphQL's reversed one).
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/filtered_external_transition/filtered_external_transition.ml#L144-L229>
fn coinbase_and_fee_transfers(block: &ArcBlockWithHash) -> Option<(u64, Vec<RpcBlockFeeTransfer>)> {
    let consensus = block.consensus_state();
    let diff =
        ledger::staged_ledger::diff::Diff::try_from(&block.body().staged_ledger_diff).ok()?;
    let receiver = (&consensus.coinbase_receiver).try_into().ok()?;
    let transactions = diff
        .get_transactions(
            constraint_constants(),
            receiver,
            consensus.supercharge_coinbase,
        )
        .ok()?;
    let mut coinbase = 0u64;
    let mut transfers = Vec::new();
    for transaction in transactions {
        match transaction.data {
            Transaction::Command(_) => {}
            Transaction::FeeTransfer(fee_transfer) => {
                transfers.extend(fee_transfer.iter().map(|t| RpcBlockFeeTransfer {
                    receiver: t.receiver_pk.clone().into(),
                    fee: t.fee.as_u64(),
                    via_coinbase: false,
                }));
            }
            Transaction::Coinbase(part) => {
                coinbase = coinbase.checked_add(part.amount.as_u64())?;
                if let Some(t) = &part.fee_transfer {
                    transfers.push(RpcBlockFeeTransfer {
                        receiver: t.receiver_pk.clone().into(),
                        fee: t.fee.as_u64(),
                        via_coinbase: true,
                    });
                }
            }
        }
    }
    Some((coinbase, transfers))
}

/// The best-chain block a [`RpcBestChainBlockQuery`] names, if the frontier
/// holds it.
pub fn best_chain_block(
    best_chain: &[AppliedBlock],
    query: &RpcBestChainBlockQuery,
) -> RpcBestChainBlockGetResponse {
    best_chain
        .iter()
        .find(|b| match query {
            RpcBestChainBlockQuery::Hash(hash) => b.block.hash() == hash,
            RpcBestChainBlockQuery::Height(height) => b.block.height() == *height,
        })
        .map(RpcBlockDetail::new)
}

/// Where a sync is: the ledgers in the order it fetches them, then the
/// blocks, then the commit.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum RpcSyncStage {
    Idle,
    Init,
    StakingLedger,
    NextEpochLedger,
    RootLedger,
    Blocks,
    Commit,
    Synced,
}

impl RpcSyncStage {
    pub fn of(sync: &TransitionFrontierSyncState) -> Self {
        use TransitionFrontierSyncState as S;
        match sync {
            S::Idle => Self::Idle,
            S::Init { .. } => Self::Init,
            S::StakingLedgerPending(_) | S::StakingLedgerSuccess { .. } => Self::StakingLedger,
            S::NextEpochLedgerPending(_) | S::NextEpochLedgerSuccess { .. } => {
                Self::NextEpochLedger
            }
            S::RootLedgerPending(_) | S::RootLedgerSuccess { .. } => Self::RootLedger,
            S::BlocksPending { .. } | S::BlocksSuccess { .. } => Self::Blocks,
            S::CommitPending { .. } | S::CommitSuccess { .. } => Self::Commit,
            S::Synced { .. } => Self::Synced,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RpcLedgerSyncStatus {
    /// The sync hasn't reached this ledger.
    Waiting,
    Syncing,
    /// Fetched, or already held and skipped.
    Done,
}

/// One ledger's progress in a sync.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcLedgerSyncProgress {
    pub status: RpcLedgerSyncStatus,
    /// What the progress bar fills with, while syncing: for a snarked
    /// ledger, accounts and hashes fetched of an estimate; for the staged
    /// ledger, parts fetched of the parts.
    pub fetched: Option<u64>,
    pub total: Option<u64>,
    /// A snarked ledger's accounts: fetched, of what the peer said it has.
    pub accounts_fetched: Option<u64>,
    pub accounts_total: Option<u64>,
}

impl RpcLedgerSyncProgress {
    fn new(status: RpcLedgerSyncStatus) -> Self {
        Self {
            status,
            fetched: None,
            total: None,
            accounts_fetched: None,
            accounts_total: None,
        }
    }
}

/// The blocks between the root and the target, while the sync fetches and
/// applies them.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcBlocksSyncProgress {
    /// Blocks in the chain the sync builds, the root first.
    pub total: u32,
    /// Fetched or further along.
    pub fetched: u32,
    pub applied: u32,
    pub failed: u32,
    /// One character a block, root first: `.` missing, `f` fetching, `F`
    /// fetched, `a` applying, `A` applied, `x` failed to apply.
    pub cells: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcSyncProgress {
    /// Consensus `k`: a synced frontier holds k + 1 blocks.
    pub k: u32,
    /// The phase `/status` reports.
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub phase: SyncPhase,
    pub stage: RpcSyncStage,
    pub root_height: Option<u32>,
    pub target_height: Option<u32>,
    pub staking_epoch_ledger: RpcLedgerSyncProgress,
    pub next_epoch_ledger: RpcLedgerSyncProgress,
    pub root_snarked_ledger: RpcLedgerSyncProgress,
    pub root_staged_ledger: RpcLedgerSyncProgress,
    /// `None` before the sync reaches its blocks, and after a commit.
    pub blocks: Option<RpcBlocksSyncProgress>,
}

/// A snarked ledger's progress: the estimate the message-progress RPC has
/// always reported, and the account counts.
fn snarked_progress(state: &TransitionFrontierSyncLedgerSnarkedState) -> RpcLedgerSyncProgress {
    let mut progress = RpcLedgerSyncProgress::new(RpcLedgerSyncStatus::Syncing);
    if let Some(LedgerSyncProgress {
        fetched,
        estimation,
    }) = state.estimation()
    {
        progress.fetched = Some(fetched);
        progress.total = Some(estimation);
    }
    if let TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncPending {
        total_accounts_expected,
        synced_accounts_count,
        ..
    } = state
    {
        progress.accounts_fetched = Some(*synced_accounts_count);
        progress.accounts_total = Some(*total_accounts_expected);
    }
    progress
}

/// The ledger the sync fetches now, as the message-progress RPC reports it:
/// `(staking, next epoch, root)`, one of them `Some` at most. The root's
/// staged part is `fetched == total` before its fetch starts, and `0 of 1`
/// while the fetching peer's progress is unknown.
pub fn ledger_sync_progress(
    state: &State,
) -> (
    Option<LedgerSyncProgress>,
    Option<LedgerSyncProgress>,
    Option<RootLedgerSyncProgress>,
) {
    use TransitionFrontierSyncLedgerState as L;
    match &state.transition_frontier.sync {
        TransitionFrontierSyncState::StakingLedgerPending(s) => match &s.ledger {
            L::Snarked(s) => (s.estimation(), None, None),
            _ => (None, None, None),
        },
        TransitionFrontierSyncState::NextEpochLedgerPending(s) => match &s.ledger {
            L::Snarked(s) => (None, s.estimation(), None),
            _ => (None, None, None),
        },
        TransitionFrontierSyncState::RootLedgerPending(s) => match &s.ledger {
            L::Snarked(s) => (
                None,
                None,
                s.estimation().map(|data| RootLedgerSyncProgress {
                    fetched: data.fetched,
                    estimation: data.estimation,
                    staged: None,
                }),
            ),
            L::Staged(s) => {
                let staged = staged_progress(state, s);
                // A complete snarked part, so a client that sums the
                // fractions shows it done while the staged part fetches.
                (
                    None,
                    None,
                    Some(RootLedgerSyncProgress {
                        fetched: 1,
                        estimation: 1,
                        staged,
                    }),
                )
            }
            _ => (None, None, None),
        },
        _ => (None, None, None),
    }
}

fn staged_progress(
    state: &State,
    staged: &crate::transition_frontier::sync::ledger::staged::TransitionFrontierSyncLedgerStagedState,
) -> Option<RootStagedLedgerSyncProgress> {
    let unknown = || RootStagedLedgerSyncProgress {
        fetched: 0,
        total: 1,
    };
    match staged.fetch_attempts() {
        None => staged.target_with_parts().map(|(_, parts)| {
            let v = parts
                .map(|parts| parts.total_pieces_to_transfer())
                .unwrap_or(0);
            RootStagedLedgerSyncProgress {
                fetched: v,
                total: v,
            }
        }),
        Some(attempts) => attempts
            .iter()
            .find(|(_, s)| s.fetch_pending_rpc_id().is_some())
            .map(|(id, _)| id)
            .and_then(|peer_id| state.p2p.get_ready_peer(peer_id))
            .map(
                |peer| match peer.channels.streaming_rpc.pending_local_rpc_progress() {
                    None => unknown(),
                    Some(P2pStreamingRpcReceiveProgress::StagedLedgerParts(progress)) => {
                        let (fetched, total) = progress.progress();
                        RootStagedLedgerSyncProgress { fetched, total }
                    }
                },
            ),
    }
}

fn block_cell(block: &TransitionFrontierSyncBlockState) -> char {
    match block {
        TransitionFrontierSyncBlockState::FetchPending { attempts, .. } => {
            if attempts.values().any(|a| a.fetch_pending_since().is_some()) {
                'f'
            } else {
                '.'
            }
        }
        TransitionFrontierSyncBlockState::FetchSuccess { .. } => 'F',
        TransitionFrontierSyncBlockState::ApplyPending { .. } => 'a',
        TransitionFrontierSyncBlockState::ApplyError { .. } => 'x',
        TransitionFrontierSyncBlockState::ApplySuccess { .. } => 'A',
    }
}

/// Counts and cells for a chain of block states, root first.
pub fn blocks_progress(cells: impl IntoIterator<Item = char>) -> RpcBlocksSyncProgress {
    let cells: String = cells.into_iter().collect();
    let count = |pred: fn(char) -> bool| {
        u32::try_from(cells.chars().filter(|c| pred(*c)).count()).unwrap_or(u32::MAX)
    };
    RpcBlocksSyncProgress {
        total: count(|_| true),
        fetched: count(|c| matches!(c, 'F' | 'a' | 'A' | 'x')),
        applied: count(|c| c == 'A'),
        failed: count(|c| c == 'x'),
        cells,
    }
}

pub fn sync_progress(state: &State) -> RpcSyncProgress {
    use RpcLedgerSyncStatus::*;
    let sync = &state.transition_frontier.sync;
    let stage = RpcSyncStage::of(sync);
    // A ledger before the stage is done; the stage's own ledger syncs.
    let status_for = |own: RpcSyncStage| match stage.cmp(&own) {
        std::cmp::Ordering::Less => Waiting,
        std::cmp::Ordering::Equal => Syncing,
        std::cmp::Ordering::Greater => Done,
    };
    let mut staking = RpcLedgerSyncProgress::new(status_for(RpcSyncStage::StakingLedger));
    let mut next = RpcLedgerSyncProgress::new(status_for(RpcSyncStage::NextEpochLedger));
    let mut root_snarked = RpcLedgerSyncProgress::new(status_for(RpcSyncStage::RootLedger));
    let mut root_staged = RpcLedgerSyncProgress::new(status_for(RpcSyncStage::RootLedger));
    use TransitionFrontierSyncLedgerState as L;
    match sync {
        TransitionFrontierSyncState::StakingLedgerPending(s) => {
            if let L::Snarked(s) = &s.ledger {
                staking = snarked_progress(s);
            }
            root_staged.status = Waiting;
        }
        TransitionFrontierSyncState::StakingLedgerSuccess { .. } => staking.status = Done,
        TransitionFrontierSyncState::NextEpochLedgerPending(s) => {
            if let L::Snarked(s) = &s.ledger {
                next = snarked_progress(s);
            }
        }
        TransitionFrontierSyncState::NextEpochLedgerSuccess { .. } => next.status = Done,
        TransitionFrontierSyncState::RootLedgerPending(s) => match &s.ledger {
            L::Snarked(s) => {
                root_snarked = snarked_progress(s);
                root_staged.status = Waiting;
            }
            L::Staged(s) => {
                root_snarked.status = Done;
                if let Some(p) = staged_progress(state, s) {
                    root_staged.fetched = Some(p.fetched);
                    root_staged.total = Some(p.total);
                }
            }
            _ => root_staged.status = Waiting,
        },
        TransitionFrontierSyncState::RootLedgerSuccess { .. } => {
            root_snarked.status = Done;
            root_staged.status = Done;
        }
        _ => {}
    }
    let blocks = match sync {
        TransitionFrontierSyncState::BlocksPending { chain, .. } => {
            Some(blocks_progress(chain.iter().map(block_cell)))
        }
        TransitionFrontierSyncState::BlocksSuccess { chain, .. }
        | TransitionFrontierSyncState::CommitPending { chain, .. }
        | TransitionFrontierSyncState::CommitSuccess { chain, .. } => {
            Some(blocks_progress(chain.iter().map(|_| 'A')))
        }
        _ => None,
    };
    RpcSyncProgress {
        k: state.config.consensus_constants.k,
        phase: state.reported_sync_phase(),
        stage,
        root_height: sync.root_block().map(|b| b.height()),
        target_height: sync.best_tip().map(|b| b.height()).or_else(|| match sync {
            // The tip isn't fetched yet: the chain runs from the root to it.
            TransitionFrontierSyncState::BlocksPending { chain, .. } => {
                let root = sync.root_block()?.height();
                let after = u32::try_from(chain.len().checked_sub(1)?).ok()?;
                root.checked_add(after)
            }
            _ => None,
        }),
        staking_epoch_ledger: staking,
        next_epoch_ledger: next,
        root_snarked_ledger: root_snarked,
        root_staged_ledger: root_staged,
        blocks,
    }
}

/// An account as the web node's account page shows it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcAccountSummary {
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub public_key: AccountPublicKey,
    /// The token id, in Base58Check; the default token is MINA.
    pub token_id: String,
    /// The account's own token symbol, if it set one.
    pub token_symbol: String,
    /// Nanomina.
    #[serde(with = "nanomina")]
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub balance: u64,
    pub nonce: u32,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub delegate: Option<AccountPublicKey>,
    /// `None` for an untimed account.
    pub timing: Option<RpcAccountTiming>,
    /// `None` for an account that isn't a zkApp.
    pub zkapp: Option<RpcAccountZkapp>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcAccountTiming {
    #[serde(with = "nanomina")]
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub initial_minimum_balance: u64,
    pub cliff_time: u32,
    #[serde(with = "nanomina")]
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub cliff_amount: u64,
    pub vesting_period: u32,
    #[serde(with = "nanomina")]
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub vesting_increment: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcAccountZkapp {
    /// The eight state fields, as decimal strings.
    pub app_state: Vec<String>,
    /// The verification key's hash, as a decimal string; `None` without a
    /// key.
    pub verification_key_hash: Option<String>,
    pub zkapp_version: u32,
    pub proved_state: bool,
    pub zkapp_uri: String,
}

impl From<&Account> for RpcAccountSummary {
    fn from(account: &Account) -> Self {
        let timing = match &account.timing {
            Timing::Untimed => None,
            Timing::Timed {
                initial_minimum_balance,
                cliff_time,
                cliff_amount,
                vesting_period,
                vesting_increment,
            } => Some(RpcAccountTiming {
                initial_minimum_balance: initial_minimum_balance.as_u64(),
                cliff_time: cliff_time.as_u32(),
                cliff_amount: cliff_amount.as_u64(),
                vesting_period: vesting_period.as_u32(),
                vesting_increment: vesting_increment.as_u64(),
            }),
        };
        let zkapp = account.zkapp.as_ref().map(|z| RpcAccountZkapp {
            app_state: z.app_state.iter().map(|f| f.to_decimal()).collect(),
            verification_key_hash: z.verification_key.as_ref().map(|vk| vk.hash().to_decimal()),
            zkapp_version: z.zkapp_version,
            proved_state: z.proved_state,
            zkapp_uri: String::from_utf8_lossy(&z.zkapp_uri).into_owned(),
        });
        Self {
            public_key: account.public_key.clone().into(),
            token_id: TokenIdKeyHash::from(account.token_id.clone()).to_string(),
            token_symbol: String::from_utf8_lossy(&account.token_symbol.0).into_owned(),
            balance: account.balance.as_u64(),
            nonce: account.nonce.as_u32(),
            delegate: account.delegate.clone().map(Into::into),
            timing,
            zkapp,
        }
    }
}
