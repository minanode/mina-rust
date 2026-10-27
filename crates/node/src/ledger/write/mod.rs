mod ledger_write_actions;
use ark_ff::BigInteger256;
use ledger::{
    scan_state::transaction_logic::{
        valid,
        zkapp_command::{AccessedOrNot, AccountUpdate, CallForest, ZkAppCommand},
        UserCommand, WithStatus,
    },
    staged_ledger::{diff::Diff, staged_ledger::StagedLedger},
    Account, AccountId, AccountIdOrderable, AccountIndex, BaseLedger, TokenId,
};
pub use ledger_write_actions::*;
use mina_signer::CompressedPubKey;

mod ledger_write_state;
pub use ledger_write_state::*;
use mina_core::block::AppliedBlock;

mod ledger_write_reducer;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use ledger::scan_state::scan_state::{transaction_snark::OneOrTwo, AvailableJobMessage};
use mina_p2p_messages::v2::{self, StateBodyHash};
use serde::{Deserialize, Serialize};

use crate::{
    block_producer_effectful::StagedLedgerDiffCreateOutput,
    core::{
        block::ArcBlockWithHash,
        snark::{Snark, SnarkJobId},
    },
    transition_frontier::sync::{
        ledger::staged::StagedLedgerAuxAndPendingCoinbasesValid,
        TransitionFrontierRootSnarkedLedgerUpdates,
    },
};

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Copy)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum LedgerWriteKind {
    StagedLedgerReconstruct,
    StagedLedgerDiffCreate,
    BlockApply,
    Commit,
    PersistCheckpoint,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum LedgerWriteRequest {
    StagedLedgerReconstruct {
        snarked_ledger_hash: v2::LedgerHash,
        parts: Option<Arc<StagedLedgerAuxAndPendingCoinbasesValid>>,
    },
    StagedLedgerDiffCreate {
        pred_block: AppliedBlock,
        global_slot_since_genesis: v2::MinaNumbersGlobalSlotSinceGenesisMStableV1,
        is_new_epoch: bool,
        producer: v2::NonZeroCurvePoint,
        delegator: v2::NonZeroCurvePoint,
        coinbase_receiver: v2::NonZeroCurvePoint,
        completed_snarks: BTreeMap<SnarkJobId, Snark>,
        supercharge_coinbase: bool,
        transactions_by_fee: Vec<valid::UserCommand>,
    },
    BlockApply {
        block: ArcBlockWithHash,
        pred_block: AppliedBlock,
        skip_verification: bool,
        /// A block a frontier store restore replays (I5d1): a staged ledger
        /// hash mismatch is an error, not a panic.
        #[serde(default)]
        restored: bool,
    },
    Commit {
        root_history: crate::transition_frontier::root_history::RootHistory,
        ledgers_to_keep: LedgersToKeep,
        root_snarked_ledger_updates: TransitionFrontierRootSnarkedLedgerUpdates,
        needed_protocol_states: BTreeMap<v2::StateHash, v2::MinaStateProtocolStateValueStableV2>,
        new_root: AppliedBlock,
        new_best_tip: AppliedBlock,
    },
    /// Snapshot the root's ledgers and parts for a frontier store checkpoint
    /// (R20 D3), right after a commit, and hand them to the store thread.
    PersistCheckpoint(Box<crate::ledger::PersistCheckpointRequest>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum LedgerWriteResponse {
    StagedLedgerReconstruct {
        staged_ledger_hash: v2::LedgerHash,
        result: Result<(), String>,
    },
    StagedLedgerDiffCreate {
        pred_block_hash: v2::StateHash,
        global_slot_since_genesis: v2::MinaNumbersGlobalSlotSinceGenesisMStableV1,
        result: Result<Arc<StagedLedgerDiffCreateOutput>, String>,
    },
    BlockApply {
        block_hash: v2::StateHash,
        result: Result<BlockApplyResult, String>,
    },
    Commit {
        best_tip_hash: v2::StateHash,
        result: CommitResult,
    },
    /// The snapshot was taken and the job handed to the store thread, whose
    /// result comes later as an event.
    PersistCheckpoint {
        root: v2::StateHash,
        result: Result<crate::persist::event::PersistSnapshotInfo, String>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlockApplyResult {
    pub block: ArcBlockWithHash,
    pub just_emitted_a_proof: bool,
    pub archive_data: Option<BlockApplyResultArchive>,
}

/// What the archive and precomputed blocks need from applying a block, in
/// OCaml's order.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlockApplyResultArchive {
    /// The accounts the block's transactions accessed, as they are after it,
    /// by account id.
    pub accounts_accessed: Vec<(AccountIndex, Account)>,
    /// With the account creation fee.
    pub accounts_created: Vec<(AccountId, u64)>,
    /// Every token of an account the block's transactions refer to, accessed
    /// or not, by token id, with its owner.
    pub tokens_used: Vec<(TokenId, Option<AccountId>)>,
    /// The fee payers of the block's user commands, with their receipt chain
    /// hashes after the block, by account id.
    pub sender_receipt_chains_from_parent_ledger: Vec<(AccountId, v2::ReceiptChainHash)>,
}

impl BlockApplyResultArchive {
    /// Computes the archive data of `block` from `staged_ledger`, the staged
    /// ledger after applying it, and the accounts applying it created.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_block/precomputed_block.ml#L101-L185>
    /// and <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/archive/lib/diff.ml#L47-L130>
    pub fn new(
        block: &ArcBlockWithHash,
        staged_ledger: &StagedLedger,
        accounts_created: &[AccountId],
    ) -> Result<Self, String> {
        let constraint_constants = crate::core::constants::constraint_constants();
        let ledger = staged_ledger.ledger();

        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_block/block.ml#L109-L115>
        let consensus_state = block.consensus_state();
        let coinbase_receiver: CompressedPubKey = (&consensus_state.coinbase_receiver)
            .try_into()
            .map_err(|e| format!("{e:?}"))?;
        let diff: Diff = (&block.body().staged_ledger_diff)
            .try_into()
            .map_err(|e| format!("{e:?}"))?;
        let transactions = diff
            .get_transactions(
                constraint_constants,
                coinbase_receiver,
                consensus_state.supercharge_coinbase,
            )
            .map_err(|e| format!("{e:?}"))?;
        let mut account_ids_accessed: Vec<(AccountIdOrderable, bool)> = transactions
            .iter()
            .flat_map(|WithStatus { data, status }| data.account_access_statuses(status))
            .map(|(id, status)| (id.into(), status == AccessedOrNot::Accessed))
            .collect();
        account_ids_accessed.sort();
        account_ids_accessed.dedup();

        let accounts_accessed = account_ids_accessed
            .iter()
            .filter(|(_, accessed)| *accessed)
            .filter_map(|(id, _)| {
                // An accessed account may not be in the ledger: OCaml skips it.
                let index = ledger.index_of_account(id.clone().into())?;
                let account = ledger.get_at_index(index)?;
                Some((index, *account))
            })
            .collect();

        let accounts_created = accounts_created
            .iter()
            .map(|id| (id.clone(), constraint_constants.account_creation_fee))
            .collect();

        // A token is used whatever the status of the transaction using it.
        let mut tokens: Vec<_> = account_ids_accessed
            .into_iter()
            .map(|(id, _)| AccountId::from(id).token_id)
            .map(|token_id| (BigInteger256::from(token_id.0), token_id))
            .collect();
        tokens.sort_by(|(a, _), (b, _)| a.cmp(b));
        tokens.dedup_by(|(a, _), (b, _)| a == b);
        let tokens_used = tokens
            .into_iter()
            .map(|(_, token_id)| {
                let owner = ledger.token_owner(token_id.clone());
                (token_id, owner)
            })
            .collect();

        let senders: BTreeSet<_> = block
            .body()
            .transactions()
            .filter_map(|tx| UserCommand::try_from(tx).ok().map(|cmd| cmd.fee_payer()))
            .map(AccountIdOrderable::from)
            .collect();
        let sender_receipt_chains_from_parent_ledger = senders
            .into_iter()
            .map(AccountId::from)
            .filter_map(|sender| {
                let account = ledger.get(ledger.location_of_account(&sender)?)?;
                Some((sender, account.receipt_chain_hash.into()))
            })
            .collect();

        Ok(Self {
            accounts_accessed,
            accounts_created,
            tokens_used,
            sender_receipt_chains_from_parent_ledger,
        })
    }
}

impl BlockApplyResultArchive {
    fn v2_accounts_accessed(
        &self,
    ) -> mina_p2p_messages::list::List<(
        mina_p2p_messages::number::UInt64,
        v2::MinaBaseAccountBinableArgStableV2,
    )> {
        self.accounts_accessed
            .iter()
            .map(|(index, account)| (index.0.into(), account.into()))
            .collect()
    }

    fn v2_accounts_created(
        &self,
    ) -> mina_p2p_messages::list::List<(v2::MinaBaseAccountIdStableV2, v2::CurrencyFeeStableV1)>
    {
        self.accounts_created
            .iter()
            .map(|(account_id, fee)| {
                (
                    account_id.clone().into(),
                    v2::CurrencyFeeStableV1((*fee).into()),
                )
            })
            .collect()
    }

    fn v2_tokens_used(
        &self,
    ) -> mina_p2p_messages::list::List<(
        v2::MinaBaseTokenIdStableV2,
        Option<v2::MinaBaseAccountIdStableV2>,
    )> {
        self.tokens_used
            .iter()
            .map(|(token_id, owner)| (token_id.into(), owner.clone().map(Into::into)))
            .collect()
    }
}

impl TryFrom<BlockApplyResult> for v2::ArchiveTransitionFrontierDiff {
    type Error = String;

    fn try_from(value: BlockApplyResult) -> Result<Self, Self::Error> {
        (&value).try_into()
    }
}

impl TryFrom<&BlockApplyResult> for v2::ArchiveTransitionFrontierDiff {
    type Error = String;

    fn try_from(value: &BlockApplyResult) -> Result<Self, Self::Error> {
        let archive_data = value.archive_data()?;
        Ok(Self::BreadcrumbAdded {
            // TODO(adonagy): check if we need the StateBodyHash, if no keep the None
            block: (
                (*value.block.block).clone(),
                (
                    value
                        .block
                        .header()
                        .protocol_state
                        .body
                        .try_hash()
                        .ok()
                        .map(StateBodyHash::from),
                    value.block.hash().clone(),
                ),
            ),
            accounts_accessed: archive_data.v2_accounts_accessed(),
            accounts_created: archive_data.v2_accounts_created(),
            tokens_used: archive_data.v2_tokens_used(),
            sender_receipt_chains_from_parent_ledger: archive_data
                .sender_receipt_chains_from_parent_ledger
                .iter()
                .map(|(account_id, receipt_chain_hash)| {
                    (
                        account_id.clone().into(),
                        receipt_chain_hash.clone().into_inner(),
                    )
                })
                .collect(),
        })
    }
}

/// The hashes precomputed blocks carry, computed by the ledger.
pub struct LedgerHashes;

impl v2::precomputed::PrecomputedHashes for LedgerHashes {
    fn verification_key(
        &self,
        vk: &v2::MinaBaseVerificationKeyWireStableV1,
    ) -> Result<mina_p2p_messages::bigint::BigInt, String> {
        let vk = ledger::VerificationKey::try_from(vk).map_err(|e| format!("{e:?}"))?;
        Ok(vk.hash().into())
    }

    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_block/legacy_format.ml#L50-L62>
    fn zkapp_command(
        &self,
        command: &v2::MinaBaseZkappCommandTStableV1WireStableV1,
    ) -> Result<
        Vec<(
            mina_p2p_messages::bigint::BigInt,
            mina_p2p_messages::bigint::BigInt,
        )>,
        String,
    > {
        fn collect(
            forest: &CallForest<AccountUpdate>,
            out: &mut Vec<(
                mina_p2p_messages::bigint::BigInt,
                mina_p2p_messages::bigint::BigInt,
            )>,
        ) -> Result<(), String> {
            for item in &forest.0 {
                let digest = item.elt.account_update_digest.get();
                let stack_hash = item.stack_hash.get();
                let (Some(digest), Some(stack_hash)) = (digest, stack_hash) else {
                    return Err("account update hashes not computed".to_owned());
                };
                out.push((digest.into(), stack_hash.into()));
                collect(&item.elt.calls, out)?;
            }
            Ok(())
        }
        let command = ZkAppCommand::try_from(command).map_err(|e| format!("{e:?}"))?;
        command.account_updates.accumulate_hashes();
        let mut out = Vec::new();
        collect(&command.account_updates, &mut out)?;
        Ok(out)
    }
}

impl BlockApplyResult {
    fn archive_data(&self) -> Result<&BlockApplyResultArchive, String> {
        self.archive_data
            .as_ref()
            .ok_or_else(|| "Archive data not available, not running in archive mode".to_owned())
    }

    /// The precomputed block OCaml writes for this block, `scheduled_time`
    /// being when it is written (OCaml's `Block_time.now`), not the block's
    /// timestamp.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_block/precomputed_block.ml#L173-L185>
    /// and <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_subscriptions.ml#L139-L153>
    pub fn precomputed_block(
        &self,
        scheduled_time: v2::BlockTimeTimeStableV1,
    ) -> Result<v2::PrecomputedBlock, String> {
        let archive_data = self.archive_data()?;
        let header = self.block.header();
        Ok(v2::PrecomputedBlock {
            scheduled_time,
            protocol_state: header.protocol_state.clone(),
            protocol_state_proof: header.protocol_state_proof.as_ref().clone().into(),
            staged_ledger_diff: self.block.body().staged_ledger_diff.clone(),
            delta_transition_chain_proof: header.delta_block_chain_proof.clone(),
            protocol_version: header.current_protocol_version.clone(),
            proposed_protocol_version: header.proposed_protocol_version_opt.clone(),
            accounts_accessed: archive_data.v2_accounts_accessed(),
            accounts_created: archive_data.v2_accounts_created(),
            tokens_used: archive_data.v2_tokens_used(),
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Ord, PartialOrd, Eq, PartialEq, Default, Clone)]
pub struct LedgersToKeep {
    snarked: BTreeSet<v2::LedgerHash>,
    staged: BTreeSet<Arc<v2::MinaBaseStagedLedgerHashStableV1>>,
}

impl LedgersToKeep {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains<'a, T>(&self, key: T) -> bool
    where
        T: 'a + Into<LedgerToKeep<'a>>,
    {
        match key.into() {
            LedgerToKeep::Snarked(hash) => self.snarked.contains(hash),
            LedgerToKeep::Staged(hash) => self.staged.contains(hash),
        }
    }

    pub fn add_snarked(&mut self, hash: v2::LedgerHash) -> bool {
        self.snarked.insert(hash)
    }

    pub fn add_staged(&mut self, hash: Arc<v2::MinaBaseStagedLedgerHashStableV1>) -> bool {
        self.staged.insert(hash)
    }
}

impl<'a> FromIterator<&'a ArcBlockWithHash> for LedgersToKeep {
    fn from_iter<T: IntoIterator<Item = &'a ArcBlockWithHash>>(iter: T) -> Self {
        let mut res = Self::new();
        let mut root = None;
        let best_tip = iter.into_iter().fold(None, |best_tip, block| {
            if root.is_none() {
                root = Some(block);
            }
            res.add_snarked(block.snarked_ledger_hash().clone());
            res.add_staged(Arc::new(block.staged_ledger_hashes().clone()));
            match best_tip {
                None => Some(block),
                Some(tip) if tip.height() < block.height() => Some(block),
                old => old,
            }
        });

        if let Some(best_tip) = best_tip {
            res.add_snarked(best_tip.staking_epoch_ledger_hash().clone());
            res.add_snarked(best_tip.next_epoch_ledger_hash().clone());
        }
        if let Some(root) = root {
            // OCaml: https://github.com/MinaProtocol/mina/blob/4.0.2/src/lib/consensus/proof_of_stake.ml#L3030-L3079
            // frontier_root_transition keeps the old epoch ledgers until the
            // frontier root crosses the boundary.
            res.add_snarked(root.staking_epoch_ledger_hash().clone());
            res.add_snarked(root.next_epoch_ledger_hash().clone());
        }

        res
    }
}

#[derive(derive_more::From)]
pub enum LedgerToKeep<'a> {
    Snarked(&'a v2::LedgerHash),
    Staged(&'a v2::MinaBaseStagedLedgerHashStableV1),
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct CommitResult {
    pub alive_masks: usize,
    pub available_jobs: Arc<Vec<OneOrTwo<AvailableJobMessage>>>,
    pub needed_protocol_states: BTreeSet<v2::StateHash>,
}

impl LedgerWriteRequest {
    pub fn kind(&self) -> LedgerWriteKind {
        match self {
            Self::StagedLedgerReconstruct { .. } => LedgerWriteKind::StagedLedgerReconstruct,
            Self::StagedLedgerDiffCreate { .. } => LedgerWriteKind::StagedLedgerDiffCreate,
            Self::BlockApply { .. } => LedgerWriteKind::BlockApply,
            Self::Commit { .. } => LedgerWriteKind::Commit,
            Self::PersistCheckpoint(_) => LedgerWriteKind::PersistCheckpoint,
        }
    }
}

impl LedgerWriteResponse {
    pub fn kind(&self) -> LedgerWriteKind {
        match self {
            Self::StagedLedgerReconstruct { .. } => LedgerWriteKind::StagedLedgerReconstruct,
            Self::StagedLedgerDiffCreate { .. } => LedgerWriteKind::StagedLedgerDiffCreate,
            Self::BlockApply { .. } => LedgerWriteKind::BlockApply,
            Self::Commit { .. } => LedgerWriteKind::Commit,
            Self::PersistCheckpoint { .. } => LedgerWriteKind::PersistCheckpoint,
        }
    }
}
