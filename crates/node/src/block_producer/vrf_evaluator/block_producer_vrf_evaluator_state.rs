use std::{collections::BTreeMap, sync::Arc};

use mina_core::block::ArcBlockWithHash;
use mina_p2p_messages::v2;
use serde::{Deserialize, Serialize};

use crate::{account::AccountPublicKey, block_producer::BlockProducerWonSlot};

use super::{DelegatorTable, EpochFingerprint, VrfEvaluatorInput, VrfWonSlotWithHash};

pub const SLOTS_PER_EPOCH: u32 = 7140;

/// How long to wait before asking again for a staking ledger that was not
/// available, when no best tip update asks first.
pub const UNAVAILABLE_LEDGER_RETRY: std::time::Duration = std::time::Duration::from_secs(15);

/// Completed evaluations kept, by epoch: enough for a reorg back across an
/// epoch boundary.
const COMPLETED_EPOCHS_KEPT: usize = 4;
/// Vrf evaluator sub-state
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlockProducerVrfEvaluatorState {
    pub status: BlockProducerVrfEvaluatorStatus,
    pub won_slots: BTreeMap<u32, VrfWonSlotWithHash>,
    #[serde(default)]
    pub total_won_slots: u64,
    pub latest_evaluated_slot: u32,
    pub genesis_timestamp: redux::Timestamp,
    last_evaluated_epoch: Option<u32>,
    pending_evaluation: Option<PendingEvaluation>,
    epoch_context: EpochContext,
    /// The epoch data each completed evaluation used. An epoch counts as
    /// evaluated only for exactly this data: a tip selecting other data for
    /// it starts the evaluation again.
    #[serde(default)]
    completed_epochs: BTreeMap<u32, EpochFingerprint>,
    /// The evaluation being set up or run, from its selection until it
    /// finishes or is interrupted.
    #[serde(default)]
    target: Option<EpochFingerprint>,
    /// Increases with each selected evaluation and each interruption. VRF
    /// responses carry the generation of their request.
    #[serde(default)]
    generation: u64,
    /// The clock's epoch at the last readiness check.
    #[serde(default)]
    checked_clock_epoch: Option<u32>,
    /// Since when the selected staking ledger has been unavailable, to ask
    /// again for it.
    #[serde(default)]
    ledger_unavailable_since: Option<redux::Timestamp>,
}

impl BlockProducerVrfEvaluatorState {
    pub fn new(now: redux::Timestamp) -> Self {
        Self {
            status: BlockProducerVrfEvaluatorStatus::Idle { time: now },
            won_slots: Default::default(),
            total_won_slots: 0,
            latest_evaluated_slot: Default::default(),
            genesis_timestamp: redux::Timestamp::ZERO,
            last_evaluated_epoch: Default::default(),
            pending_evaluation: Default::default(),
            epoch_context: EpochContext::Waiting,
            completed_epochs: Default::default(),
            target: None,
            generation: 0,
            checked_clock_epoch: None,
            ledger_unavailable_since: None,
        }
    }

    /// A fresh evaluator after a ledger recovery. Its generation continues
    /// from the replaced one, so a VRF response still in flight is rejected.
    pub fn reset(&self, now: redux::Timestamp) -> Self {
        Self {
            genesis_timestamp: self.genesis_timestamp,
            generation: self.generation.saturating_add(1),
            ..Self::new(now)
        }
    }

    /// Determines the position of a slot within an epoch (at the beginning, end, or within the epoch).
    /// This function calculates the position based on the `global_slot` and a predefined number of slots per epoch.
    ///
    /// Arguments:
    /// - `global_slot`: A reference to a 32-bit unsigned integer representing the global slot number.
    ///
    /// Returns:
    /// - `SlotPositionInEpoch`: An enum indicating the slot's position (Beginning, End, or Within).
    pub fn evaluate_epoch_bounds(global_slot: &u32) -> SlotPositionInEpoch {
        // OCaml Global_slot.slot uses remainder, including at UInt32.max_int.
        // src/lib/consensus/global_slot.ml (4.0.4).
        let slot = global_slot % SLOTS_PER_EPOCH;
        if slot == 0 {
            SlotPositionInEpoch::Beginning
        } else if slot == SLOTS_PER_EPOCH - 1 {
            SlotPositionInEpoch::End
        } else {
            SlotPositionInEpoch::Within
        }
    }

    /// Fetches the next slot that has been won based on the current global slot and the best tip information.
    /// If there are no future won slots, returns `None`.
    ///
    /// A won slot of the best tip's slot is skipped if the best tip's VRF
    /// output is greater: our block would be its sibling, as long, and lose.
    /// With an equal one, from a node sharing our key, the state hash decides
    /// once our block is made.
    ///
    /// Arguments:
    /// - `cur_global_slot`: The current global slot as a 32-bit unsigned integer.
    /// - `best_tip`: A reference to the `ArcBlockWithHash` representing the current best tip.
    ///
    /// Returns:
    /// - `Option<BlockProducerWonSlot>`: The next won slot, if any, as a `BlockProducerWonSlot` or `None` if there are no more slots won in the future.
    /// A win evaluated with epoch data other than the data the best tip
    /// selects for its epoch is skipped: no block extending the best tip can
    /// use it.
    pub fn next_won_slot(
        &self,
        cur_global_slot: u32,
        best_tip: &ArcBlockWithHash,
    ) -> Option<BlockProducerWonSlot> {
        self.won_slots_from(cur_global_slot, best_tip).next()
    }

