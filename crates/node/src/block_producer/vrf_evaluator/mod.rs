mod block_producer_vrf_evaluator_state;
pub use block_producer_vrf_evaluator_state::*;

mod block_producer_vrf_evaluator_event;
pub use block_producer_vrf_evaluator_event::*;

mod block_producer_vrf_evaluator_actions;
pub use block_producer_vrf_evaluator_actions::*;

mod block_producer_vrf_evaluator_reducer;

use std::{collections::BTreeMap, sync::Arc};

use ledger::AccountIndex;
use mina_p2p_messages::v2::{self, EpochSeed, LedgerHash};
use mina_vrf::{VrfEvaluationOutput, VrfWonSlot};
use serde::{Deserialize, Serialize};

use crate::account::AccountPublicKey;

pub type DelegatorTable = BTreeMap<AccountIndex, (AccountPublicKey, u64)>;

/// The epoch data a VRF evaluation, its progress and its wins are bound to:
/// the evaluated epoch with its staking ledger, seed and total currency. A
/// win can only become a block whose parent selects the same data for the
/// win's epoch.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct EpochFingerprint {
    pub epoch: u32,
    pub ledger: LedgerHash,
    pub seed: EpochSeed,
    pub total_currency: u64,
}

impl EpochFingerprint {
    pub fn new(epoch: u32, data: &EpochData) -> Self {
        Self {
            epoch,
            ledger: data.ledger.clone(),
            seed: data.seed.clone(),
            total_currency: data.total_currency,
        }
    }

    /// The staking data a block extending `state` uses in `epoch`: OCaml's
    /// `select_epoch_data`. The next epoch uses the state's next epoch data,
    /// its own epoch (or the genesis epoch from a genesis-epoch state) its
    /// staking data, and any other epoch has none.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/consensus/proof_of_stake.ml#L2451-L2466>
    pub fn select(
        state: &v2::ConsensusProofOfStakeDataConsensusStateValueStableV2,
        epoch: u32,
    ) -> Option<Self> {
        let curr_epoch = super::to_epoch_and_slot(&state.curr_global_slot_since_hard_fork).0;
        let data: EpochData = if curr_epoch.checked_add(1) == Some(epoch) {
            state.next_epoch_data.clone().into()
        } else if epoch == curr_epoch || (state.epoch_count.as_u32() == 0 && epoch == 0) {
            state.staking_epoch_data.clone().into()
        } else {
            return None;
        };
        Some(Self::new(epoch, &data))
    }

    /// The staking data of a child of `parent` in `epoch`: OCaml's
    /// `Epoch_data.update_pair`. A child in a later epoch, however many
    /// epochs later, takes the parent's next epoch data; a child in the
    /// parent's epoch keeps its staking data. Unlike [`Self::select`], which
    /// decides what to evaluate, this decides what a block extending
    /// `parent` actually uses.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/consensus/proof_of_stake.ml#L1062-L1109>
    pub fn for_child(
        parent: &v2::ConsensusProofOfStakeDataConsensusStateValueStableV2,
        epoch: u32,
    ) -> Option<Self> {
        let parent_epoch = super::to_epoch_and_slot(&parent.curr_global_slot_since_hard_fork).0;
        let data: EpochData = if epoch > parent_epoch {
            parent.next_epoch_data.clone().into()
        } else if epoch == parent_epoch {
            parent.staking_epoch_data.clone().into()
        } else {
            return None;
        };
        Some(Self::new(epoch, &data))
    }
}

#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct VrfEvaluatorInput {
    pub epoch_seed: EpochSeed,
    pub delegator_table: Arc<DelegatorTable>,
    pub global_slot: u32,
    pub total_currency: u64,
    pub staking_ledger_hash: LedgerHash,
    /// The evaluation this request belongs to, echoed in its response so
    /// that a response from an interrupted evaluation is rejected.
    #[serde(default)]
    pub generation: u64,
}

#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct VrfWonSlotWithHash {
    pub won_slot: VrfWonSlot,
    pub staking_ledger_hash: LedgerHash,
    /// The epoch data the win was evaluated with. A win without it is never
    /// produced.
    #[serde(default)]
    pub epoch_data: Option<EpochFingerprint>,
}

impl VrfWonSlotWithHash {
    pub fn new(won_slot: VrfWonSlot, epoch_data: EpochFingerprint) -> Self {
        Self {
            won_slot,
            staking_ledger_hash: epoch_data.ledger.clone(),
            epoch_data: Some(epoch_data),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Serialize)]
pub struct VrfEvaluationOutputWithHash {
    pub evaluation_result: VrfEvaluationOutput,
    pub staking_ledger_hash: LedgerHash,
    #[serde(default)]
    pub generation: u64,
}

impl std::fmt::Display for VrfEvaluationOutputWithHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ", self.staking_ledger_hash)?;
        match &self.evaluation_result {
            VrfEvaluationOutput::SlotWon(won_slot) => {
                write!(f, "SlotWon {}", won_slot.global_slot)
            }
            VrfEvaluationOutput::SlotLost(global_slot) => {
                write!(f, "SlotLost {}", global_slot)
            }
        }
    }
}

impl VrfEvaluationOutputWithHash {
    pub fn new(evaluation_result: VrfEvaluationOutput, input: &VrfEvaluatorInput) -> Self {
        Self {
            evaluation_result,
            staking_ledger_hash: input.staking_ledger_hash.clone(),
            generation: input.generation,
        }
    }
}

impl VrfEvaluatorInput {
    pub fn new(
        epoch_seed: EpochSeed,
        delegator_table: Arc<DelegatorTable>,
        global_slot: u32,
        total_currency: u64,
        staking_ledger_hash: LedgerHash,
    ) -> Self {
        Self {
            epoch_seed,
            delegator_table,
            global_slot,
            total_currency,
            staking_ledger_hash,
            generation: 0,
        }
    }

    pub fn with_generation(self, generation: u64) -> Self {
        Self { generation, ..self }
    }
}
