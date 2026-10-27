use crate::block_producer::vrf_evaluator::VrfEvaluatorInput;
use mina_core::ActionEvent;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, ActionEvent)]
pub enum BlockProducerVrfEvaluatorEffectfulAction {
    EvaluateSlot {
        vrf_input: VrfEvaluatorInput,
    },
    /// `won`: the slot is ours, counted once.
    SlotEvaluated {
        epoch: u32,
        won: bool,
    },
    /// The epoch's evaluation starts: its stake, for the dashboard's expected
    /// wins (the key's delegated stake and the staking ledger's total).
    InitializeStats {
        epoch: u32,
        initial_slot: u32,
        delegated_stake: u64,
        total_currency: u64,
        delegators: u32,
    },
}

impl redux::EnablingCondition<crate::State> for BlockProducerVrfEvaluatorEffectfulAction {
    fn is_enabled(&self, _state: &crate::State, _time: redux::Timestamp) -> bool {
        true
    }
}

impl From<BlockProducerVrfEvaluatorEffectfulAction> for crate::Action {
    fn from(value: BlockProducerVrfEvaluatorEffectfulAction) -> Self {
        Self::BlockProducerEffectful(crate::BlockProducerEffectfulAction::VrfEvaluator(value))
    }
}