    /// The wins [`Self::next_won_slot`] would choose from, in slot order.
    pub fn won_slots_from<'a>(
        &'a self,
        cur_global_slot: u32,
        best_tip: &'a ArcBlockWithHash,
    ) -> impl Iterator<Item = BlockProducerWonSlot> + 'a {
        // A block on the best tip, or on its parent for a win of the tip's
        // slot, takes the same staking data for the win's epoch.
        self.won_slots
            .range(cur_global_slot..)
            .map(|(_, won_slot)| {
                BlockProducerWonSlot::from_vrf_won_slot(won_slot, best_tip.genesis_timestamp())
            })
            .filter(|won_slot| {
                won_slot >= best_tip && won_slot.epoch_data_matches(best_tip.consensus_state())
            })
    }

    /// Retrieves the current epoch context.
    ///
    /// Returns:
    /// - `&EpochContext`: A reference to the `EpochContext`
    pub fn epoch_context(&self) -> &EpochContext {
        &self.epoch_context
    }

    /// Sets the epoch context based on the current evaluation status and the available data.
    /// This method updates the `epoch_context` field in the `ReadinessCheck` state
    pub fn set_epoch_context(&mut self) {
        // guard the epoch context change and permit it only if in the ReadinessCheck state
        if let BlockProducerVrfEvaluatorStatus::ReadinessCheck {
            best_tip_epoch,
            root_block_epoch,
            current_epoch,
            is_next_epoch_seed_finalized,
            staking_epoch_data,
            next_epoch_data,
            ..
        } = &self.status
        {
            self.epoch_context = self.select_epoch_context(
                *current_epoch,
                *best_tip_epoch,
                *root_block_epoch,
                *is_next_epoch_seed_finalized,
                (**staking_epoch_data).clone().into(),
                (**next_epoch_data).clone().into(),
            );
        }
    }

    /// The epoch to evaluate, with the data the best tip selects for it.
    ///
    /// Like OCaml, a clock in the epoch after the best tip's evaluates that
    /// epoch at once with the tip's next epoch data, and a clock further
    /// ahead has no data until a later tip arrives (OCaml exits). Within the
    /// tip's epoch, its staking data evaluates the epoch, and the next epoch
    /// is evaluated ahead only once its seed is final and the root has
    /// reached the tip's epoch. Any epoch already evaluated with exactly the
    /// selected data is not evaluated again; other data is.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/consensus/proof_of_stake.ml#L2919-L3020>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/block_producer/block_producer.ml#L1072-L1093>
    fn select_epoch_context(
        &self,
        current_epoch: Option<u32>,
        best_tip_epoch: u32,
        root_block_epoch: u32,
        is_next_epoch_seed_finalized: bool,
        staking: EpochData,
        next: EpochData,
    ) -> EpochContext {
        let Some(current_epoch) = current_epoch else {
            return EpochContext::Waiting;
        };
        let next_epoch = best_tip_epoch.checked_add(1);
        let next_pending = |epoch: u32| !self.is_evaluated(&EpochFingerprint::new(epoch, &next));
        if current_epoch > best_tip_epoch {
            return if next_epoch == Some(current_epoch) && next_pending(current_epoch) {
                EpochContext::Next(next)
            } else {
                EpochContext::Waiting
            };
        }
        if !self.is_evaluated(&EpochFingerprint::new(best_tip_epoch, &staking)) {
            EpochContext::Current(staking)
        } else if next_epoch.is_some_and(next_pending)
            && root_block_epoch == best_tip_epoch
            && is_next_epoch_seed_finalized
        {
            EpochContext::Next(next)
        } else {
            EpochContext::Waiting
        }
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.status, BlockProducerVrfEvaluatorStatus::Idle { .. })
    }

    pub fn is_initialized(&self) -> bool {
        !matches!(
            self.status,
            BlockProducerVrfEvaluatorStatus::Idle { .. }
                | BlockProducerVrfEvaluatorStatus::InitialisationPending { .. }
        )
    }

    /// Whether an evaluation of `epoch_data`'s epoch has completed with
    /// exactly this data.
    pub fn is_evaluated(&self, epoch_data: &EpochFingerprint) -> bool {
        self.completed_epochs.get(&epoch_data.epoch) == Some(epoch_data)
    }

    /// The epoch data of `epoch`'s completed evaluation, if any.
    pub fn completed_epoch(&self, epoch: u32) -> Option<&EpochFingerprint> {
        self.completed_epochs.get(&epoch)
    }

    /// Records an evaluation completed with `epoch_data`.
    pub fn mark_epoch_evaluated(&mut self, epoch_data: EpochFingerprint) {
        let epoch = epoch_data.epoch;
        self.completed_epochs.insert(epoch, epoch_data);
        while self.completed_epochs.len() > COMPLETED_EPOCHS_KEPT {
            self.completed_epochs.pop_first();
        }
        self.last_evaluated_epoch = Some(self.last_evaluated_epoch.map_or(epoch, |e| e.max(epoch)));
    }

    /// The evaluation being set up or run.
    pub fn target(&self) -> Option<&EpochFingerprint> {
        self.target.as_ref()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether the evaluation being set up or run can no longer produce a
    /// block: a block extending `best_tip` would use other data for its
    /// epoch, or the clock has passed it.
    pub fn target_is_stale(
        &self,
        best_tip: &mina_p2p_messages::v2::ConsensusProofOfStakeDataConsensusStateValueStableV2,
        current_epoch: Option<u32>,
    ) -> bool {
        self.target.as_ref().is_some_and(|target| {
            EpochFingerprint::select(best_tip, target.epoch).as_ref() != Some(target)
                || current_epoch.is_some_and(|epoch| epoch > target.epoch)
        })
    }

    /// Whether a readiness check is due without a best tip update: the clock
    /// entered another epoch, the clock passed the evaluated epoch, or a
    /// missing staking ledger is to be asked for again.
    pub fn recheck_due(&self, current_epoch: u32, now: redux::Timestamp) -> bool {
        self.is_initialized()
            && (self.checked_clock_epoch != Some(current_epoch)
                || self
                    .target
                    .as_ref()
                    .is_some_and(|t| t.epoch < current_epoch)
                || self.ledger_unavailable_since.is_some_and(|since| {
                    let retry = UNAVAILABLE_LEDGER_RETRY.as_nanos() as u64;
                    now >= since.checked_add(retry).unwrap_or(since)
                }))
    }

    pub fn set_checked_clock_epoch(&mut self, current_epoch: Option<u32>) {
        self.checked_clock_epoch = current_epoch;
    }

    /// Whether the last selected staking ledger was unavailable.
    pub fn is_ledger_unavailable(&self) -> bool {
        self.ledger_unavailable_since.is_some()
    }

    /// Selects `target` for evaluation: a new generation, and no completed
    /// evaluation or win of its epoch made with other data remains.
    pub fn select_target(&mut self, target: EpochFingerprint) {
        if self.completed_epochs.get(&target.epoch) != Some(&target) {
            self.completed_epochs.remove(&target.epoch);
        }
        let stale = self
            .won_slots
            .iter()
            .filter(|(slot, won)| {
                *slot / SLOTS_PER_EPOCH == target.epoch && won.epoch_data.as_ref() != Some(&target)
            })
            .map(|(slot, _)| *slot)
            .collect::<Vec<_>>();
        for slot in stale {
            self.won_slots.remove(&slot);
        }
        self.target = Some(target);
        self.generation = self.generation.saturating_add(1);
        self.ledger_unavailable_since = None;
    }

    /// Ends the evaluation being set up or run; in-flight responses are
    /// rejected from now on.
    pub fn clear_target(&mut self) {
        self.target = None;
        self.pending_evaluation = None;
        self.generation = self.generation.saturating_add(1);
    }

    /// The selected staking ledger is unavailable: nothing is evaluated
    /// until it is asked for again.
    pub fn set_ledger_unavailable(&mut self, now: redux::Timestamp) {
        self.clear_target();
        self.ledger_unavailable_since = Some(now);
    }

    /// Retrieves the number of the last evaluated epoch, if any.
    ///
    /// Returns:
    /// - `Option<u32>`: The epoch number of the last evaluated epoch, or `None` if no epoch has been evaluated yet.
    pub fn last_evaluated_epoch(&self) -> Option<u32> {
        self.last_evaluated_epoch
    }

    /// TODO: remove, not needed anymore
    pub fn latest_evaluated_global_slot(&self) -> u32 {
        self.latest_evaluated_slot
    }

    /// Returns `true` if the evaluator is in the `ReadinessCheck` state, otherwise `false`.
    pub fn is_readiness_check(&self) -> bool {
        matches!(
            self.status,
            BlockProducerVrfEvaluatorStatus::ReadinessCheck { .. }
        )
    }

    /// Returns `true` if the evaluator is in the `EpochBoundsCheck` state, otherwise `false`.
    pub fn is_epoch_bound_evaluated(&self) -> bool {
        matches!(
            self.status,
            BlockProducerVrfEvaluatorStatus::EpochBoundsCheck { .. }
        )
    }

    /// Returns true if the evaluator is in a state that can perform a `ReadinessCheck``, otherwise `false`.
    pub fn can_check_next_evaluation(&self) -> bool {
        matches!(
            self.status,
            BlockProducerVrfEvaluatorStatus::WaitingForNextEvaluation { .. }
                | BlockProducerVrfEvaluatorStatus::EpochEvaluationSuccess { .. }
                | BlockProducerVrfEvaluatorStatus::InitialisationComplete { .. }
                | BlockProducerVrfEvaluatorStatus::EpochEvaluationInterrupted { .. }
        )
    }

    /// Returns `true` if the evaluator is in the `ReadyToEvaluate` state, otherwise `false`.
    pub fn can_construct_delegator_table(&self) -> bool {
        matches!(
            self.status,
            BlockProducerVrfEvaluatorStatus::ReadyToEvaluate { .. }
        )
    }

    /// Returns `true` if the evaluator is in the `EpochDelegatorTablePending` state, otherwise `false`.
    pub fn is_delegator_table_requested(&self) -> bool {
        matches!(
            self.status,
            BlockProducerVrfEvaluatorStatus::EpochDelegatorTablePending { .. }
        )
    }

    /// Returns `true` if the evaluator is in the `EpochDelegatorTableSuccess` state, otherwise `false`.
    pub fn is_delegator_table_constructed(&self) -> bool {
        matches!(
            self.status,
            BlockProducerVrfEvaluatorStatus::EpochDelegatorTableSuccess { .. }
        )
    }

    /// TODO: redundant fn, remove?
    pub fn can_start_epoch_evaluation(&self) -> bool {
        matches!(
            self.status,
            BlockProducerVrfEvaluatorStatus::EpochDelegatorTableSuccess { .. }
        )
    }

    /// Returns `true` if the evaluator is in any of the following states: `SlotEvaluationPending`,
    /// `SlotEvaluationReceived`, `EpochEvaluationPending`, `EpochBoundsCheck`, otherwise `false` .
    pub fn is_evaluating(&self) -> bool {
        matches!(
            self.status,
            BlockProducerVrfEvaluatorStatus::SlotEvaluationPending { .. }
                | BlockProducerVrfEvaluatorStatus::SlotEvaluationReceived { .. }
                | BlockProducerVrfEvaluatorStatus::EpochEvaluationPending { .. }
                | BlockProducerVrfEvaluatorStatus::EpochBoundsCheck { .. }
        )
    }

    // TODO(adonagy): review this, might be redundant and not needed anymore
    pub fn is_evaluation_pending(&self) -> bool {
        matches!(
            self.status,
            BlockProducerVrfEvaluatorStatus::EpochEvaluationPending { .. }
        )
    }

    /// Returns `true` if the evaluator is in the `InitialSlotSelection` state, otherwise `false`.
    pub fn is_slot_selection(&self) -> bool {
        matches!(
            self.status,
            BlockProducerVrfEvaluatorStatus::InitialSlotSelection { .. }
        )
    }

    /// Returns `true` if the evaluator is in the `SlotEvaluationPending` state, otherwise `false`.
    pub fn is_slot_requested(&self) -> bool {
        matches!(
            self.status,
            BlockProducerVrfEvaluatorStatus::SlotEvaluationPending { .. }
        )
    }

    /// Returns `true` if the evaluator is in the `SlotEvaluationReceived` state, otherwise `false`.
    pub fn is_slot_evaluated(&self) -> bool {
        matches!(
            self.status,
            BlockProducerVrfEvaluatorStatus::SlotEvaluationReceived { .. }
        )
    }

    pub fn set_latest_evaluated_global_slot(&mut self, global_slot: &u32) {
        if self.is_evaluating() {
            if let Some(ref mut pending_evaluation) = self.pending_evaluation {
                pending_evaluation.latest_evaluated_slot = *global_slot;
            }
            self.latest_evaluated_slot = *global_slot
        }
    }

    pub fn initialize_evaluator(&mut self, _epoch: u32, _last_height: u32) {}

    pub fn initial_slot(&self) -> Option<u32> {
        if let BlockProducerVrfEvaluatorStatus::InitialSlotSelection { initial_slot, .. } =
            self.status
        {
            Some(initial_slot)
        } else {
            None
        }
    }

    pub fn get_epoch_bound_from_check(&self) -> Option<SlotPositionInEpoch> {
        if let BlockProducerVrfEvaluatorStatus::EpochBoundsCheck {
            epoch_current_bound,
            ..
        } = &self.status
        {
            Some(epoch_current_bound.clone())
        } else {
            None
        }
    }

    pub fn set_pending_evaluation(&mut self, pending_evaluation: PendingEvaluation) {
        self.pending_evaluation = Some(pending_evaluation)
    }

    pub fn unset_pending_evaluation(&mut self) {
        self.pending_evaluation = None
    }

    pub fn current_evaluation(&self) -> Option<PendingEvaluation> {
        self.pending_evaluation.clone()
    }

    pub fn currently_evaluated_epoch(&self) -> Option<u32> {
        self.pending_evaluation.as_ref().map(|pe| pe.epoch_number)
    }

    pub fn construct_vrf_input(&self) -> Option<VrfEvaluatorInput> {
        if let Some(pending_evaluation) = self.current_evaluation() {
            Some(
                VrfEvaluatorInput::new(
                    pending_evaluation.epoch_data.seed,
                    pending_evaluation.epoch_data.delegator_table,
                    pending_evaluation.latest_evaluated_slot.checked_add(1)?,
                    pending_evaluation.epoch_data.total_currency,
                    pending_evaluation.epoch_data.ledger,
                )
                .with_generation(pending_evaluation.generation),
            )
        } else {
            None
        }
    }

    pub fn retention_slot(&self, current_epoch_number: &u32) -> u32 {
        const PAST_EPOCHS_TO_KEEP: u32 = 2;
        let cutoff_epoch = current_epoch_number.saturating_sub(PAST_EPOCHS_TO_KEEP);
        (cutoff_epoch.saturating_mul(SLOTS_PER_EPOCH)).saturating_sub(1)
    }

    pub fn cleanup_old_won_slots(&mut self, current_epoch_number: &u32) {
        let cutoff_slot = self.retention_slot(current_epoch_number);
        self.won_slots
            .retain(|global_slot, _| cutoff_slot < *global_slot);
    }

    /// If we need to construct delegator table, get it's inputs.
    pub fn vrf_delegator_table_inputs(&self) -> Option<(&v2::LedgerHash, &AccountPublicKey)> {
        match &self.status {
            BlockProducerVrfEvaluatorStatus::EpochDelegatorTablePending {
                staking_epoch_ledger_hash,
                producer,
                ..
            } => Some((staking_epoch_ledger_hash, producer)),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EpochData {
    pub seed: v2::EpochSeed,
    pub ledger: v2::LedgerHash,
    pub delegator_table: Arc<DelegatorTable>,
    pub total_currency: u64,
}

impl EpochData {
    pub fn new(seed: v2::EpochSeed, ledger: v2::LedgerHash, total_currency: u64) -> Self {
        Self {
            seed,
            ledger,
            total_currency,
            delegator_table: Default::default(),
        }
    }
}

impl From<v2::ConsensusProofOfStakeDataEpochDataStakingValueVersionedValueStableV1> for EpochData {
    fn from(
        value: v2::ConsensusProofOfStakeDataEpochDataStakingValueVersionedValueStableV1,
    ) -> Self {
        Self {
            seed: value.seed,
            ledger: value.ledger.hash,
            delegator_table: Default::default(),
            total_currency: value.ledger.total_currency.as_u64(),
        }
    }
}

impl From<v2::ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1> for EpochData {
    fn from(value: v2::ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1) -> Self {
        Self {
            seed: value.seed,
            ledger: value.ledger.hash,
            delegator_table: Default::default(),
            total_currency: value.ledger.total_currency.as_u64(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PendingEvaluation {
    pub epoch_number: u32,
    pub epoch_data: EpochData,
    pub latest_evaluated_slot: u32,
    /// The evaluator's generation when this evaluation began.
    #[serde(default)]
    pub generation: u64,
}

impl PendingEvaluation {
    pub fn fingerprint(&self) -> EpochFingerprint {
        EpochFingerprint::new(self.epoch_number, &self.epoch_data)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BlockProducerVrfEvaluatorStatus {
    /// Initial state
    Idle { time: redux::Timestamp },
    /// Waiting for initialization
    InitialisationPending { time: redux::Timestamp },
    /// Initialization was a success
    InitialisationComplete { time: redux::Timestamp },
    /// Evaluator is ready and able to evaluate an epoch
    ReadyToEvaluate {
        time: redux::Timestamp,
        best_tip_epoch: u32,
        is_current_epoch_evaluated: bool,
        is_next_epoch_evaluated: bool,

        best_tip_slot: u32,
        best_tip_global_slot: u32,
        next_epoch_first_slot: Option<u32>,
        staking_epoch_data: EpochData,
        producer: AccountPublicKey,
    },
    /// Waiting for delegator table building
    EpochDelegatorTablePending {
        time: redux::Timestamp,
        staking_epoch_ledger_hash: v2::LedgerHash,

        best_tip_epoch: u32,
        best_tip_slot: u32,
        best_tip_global_slot: u32,
        next_epoch_first_slot: Option<u32>,
        staking_epoch_data: EpochData,
        producer: AccountPublicKey,
    },
    /// Delegator table built successfully
    EpochDelegatorTableSuccess {
        time: redux::Timestamp,
        staking_epoch_ledger_hash: v2::LedgerHash,

        best_tip_epoch: u32,
        best_tip_slot: u32,
        best_tip_global_slot: u32,
        next_epoch_first_slot: Option<u32>,
        staking_epoch_data: EpochData,
        producer: AccountPublicKey,
    },
    InitialSlotSelection {
        time: redux::Timestamp,
        epoch_number: u32,
        initial_slot: u32,
    },
    /// Epoch evaluation in progress
    EpochEvaluationPending {
        time: redux::Timestamp,
        epoch_number: u32,
        epoch_data: EpochData,
        latest_evaluated_global_slot: u32,
        // next_vrf_input: Option<VrfEvaluatorInput>,
    },
    /// A slot was sent for evaluation to the vrf evaluator service
    SlotEvaluationPending {
        time: redux::Timestamp,
        global_slot: u32,
    },
    SlotEvaluationReceived {
        time: redux::Timestamp,
        global_slot: u32,
    },
    /// The service returned the evaluation succesfully
    EpochEvaluationSuccess {
        time: redux::Timestamp,
        epoch_number: u32,
    },
    /// Waiting for the next possible epoch evaluation
    WaitingForNextEvaluation { time: redux::Timestamp },
    /// Checking whether the evaluator is abble to evaluate the epoch
    /// Note: The current epoch can be allways evaluated right away
    ReadinessCheck {
        time: redux::Timestamp,
        current_epoch: Option<u32>,
        is_next_epoch_seed_finalized: bool,
        best_tip_epoch: u32,
        root_block_epoch: u32,
        is_current_epoch_evaluated: bool,
        is_next_epoch_evaluated: bool,
        last_evaluated_epoch: Option<u32>,
        staking_epoch_data:
            Box<v2::ConsensusProofOfStakeDataEpochDataStakingValueVersionedValueStableV1>,
        next_epoch_data: Box<v2::ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1>,
    },
    EpochBoundsCheck {
        time: redux::Timestamp,
        epoch_number: u32,
        latest_evaluated_global_slot: u32,
        epoch_current_bound: SlotPositionInEpoch,
    },
    EpochEvaluationInterrupted {
        time: redux::Timestamp,
        reason: InterruptReason,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum InterruptReason {
    BestTipWithHigherEpoch,
    EpochDataStale,
    /// The best tip selects other data for the evaluated epoch, or the
    /// clock has passed it.
    EpochDataChanged,
    /// The staking ledger with the selected hash is not available.
    StakingLedgerUnavailable,
}

impl std::fmt::Display for InterruptReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BestTipWithHigherEpoch => write!(f, "Received best tip with higher epoch"),
            Self::EpochDataStale => {
                write!(f, "Epoch data changed or expired before slot selection")
            }
            Self::EpochDataChanged => {
                write!(
                    f,
                    "Best tip selects other epoch data, or the epoch has passed"
                )
            }
            Self::StakingLedgerUnavailable => write!(f, "Staking ledger not available yet"),
        }
    }
}

impl std::fmt::Display for BlockProducerVrfEvaluatorStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle { .. } => write!(f, "Idle"),
            Self::InitialisationPending { .. } => write!(f, "InitialisationPending"),
            Self::InitialisationComplete { .. } => write!(f, "InitialisationComplete"),
            Self::ReadyToEvaluate { .. } => write!(f, "ReadyToEvaluate"),
            Self::EpochDelegatorTablePending { .. } => write!(f, "EpochDelegatorTablePending"),
            Self::EpochDelegatorTableSuccess { .. } => write!(f, "EpochDelegatorTableSuccess"),
            Self::EpochEvaluationPending { .. } => write!(f, "EpochEvaluationPending"),
            Self::SlotEvaluationPending { .. } => write!(f, "SlotEvaluationPending"),
            Self::EpochEvaluationSuccess { .. } => write!(f, "EpochEvaluationSuccess"),
            Self::WaitingForNextEvaluation { .. } => write!(f, "WaitingForNextEvaluation"),
            Self::ReadinessCheck { .. } => write!(f, "ReadinessCheck"),
            Self::InitialSlotSelection { .. } => write!(f, "StartingSlotSelection"),
            Self::EpochBoundsCheck { .. } => write!(f, "EpochBoundsCheck"),
            Self::SlotEvaluationReceived { .. } => write!(f, "SlotEvaluationReceived"),
            Self::EpochEvaluationInterrupted { .. } => write!(f, "EpochEvaluationInterrupted"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum EpochContext {
    Current(EpochData),
    Next(EpochData),
    Waiting,
}

/// Represents the position of the slot in the epoch
/// `Beginning` -> First slot of an epoch
/// `Within` -> The slot is not at the epoch bounds, i.e. is withing the epoch
/// `End` -> The last slot of an epoch
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SlotPositionInEpoch {
    Beginning,
    Within,
    End,
}

impl std::fmt::Display for EpochContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Current(_) => write!(f, "Current"),
            Self::Next(_) => write!(f, "Next"),
            Self::Waiting => write!(f, "Waiting"),
        }
    }
}

impl EpochContext {
    pub fn get_epoch_data(&self) -> Option<EpochData> {
        match self {
            EpochContext::Current(epoch_data) | EpochContext::Next(epoch_data) => {
                Some(epoch_data.clone())
            }
            EpochContext::Waiting => None,
        }
    }
}

// impl BlockProducerVrfEvaluatorStatus {
//     pub fn is_initialized(&self) -> bool {
//         !matches!(self, Self::Idle { .. })
//     }
// }

// TODO(adonagy): rework the tests so they track the situation with the genesis best tip update
#[cfg(test)]
mod test {
    use std::{str::FromStr, sync::Mutex};

    use lazy_static::lazy_static;
    use ledger::AccountIndex;
    use mina_node_account::AccountSecretKey;
    use mina_p2p_messages::{
        bigint::BigInt,
        number::Number,
        v2::{
            ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1,
            ConsensusProofOfStakeDataEpochDataStakingValueVersionedValueStableV1,
            CurrencyAmountStableV1, EpochSeed, LedgerHash, MinaBaseEpochLedgerValueStableV1,
            MinaBaseEpochSeedStableV1, StateHash, UnsignedExtendedUInt32StableV1,
            UnsignedExtendedUInt64Int64ForVersionTagsStableV1,
        },
    };
    use mina_vrf::VrfWonSlot;

    use crate::block_producer::vrf_evaluator::{
        BlockProducerVrfEvaluatorState, BlockProducerVrfEvaluatorStatus, EpochContext,
        EpochFingerprint, SlotPositionInEpoch, VrfWonSlotWithHash,
    };

    lazy_static! {
        static ref DUMMY_STAKING_EPOCH_DATA: ConsensusProofOfStakeDataEpochDataStakingValueVersionedValueStableV1 = {
            ConsensusProofOfStakeDataEpochDataStakingValueVersionedValueStableV1 {
                ledger: MinaBaseEpochLedgerValueStableV1 {
                    hash: LedgerHash::from_str("jxTAZfKKDxoX4vtt68pQCWooXoVLjnfBpusaMwewrcZxsL3uWp6").unwrap(),
                    total_currency: CurrencyAmountStableV1(UnsignedExtendedUInt64Int64ForVersionTagsStableV1(Number(10000000))),
                },
                // seed: MinaBaseEpochSeedStableV1(BigInt::from(0))
                seed: MinaBaseEpochSeedStableV1(BigInt::zero()).into(),
                start_checkpoint: StateHash::from_str("3NKxUSAJE3wqJkrtBhMYhwzrMq3B5sKjPJQRyXz1YrPWA7761opD").unwrap(),
                lock_checkpoint: StateHash::from_str("3NKxUSAJE3wqJkrtBhMYhwzrMq3B5sKjPJQRyXz1YrPWA7761opD").unwrap(),
                epoch_length: UnsignedExtendedUInt32StableV1(Number(7140))
            }
        };
        static ref DUMMY_NEXT_EPOCH_DATA: ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1 = {
            ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1 {
                ledger: MinaBaseEpochLedgerValueStableV1 {
                    hash: LedgerHash::from_str("jxTAZfKKDxoX4vtt68pQCWooXoVLjnfBpusaMwewrcZxsL3uWp6").unwrap(),
                    total_currency: CurrencyAmountStableV1(UnsignedExtendedUInt64Int64ForVersionTagsStableV1(Number(10000000))),
                },
                // seed: MinaBaseEpochSeedStableV1(BigInt::from(0))
                seed: MinaBaseEpochSeedStableV1(BigInt::zero()).into(),
                start_checkpoint: StateHash::from_str("3NKxUSAJE3wqJkrtBhMYhwzrMq3B5sKjPJQRyXz1YrPWA7761opD").unwrap(),
                lock_checkpoint: StateHash::from_str("3NKxUSAJE3wqJkrtBhMYhwzrMq3B5sKjPJQRyXz1YrPWA7761opD").unwrap(),
                epoch_length: UnsignedExtendedUInt32StableV1(Number(7140))
            }
        };
        static ref GENESIS_EPOCH_FIRST_SLOT: Mutex<BlockProducerVrfEvaluatorState> =
            Mutex::new(readiness(0, 0, 0, false, &[]));
        static ref GENESIS_EPOCH_CURRENT_EPOCH_EVALUATED: Mutex<BlockProducerVrfEvaluatorState> =
            Mutex::new(readiness(0, 0, 0, true, &[0]));
        static ref GENESIS_EPOCH_NEXT_EPOCH_EVALUATED: Mutex<BlockProducerVrfEvaluatorState> =
            Mutex::new(readiness(0, 0, 0, true, &[0, 1]));
        static ref SECOND_EPOCH_STARTUP: Mutex<BlockProducerVrfEvaluatorState> =
            Mutex::new(readiness(2, 2, 2, false, &[]));
        static ref SECOND_EPOCH_CURRENT_EPOCH_EVALUATED_WAIT_FOR_NEXT: Mutex<BlockProducerVrfEvaluatorState> =
            Mutex::new(readiness(2, 1, 2, false, &[2]));
    }

    /// A readiness check with the dummy epoch data, the clock in
    /// `current_epoch`, and `completed` epochs evaluated with the data the
    /// tip selects for them.
    fn readiness(
        current_epoch: u32,
        root_block_epoch: u32,
        best_tip_epoch: u32,
        is_next_epoch_seed_finalized: bool,
        completed: &[u32],
    ) -> BlockProducerVrfEvaluatorState {
        let mut state = BlockProducerVrfEvaluatorState::new(redux::Timestamp::global_now());
        for epoch in completed {
            state.mark_epoch_evaluated(EpochFingerprint::new(
                *epoch,
                &DUMMY_STAKING_EPOCH_DATA.to_owned().into(),
            ));
        }
        state.status = BlockProducerVrfEvaluatorStatus::ReadinessCheck {
            time: redux::Timestamp::global_now(),
            current_epoch: Some(current_epoch),
            root_block_epoch,
            best_tip_epoch,
            is_current_epoch_evaluated: completed.contains(&best_tip_epoch),
            is_next_epoch_evaluated: completed.contains(&(best_tip_epoch + 1)),
            is_next_epoch_seed_finalized,
            last_evaluated_epoch: state.last_evaluated_epoch(),
            staking_epoch_data: Box::new(DUMMY_STAKING_EPOCH_DATA.to_owned()),
            next_epoch_data: Box::new(DUMMY_NEXT_EPOCH_DATA.to_owned()),
        };
        state
    }

    #[test]
    fn correctly_set_epoch_context_on_startup() {
        let mut vrf_evaluator_state = GENESIS_EPOCH_FIRST_SLOT.lock().unwrap();
        vrf_evaluator_state.set_epoch_context();
        assert!(matches!(
            vrf_evaluator_state.epoch_context(),
            EpochContext::Current(_)
        ));
    }

    #[test]
    fn correctly_switch_to_next_epoch() {
        let mut vrf_evaluator_state = GENESIS_EPOCH_CURRENT_EPOCH_EVALUATED.lock().unwrap();
        vrf_evaluator_state.set_epoch_context();
        assert!(matches!(
            vrf_evaluator_state.epoch_context(),
            EpochContext::Next(_)
        ));
    }

    #[test]
    fn correctly_switch_to_waiting() {
        let mut vrf_evaluator_state = GENESIS_EPOCH_NEXT_EPOCH_EVALUATED.lock().unwrap();
        vrf_evaluator_state.set_epoch_context();
        assert!(matches!(
            vrf_evaluator_state.epoch_context(),
            EpochContext::Waiting
        ));
    }

    #[test]
    fn generic_epoch_set_epoch_context_on_startup() {
        let mut vrf_evaluator_state = SECOND_EPOCH_STARTUP.lock().unwrap();
        vrf_evaluator_state.set_epoch_context();
        assert!(matches!(
            vrf_evaluator_state.epoch_context(),
            EpochContext::Current(_)
        ));
    }

    #[test]
    fn generic_epoch_correctly_switch_to_next_epoch() {
        // Staking ledger not yet materialized (wait k=290 blocks)
        // The epoch seed is not finalized yet
        let mut vrf_evaluator_state = SECOND_EPOCH_CURRENT_EPOCH_EVALUATED_WAIT_FOR_NEXT
            .lock()
            .unwrap();
        vrf_evaluator_state.set_epoch_context();
        assert!(matches!(
            vrf_evaluator_state.epoch_context(),
            EpochContext::Waiting
        ));

        // Epoch has changed but the root is still from the previous epoch.
        // Next epoch must not be evaluated yet.
        // The epoch seed is not finalized yet.
        vrf_evaluator_state.status = BlockProducerVrfEvaluatorStatus::ReadinessCheck {
            time: redux::Timestamp::global_now(),
            current_epoch: Some(2), // TODO(adonagy)
            root_block_epoch: 1,
            best_tip_epoch: 2,
            is_current_epoch_evaluated: true,
            is_next_epoch_evaluated: false,
            is_next_epoch_seed_finalized: false,
            last_evaluated_epoch: Some(2),
            staking_epoch_data: Box::new(DUMMY_STAKING_EPOCH_DATA.to_owned()),
            next_epoch_data: Box::new(DUMMY_NEXT_EPOCH_DATA.to_owned()),
        };

        vrf_evaluator_state.set_epoch_context();
        assert!(matches!(
            vrf_evaluator_state.epoch_context(),
            EpochContext::Waiting
        ));

        // Epoch has changed, and the root is already at that epoch too.
        // The epoch seed is still not finalized.
        vrf_evaluator_state.status = BlockProducerVrfEvaluatorStatus::ReadinessCheck {
            time: redux::Timestamp::global_now(),
            current_epoch: Some(2), // TODO(adonagy)
            root_block_epoch: 2,
            best_tip_epoch: 2,
            is_current_epoch_evaluated: true,
            is_next_epoch_evaluated: false,
            is_next_epoch_seed_finalized: false,
            last_evaluated_epoch: Some(2),
            staking_epoch_data: Box::new(DUMMY_STAKING_EPOCH_DATA.to_owned()),
            next_epoch_data: Box::new(DUMMY_NEXT_EPOCH_DATA.to_owned()),
        };

        vrf_evaluator_state.set_epoch_context();
        assert!(matches!(
            vrf_evaluator_state.epoch_context(),
            EpochContext::Waiting
        ));

        // Epoch has changed, and the root is already at that epoch too.
        // The epoch seed is also finalized.
        vrf_evaluator_state.status = BlockProducerVrfEvaluatorStatus::ReadinessCheck {
            time: redux::Timestamp::global_now(),
            current_epoch: Some(2), // TODO(adonagy)
            root_block_epoch: 2,
            best_tip_epoch: 2,
            is_current_epoch_evaluated: true,
            is_next_epoch_evaluated: false,
            is_next_epoch_seed_finalized: true,
            last_evaluated_epoch: Some(2),
            staking_epoch_data: Box::new(DUMMY_STAKING_EPOCH_DATA.to_owned()),
            next_epoch_data: Box::new(DUMMY_NEXT_EPOCH_DATA.to_owned()),
        };

        vrf_evaluator_state.set_epoch_context();
        assert!(matches!(
            vrf_evaluator_state.epoch_context(),
            EpochContext::Next(_)
        ));
    }

    /// OCaml selects the tip's next epoch data for a clock in the following
    /// epoch, and has none for a clock further ahead (select_epoch_data,
    /// proof_of_stake.ml:2451-2466). Neither the root nor the seed gate of
    /// precomputation applies: the epoch has started.
    #[test]
    fn clock_ahead_of_the_tip_selects_next_epoch_data_or_waits() {
        for (root, finalized) in [(0, false), (1, false), (2, true)] {
            let mut state = readiness(3, root, 2, finalized, &[2]);
            state.set_epoch_context();
            assert!(matches!(state.epoch_context(), EpochContext::Next(_)));
            let mut state = readiness(3, root, 2, finalized, &[]);
            state.set_epoch_context();
            assert!(
                matches!(state.epoch_context(), EpochContext::Next(_)),
                "the tip's own epoch has passed"
            );
            let mut state = readiness(4, root, 2, finalized, &[]);
            state.set_epoch_context();
            assert!(matches!(state.epoch_context(), EpochContext::Waiting));
        }
        // Already evaluated with exactly that data: wait.
        let mut state = readiness(3, 0, 2, false, &[3]);
        state.set_epoch_context();
        assert!(matches!(state.epoch_context(), EpochContext::Waiting));
    }

    /// A completed epoch counts only for the data it was evaluated with.
    #[test]
    fn a_completed_epoch_with_other_data_is_evaluated_again() {
        let mut state = readiness(2, 2, 2, false, &[]);
        let mut other: super::EpochData = DUMMY_STAKING_EPOCH_DATA.to_owned().into();
        other.total_currency += 1;
        state.mark_epoch_evaluated(EpochFingerprint::new(2, &other));
        state.set_epoch_context();
        assert!(matches!(state.epoch_context(), EpochContext::Current(_)));
        let mut state = readiness(3, 0, 2, false, &[2]);
        state.mark_epoch_evaluated(EpochFingerprint::new(3, &other));
        state.set_epoch_context();
        assert!(matches!(state.epoch_context(), EpochContext::Next(_)));
    }

    #[test]
    fn test_evaluate_epoch_bounds() {
        const GENESIS_EPOCH_BEGINNING: u32 = 0;
        const GENESIS_EPOCH_WITHIN: u32 = 2000;
        const GENESIS_EPOCH_END: u32 = 7139;

        const BEGINNING: u32 = 7140;
        const WITHIN: u32 = 7500;
        const END: u32 = 14279;

        let res = BlockProducerVrfEvaluatorState::evaluate_epoch_bounds(&GENESIS_EPOCH_BEGINNING);
        assert!(matches!(res, SlotPositionInEpoch::Beginning));
        let res = BlockProducerVrfEvaluatorState::evaluate_epoch_bounds(&GENESIS_EPOCH_WITHIN);
        assert!(matches!(res, SlotPositionInEpoch::Within));
        let res = BlockProducerVrfEvaluatorState::evaluate_epoch_bounds(&GENESIS_EPOCH_END);
        assert!(matches!(res, SlotPositionInEpoch::End));

        let res = BlockProducerVrfEvaluatorState::evaluate_epoch_bounds(&BEGINNING);
        assert!(matches!(res, SlotPositionInEpoch::Beginning));

        let res = BlockProducerVrfEvaluatorState::evaluate_epoch_bounds(&WITHIN);
        assert!(matches!(res, SlotPositionInEpoch::Within));

        let res = BlockProducerVrfEvaluatorState::evaluate_epoch_bounds(&END);
        assert!(matches!(res, SlotPositionInEpoch::End));
    }

    fn generate_slots(
        start_slot: u32,
        end_slot: u32,
    ) -> impl Iterator<Item = (u32, VrfWonSlotWithHash)> {
        let dummy_ledger_hash =
            LedgerHash::from_str("jxTAZfKKDxoX4vtt68pQCWooXoVLjnfBpusaMwewrcZxsL3uWp6").unwrap();

        (start_slot..=end_slot).map(move |slot| {
            let dummy_won_slot = VrfWonSlot {
                producer: AccountSecretKey::genesis_producer().public_key(),
                winner_account: AccountSecretKey::genesis_producer().public_key(),
                vrf_output: Box::new(
                    mina_vrf::genesis_vrf(EpochSeed::from(MinaBaseEpochSeedStableV1(
                        BigInt::zero(),
                    )))
                    .unwrap(),
                ),
                global_slot: slot,
                account_index: AccountIndex(0),
                value_with_threshold: None,
            };
            let epoch_data = EpochFingerprint {
                epoch: slot / super::SLOTS_PER_EPOCH,
                ledger: dummy_ledger_hash.clone(),
                seed: EpochSeed::zero(),
                total_currency: 0,
            };
            (slot, VrfWonSlotWithHash::new(dummy_won_slot, epoch_data))
        })
    }

    #[test]
    fn test_cleanup_old_won_slots() {
        // arbitrary, need it just to fill it with some slots
        let mut vrf_evaluator_state = SECOND_EPOCH_STARTUP.lock().unwrap();

        vrf_evaluator_state
            .won_slots
            .extend(generate_slots(1, 28560));

        assert_eq!(28560, vrf_evaluator_state.won_slots.len());

        // should keep all the slots
        vrf_evaluator_state.cleanup_old_won_slots(&1);
        assert_eq!(
            1,
            *vrf_evaluator_state.won_slots.first_key_value().unwrap().0,
            "First retained slot should be the first slot of the epoch, 7140"
        );

        // epoch 1 slots should be discarded
        vrf_evaluator_state.cleanup_old_won_slots(&3);
        assert_eq!(
            7140,
            *vrf_evaluator_state.won_slots.first_key_value().unwrap().0,
            "First retained slot should be the first slot of the epoch, 7140"
        );

        // epoch 2 slots should be discarded
        vrf_evaluator_state.cleanup_old_won_slots(&4);
        assert_eq!(
            14280,
            *vrf_evaluator_state.won_slots.first_key_value().unwrap().0,
            "First retained slot should be the first slot of the epoch, 142180"
        );
    }
}
