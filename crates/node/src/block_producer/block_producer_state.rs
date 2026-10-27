use std::{collections::BTreeSet, sync::Arc, time::Duration};

use ledger::scan_state::transaction_logic::valid;
use mina_core::{
    block::{AppliedBlock, ArcBlockWithHash},
    consensus::consensus_take,
};
use mina_p2p_messages::v2::{
    self, ConsensusProofOfStakeDataConsensusStateValueStableV2 as MinaConsensusState,
};
use serde::{Deserialize, Serialize};

use crate::account::AccountPublicKey;

use super::{
    vrf_evaluator::BlockProducerVrfEvaluatorState, BlockProducerConfig, BlockProducerWonSlot,
    BlockWithoutProof,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlockProducerState(Option<BlockProducerEnabled>);

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlockProducerEnabled {
    pub config: BlockProducerConfig,
    pub vrf_evaluator: BlockProducerVrfEvaluatorState,
    pub current: BlockProducerCurrentState,
    /// Timing and the consensus point captured together by the producer.
    #[serde(default)]
    pub next_producer_timing: Option<crate::rpc::RpcNextBlockProduction>,
    /// Blocks that were injected into transition frontier, but hasn't
    /// become our best tip yet.
    pub injected_blocks: BTreeSet<v2::StateHash>,
    #[serde(default)]
    pub proof_retried: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BlockProducerCurrentState {
    Idle {
        time: redux::Timestamp,
    },
    WonSlotDiscarded {
        time: redux::Timestamp,
        won_slot: BlockProducerWonSlot,
        reason: BlockProducerWonSlotDiscardReason,
    },
    WonSlot {
        time: redux::Timestamp,
        won_slot: BlockProducerWonSlot,
    },
    WonSlotWait {
        time: redux::Timestamp,
        won_slot: BlockProducerWonSlot,
    },
    WonSlotProduceInit {
        time: redux::Timestamp,
        won_slot: BlockProducerWonSlot,
        /// Chain that we are extending.
        chain: Vec<AppliedBlock>,
    },
    WonSlotTransactionsGet {
        time: redux::Timestamp,
        won_slot: BlockProducerWonSlot,
        /// Chain that we are extending.
        chain: Vec<AppliedBlock>,
    },
    WonSlotTransactionsSuccess {
        time: redux::Timestamp,
        won_slot: BlockProducerWonSlot,
        /// Chain that we are extending.
        chain: Vec<AppliedBlock>,
        transactions_by_fee: Vec<valid::UserCommand>,
    },
    StagedLedgerDiffCreatePending {
        time: redux::Timestamp,
        won_slot: BlockProducerWonSlot,
        /// Chain that we are extending.
        chain: Vec<AppliedBlock>,
        transactions_by_fee: Vec<valid::UserCommand>,
    },
    StagedLedgerDiffCreateSuccess {
        time: redux::Timestamp,
        won_slot: BlockProducerWonSlot,
        /// Chain that we are extending.
        chain: Vec<AppliedBlock>,
        diff: v2::StagedLedgerDiffDiffStableV2,
        /// `protocol_state.blockchain_state.body_reference`
        diff_hash: v2::ConsensusBodyReferenceStableV1,
        staged_ledger_hash: v2::MinaBaseStagedLedgerHashStableV1,
        emitted_ledger_proof: Option<Arc<v2::LedgerProofProdStableV2>>,
        pending_coinbase_update: v2::MinaBasePendingCoinbaseUpdateStableV1,
        pending_coinbase_witness: v2::MinaBasePendingCoinbaseWitnessStableV2,
        stake_proof_sparse_ledger: v2::MinaBaseSparseLedgerBaseStableV2,
    },
    BlockUnprovenBuilt {
        time: redux::Timestamp,
        won_slot: BlockProducerWonSlot,
        /// Chain that we are extending.
        chain: Vec<AppliedBlock>,
        emitted_ledger_proof: Option<Arc<v2::LedgerProofProdStableV2>>,
        pending_coinbase_update: v2::MinaBasePendingCoinbaseUpdateStableV1,
        pending_coinbase_witness: v2::MinaBasePendingCoinbaseWitnessStableV2,
        stake_proof_sparse_ledger: v2::MinaBaseSparseLedgerBaseStableV2,
        block: BlockWithoutProof,
        block_hash: v2::StateHash,
    },
    BlockProvePending {
        time: redux::Timestamp,
        won_slot: BlockProducerWonSlot,
        /// Chain that we are extending.
        chain: Vec<AppliedBlock>,
        emitted_ledger_proof: Option<Arc<v2::LedgerProofProdStableV2>>,
        pending_coinbase_update: v2::MinaBasePendingCoinbaseUpdateStableV1,
        pending_coinbase_witness: v2::MinaBasePendingCoinbaseWitnessStableV2,
        stake_proof_sparse_ledger: v2::MinaBaseSparseLedgerBaseStableV2,
        block: BlockWithoutProof,
        block_hash: v2::StateHash,
        #[serde(default)]
        verification_req_id: Option<mina_snark::block_verify::SnarkBlockVerifyId>,
    },
    BlockProveSuccess {
        time: redux::Timestamp,
        won_slot: BlockProducerWonSlot,
        /// Chain that we are extending.
        chain: Vec<AppliedBlock>,
        block: BlockWithoutProof,
        block_hash: v2::StateHash,
        proof: Arc<v2::MinaBaseProofStableV2>,
    },
    Produced {
        time: redux::Timestamp,
        won_slot: BlockProducerWonSlot,
        /// Chain that we are extending.
        chain: Vec<AppliedBlock>,
        block: ArcBlockWithHash,
    },
    Injected {
        time: redux::Timestamp,
        won_slot: BlockProducerWonSlot,
        /// Chain that we are extending.
        chain: Vec<AppliedBlock>,
        block: ArcBlockWithHash,
    },
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Copy)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum BlockProducerWonSlotDiscardReason {
    BestTipStakingLedgerDifferent,
    BestTipGlobalSlotHigher,
    BestTipSuperior,
    ProofVerificationFailed,
    ProofGenerationFailed,
    /// The won slot ended before its block was started.
    WonSlotExpired,
}

impl BlockProducerState {
    pub fn new(now: redux::Timestamp, config: Option<BlockProducerConfig>) -> Self {
        Self(config.map(|config| BlockProducerEnabled {
            config: config.clone(),
            vrf_evaluator: BlockProducerVrfEvaluatorState::new(now),
            current: BlockProducerCurrentState::Idle { time: now },
            injected_blocks: Default::default(),
            next_producer_timing: None,
            proof_retried: false,
        }))
    }

    pub fn with<'a, F, R: 'a>(&'a self, default: R, fun: F) -> R
    where
        F: FnOnce(&'a BlockProducerEnabled) -> R,
    {
        self.0.as_ref().map_or(default, fun)
    }

    pub fn as_mut(&mut self) -> Option<&mut BlockProducerEnabled> {
        self.0.as_mut()
    }

    pub fn as_ref(&self) -> Option<&BlockProducerEnabled> {
        self.0.as_ref()
    }

    pub fn next_block_production(&self) -> Option<crate::rpc::RpcNextBlockProduction> {
        self.as_ref()?.next_producer_timing.clone()
    }

    pub fn is_enabled(&self) -> bool {
        self.0.is_some()
    }

    pub fn config(&self) -> Option<&BlockProducerConfig> {
        self.with(None, |this| Some(&this.config))
    }

    pub fn is_me(&self, producer: &v2::NonZeroCurvePoint) -> bool {
        self.with(false, |this| producer == &this.config.pub_key)
    }

    /// Checks if the block was produced by us recently.
    pub fn is_produced_by_me(&self, block: &ArcBlockWithHash) -> bool {
        self.with(false, |this| {
            block.producer() == &this.config.pub_key && this.injected_blocks.contains(block.hash())
        })
    }

    pub fn is_producing(&self) -> bool {
        self.with(false, |this| this.current.is_producing())
    }

    pub fn current_won_slot(&self) -> Option<&BlockProducerWonSlot> {
        self.with(None, |this| this.current.won_slot())
    }

    pub fn current_parent_chain(&self) -> Option<&[AppliedBlock]> {
        self.with(None, |this| this.current.parent_chain())
    }

    /// Won slot that we are in the middle of producing.
    pub fn producing_won_slot(&self) -> Option<&BlockProducerWonSlot> {
        self.current_won_slot().filter(|_| self.is_producing())
    }

    pub fn produced_block(&self) -> Option<&ArcBlockWithHash> {
        self.with(None, |this| this.current.produced_block())
    }

    pub fn produced_block_with_chain(&self) -> Option<(&ArcBlockWithHash, &[AppliedBlock])> {
        self.with(None, |this| this.current.produced_block_with_chain())
    }

    pub fn vrf_evaluator(&self) -> Option<&BlockProducerVrfEvaluatorState> {
        self.with(None, |this| Some(&this.vrf_evaluator))
    }

    pub fn vrf_evaluator_with_config(
        &self,
    ) -> Option<(&BlockProducerVrfEvaluatorState, &BlockProducerConfig)> {
        self.with(None, |this| Some((&this.vrf_evaluator, &this.config)))
    }

    /// If we need to construct delegator table, get it's inputs.
    pub fn vrf_delegator_table_inputs(&self) -> Option<(&v2::LedgerHash, &AccountPublicKey)> {
        self.vrf_evaluator()?.vrf_delegator_table_inputs()
    }

    pub fn pending_transactions(&self) -> Vec<valid::UserCommand> {
        self.with(Vec::new(), |this| this.current.pending_transactions())
    }
}

impl BlockProducerEnabled {
    /// Record the timing when scheduling runs, not when status is queried.
    /// A best-tip update while waiting for a won slot must not rewrite the
    /// consensus point that generated that timing.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L1371-L1427>
    pub(crate) fn update_next_producer_timing(
        &mut self,
        best_tip: &ArcBlockWithHash,
        cur_global_slot: Option<u32>,
    ) {
        use crate::rpc::{RpcConsensusSlot, RpcNextBlockProduction};
        let consensus_slot = |global_slot, since_genesis| {
            RpcConsensusSlot::new(
                global_slot,
                since_genesis,
                best_tip
                    .curr_global_slot_since_hard_fork()
                    .slots_per_epoch
                    .as_u32(),
                u64::from(best_tip.genesis_timestamp()) / 1_000_000,
                mina_core::constants::constraint_constants().block_window_duration_ms,
            )
        };
        let Some(generated_from_consensus_at) =
            consensus_slot(best_tip.global_slot(), best_tip.global_slot_since_genesis())
        else {
            return;
        };
        let won_slot = if self.current.won_slot_should_search() {
            cur_global_slot.and_then(|slot| self.next_won_slot(slot, best_tip))
        } else {
            self.current.won_slot().cloned()
        };
        let next_won_slot = won_slot.and_then(|won_slot| {
            consensus_slot(
                won_slot.global_slot(),
                won_slot
                    .global_slot_since_genesis(best_tip.global_slot_diff())
                    .as_u32(),
            )
        });
        self.next_producer_timing = Some(RpcNextBlockProduction {
            generated_from_consensus_at,
            next_won_slot,
        });
    }

    /// The next won slot to produce a block in, from `cur_global_slot` on,
    /// that `best_tip` may not beat, but for a won slot already tried
    /// ([`BlockProducerCurrentState::won_slot_search_start`]).
    pub fn next_won_slot(
        &self,
        cur_global_slot: u32,
        best_tip: &ArcBlockWithHash,
    ) -> Option<BlockProducerWonSlot> {
        let from = self.current.won_slot_search_start(cur_global_slot);
        self.vrf_evaluator.next_won_slot(from, best_tip)
    }
}

impl BlockProducerCurrentState {
    pub fn built_block_hash(&self) -> Option<&v2::StateHash> {
        match self {
            Self::BlockUnprovenBuilt { block_hash, .. }
            | Self::BlockProvePending { block_hash, .. }
            | Self::BlockProveSuccess { block_hash, .. } => Some(block_hash),
            Self::Produced { block, .. } | Self::Injected { block, .. } => Some(block.hash()),
            _ => None,
        }
    }

    pub fn won_slot_should_search(&self) -> bool {
        match self {
            Self::Idle { .. } | Self::WonSlotDiscarded { .. } | Self::Injected { .. } => true,
            Self::WonSlot { .. }
            | Self::WonSlotWait { .. }
            | Self::WonSlotProduceInit { .. }
            | Self::WonSlotTransactionsGet { .. }
            | Self::WonSlotTransactionsSuccess { .. }
            | Self::StagedLedgerDiffCreatePending { .. }
            | Self::StagedLedgerDiffCreateSuccess { .. }
            | Self::BlockUnprovenBuilt { .. }
            | Self::BlockProvePending { .. }
            | Self::BlockProveSuccess { .. }
            | Self::Produced { .. } => false,
        }
    }

    pub fn won_slot(&self) -> Option<&BlockProducerWonSlot> {
        match self {
            Self::Idle { .. } => None,
            Self::WonSlotDiscarded { won_slot, .. }
            | Self::WonSlot { won_slot, .. }
            | Self::WonSlotWait { won_slot, .. }
            | Self::WonSlotProduceInit { won_slot, .. }
            | Self::WonSlotTransactionsGet { won_slot, .. }
            | Self::WonSlotTransactionsSuccess { won_slot, .. }
            | Self::StagedLedgerDiffCreatePending { won_slot, .. }
            | Self::StagedLedgerDiffCreateSuccess { won_slot, .. }
            | Self::BlockUnprovenBuilt { won_slot, .. }
            | Self::BlockProvePending { won_slot, .. }
            | Self::BlockProveSuccess { won_slot, .. }
            | Self::Produced { won_slot, .. }
            | Self::Injected { won_slot, .. } => Some(won_slot),
        }
    }

    pub fn parent_chain(&self) -> Option<&[AppliedBlock]> {
        match self {
            Self::Idle { .. }
            | Self::WonSlotDiscarded { .. }
            | Self::WonSlot { .. }
            | Self::WonSlotWait { .. } => None,
            Self::WonSlotProduceInit { chain, .. }
            | Self::WonSlotTransactionsGet { chain, .. }
            | Self::WonSlotTransactionsSuccess { chain, .. }
            | Self::StagedLedgerDiffCreatePending { chain, .. }
            | Self::StagedLedgerDiffCreateSuccess { chain, .. }
            | Self::BlockUnprovenBuilt { chain, .. }
            | Self::BlockProvePending { chain, .. }
            | Self::BlockProveSuccess { chain, .. }
            | Self::Produced { chain, .. }
            | Self::Injected { chain, .. } => Some(chain),
        }
    }

    pub fn won_slot_should_wait(&self, now: redux::Timestamp) -> bool {
        matches!(self, Self::WonSlot { .. }) && !self.won_slot_should_produce(now)
    }

    pub fn won_slot_should_produce(&self, now: redux::Timestamp) -> bool {
        self.won_slot_is_due(now, Duration::ZERO)
    }

    /// Whether the won slot we wait for starts within `lead` from `now`,
    /// plus the time a block takes to produce, and has not ended: with no
    /// lead, it is time to start producing the block; with one, to start
    /// what must be ready by then, such as the genesis block's proof
    /// ([`crate::State::producing_block_after_genesis`]). The end does not
    /// depend on the lead, so a slot that has started stays due to its end.
    pub fn won_slot_is_due(&self, now: redux::Timestamp, lead: Duration) -> bool {
        // TODO(binier): maybe have runtime estimate
        #[cfg(not(target_arch = "wasm32"))]
        const BLOCK_PRODUCTION_ESTIMATE: u64 = Duration::from_secs(6).as_nanos() as u64;
        #[cfg(target_arch = "wasm32")]
        const BLOCK_PRODUCTION_ESTIMATE: u64 = Duration::from_secs(20).as_nanos() as u64;

        match self {
            Self::WonSlot { won_slot, .. } | Self::WonSlotWait { won_slot, .. } => {
                let slot_upper_bound = won_slot.next_slot_time();
                let lead = u64::try_from(lead.as_nanos()).expect("overflow");
                let estimated_produced_time = now
                    .checked_add(BLOCK_PRODUCTION_ESTIMATE)
                    .and_then(|time| time.checked_add(lead))
                    .expect("overflow");
                estimated_produced_time >= won_slot.slot_time && now < slot_upper_bound
            }
            _ => false,
        }
    }

    /// The global slot to search the next won slot from, at `cur_global_slot`:
    /// that one, or the one after a won slot already tried, whose block we
    /// injected, could not prove, found beaten or did not start in time. As a
    /// best tip of its own slot no longer rules it out, such a slot would
    /// otherwise be found again while it lasts, built and proved again. OCaml
    /// takes each won slot off a queue, so it tries a slot once, whatever
    /// comes of it.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L1106-L1191>
    pub fn won_slot_search_start(&self, cur_global_slot: u32) -> u32 {
        match self {
            Self::WonSlotDiscarded {
                won_slot,
                reason:
                    BlockProducerWonSlotDiscardReason::ProofGenerationFailed
                    | BlockProducerWonSlotDiscardReason::ProofVerificationFailed
                    | BlockProducerWonSlotDiscardReason::BestTipSuperior
                    | BlockProducerWonSlotDiscardReason::WonSlotExpired,
                ..
            }
            | Self::Injected { won_slot, .. } => {
                cur_global_slot.max(won_slot.global_slot().saturating_add(1))
            }
            _ => cur_global_slot,
        }
    }

    /// Whether the won slot we have yet to start a block for has ended.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L1174-L1191>
    pub fn won_slot_has_expired(&self, now: redux::Timestamp) -> bool {
        match self {
            Self::WonSlot { won_slot, .. } | Self::WonSlotWait { won_slot, .. } => {
                now >= won_slot.next_slot_time()
            }
            _ => false,
        }
    }

    /// The consensus state and hash of the block we make, once it is built.
    pub fn candidate_block(&self) -> Option<(&MinaConsensusState, &v2::StateHash)> {
        match self {
            Self::BlockUnprovenBuilt {
                block, block_hash, ..
            }
            | Self::BlockProvePending {
                block, block_hash, ..
            }
            | Self::BlockProveSuccess {
                block, block_hash, ..
            } => Some((&block.protocol_state.body.consensus_state, block_hash)),
            Self::Produced { block, .. } => Some((block.consensus_state(), block.hash())),
            _ => None,
        }
    }

    /// Why to give up the won slot at `now`, with `best_tip`, if we are to.
    ///
    /// Whether our block would beat the best tip is only known once its
    /// consensus state is: until then neither the slot nor the VRF output
    /// rules it out. A same-slot best tip with a greater VRF output can be
    /// shorter than our block will be, and with an equal one, from a node
    /// sharing our key, the state hash decides. OCaml makes the block whatever
    /// the best tip, on the parent of a best tip of the same slot, and then
    /// selects. A later best tip still ends the slot, as gossip of its block
    /// would be too late, and so does a staking ledger we did not win with.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L715-L747>
    pub fn won_slot_should_discard(
        &self,
        best_tip: &ArcBlockWithHash,
        now: redux::Timestamp,
    ) -> Option<BlockProducerWonSlotDiscardReason> {
        if matches!(self, Self::WonSlotDiscarded { .. }) {
            return None;
        }

        let won_slot = self.won_slot()?;
        if self.won_slot_has_expired(now) {
            return Some(BlockProducerWonSlotDiscardReason::WonSlotExpired);
        }

        if won_slot.global_slot() < best_tip.global_slot() {
            return Some(BlockProducerWonSlotDiscardReason::BestTipGlobalSlotHigher);
        }

        if &won_slot.staking_ledger_hash != best_tip.staking_epoch_ledger_hash()
            && &won_slot.staking_ledger_hash != best_tip.next_epoch_ledger_hash()
        {
            return Some(BlockProducerWonSlotDiscardReason::BestTipStakingLedgerDifferent);
        }

        if self
            .candidate_block()
            .is_some_and(|(consensus_state, hash)| {
                !consensus_take(
                    best_tip.consensus_state(),
                    consensus_state,
                    best_tip.hash(),
                    hash,
                )
            })
        {
            return Some(BlockProducerWonSlotDiscardReason::BestTipSuperior);
        }

        None
    }

    pub fn is_producing(&self) -> bool {
        match self {
            Self::Idle { .. }
            | Self::WonSlotDiscarded { .. }
            | Self::WonSlot { .. }
            | Self::WonSlotWait { .. }
            | Self::Injected { .. } => false,
            Self::WonSlotProduceInit { .. }
            | Self::WonSlotTransactionsGet { .. }
            | Self::WonSlotTransactionsSuccess { .. }
            | Self::StagedLedgerDiffCreatePending { .. }
            | Self::StagedLedgerDiffCreateSuccess { .. }
            | Self::BlockUnprovenBuilt { .. }
            | Self::BlockProvePending { .. }
            | Self::BlockProveSuccess { .. }
            | Self::Produced { .. } => true,
        }
    }

    pub fn produced_block(&self) -> Option<&ArcBlockWithHash> {
        match self {
            Self::Produced { block, .. } => Some(block),
            _ => None,
        }
    }

    pub fn injected_block(&self) -> Option<&ArcBlockWithHash> {
        match self {
            Self::Injected { block, .. } => Some(block),
            _ => None,
        }
    }

    pub fn produced_block_with_chain(&self) -> Option<(&ArcBlockWithHash, &[AppliedBlock])> {
        match self {
            Self::Produced { chain, block, .. } => Some((block, chain)),
            _ => None,
        }
    }

    pub fn pending_transactions(&self) -> Vec<valid::UserCommand> {
        match self {
            Self::WonSlotTransactionsSuccess {
                transactions_by_fee,
                ..
            }
            | Self::StagedLedgerDiffCreatePending {
                transactions_by_fee,
                ..
            } => transactions_by_fee.to_vec(),
            _ => vec![],
        }
    }
}

impl Default for BlockProducerCurrentState {
    fn default() -> Self {
        Self::Idle {
            time: redux::Timestamp::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use mina_p2p_messages::{bigint::BigInt, v2};

    use super::*;
    use crate::account::AccountSecretKey;

    fn won_slot_at(slot_time: redux::Timestamp) -> BlockProducerCurrentState {
        let seed = v2::EpochSeed::from(v2::MinaBaseEpochSeedStableV1(BigInt::zero()));
        let won_slot = BlockProducerWonSlot {
            slot_time,
            delegator: (
                AccountSecretKey::genesis_producer().public_key().into(),
                ledger::AccountIndex(0),
            ),
            global_slot: v2::ConsensusGlobalSlotStableV1 {
                slot_number: v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(
                    10u32.into(),
                ),
                slots_per_epoch: 7140.into(),
            },
            vrf_output: Box::new(mina_vrf::genesis_vrf(seed).unwrap()),
            value_with_threshold: None,
            staking_ledger_hash: v2::LedgerHash::zero(),
        };
        BlockProducerCurrentState::WonSlot {
            time: slot_time,
            won_slot,
        }
    }

    /// A won slot is produced from shortly before it starts until it ends,
    /// one block window later: 90 s on Mesa, where the producer used to allow
    /// 180 s, so it could make a block for a slot that had passed.
    #[test]
    fn won_slot_is_produced_within_its_slot() {
        let window_ms = mina_core::constants::constraint_constants().block_window_duration_ms;
        assert_eq!(window_ms, 90_000);
        let ms = |ms: u64| ms * 1_000_000;
        let start = redux::Timestamp::new(ms(1_000_000_000));
        let state = won_slot_at(start);
        let at = |offset_ms: i64| {
            let ns = u64::from(start) as i64 + offset_ms * 1_000_000;
            state.won_slot_should_produce(redux::Timestamp::new(ns as u64))
        };

        assert!(!at(-60_000));
        assert!(at(-1_000), "within the production estimate before the slot");
        assert!(at(0));
        assert!(at(window_ms as i64 - 1));
        assert!(!at(window_ms as i64), "the slot is over");
        assert!(!at(150_000), "the slot is over, though 180 s are not");
        assert!(state.won_slot_should_wait(redux::Timestamp::new(u64::from(start) - ms(60_000))));
    }
}
