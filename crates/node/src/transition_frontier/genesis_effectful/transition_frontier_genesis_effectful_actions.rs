use std::sync::Arc;

use mina_core::ActionEvent;
use mina_p2p_messages::v2::{ProverExtendBlockchainInputStableV2, StateHash};
use serde::{Deserialize, Serialize};

use crate::transition_frontier::genesis::GenesisConfig;

pub type TransitionFrontierGenesisEffectfulActionWithMeta =
    redux::ActionWithMeta<TransitionFrontierGenesisEffectfulAction>;
pub type TransitionFrontierGenesisEffectfulActionWithMetaRef<'a> =
    redux::ActionWithMeta<&'a TransitionFrontierGenesisEffectfulAction>;

#[derive(derive_more::From, Serialize, Deserialize, Debug, Clone, ActionEvent)]
#[action_event(level = trace)]
pub enum TransitionFrontierGenesisEffectfulAction {
    LedgerLoadInit {
        config: Arc<GenesisConfig>,
    },
    /// I5d2: load the deferred genesis ledgers of `config`, checking that
    /// it gives the genesis the node started with, `expected`.
    LedgersLoadInit {
        config: Arc<GenesisConfig>,
        expected: Arc<GenesisConfig>,
    },
    ProveInit {
        block_hash: StateHash,
        input: Box<ProverExtendBlockchainInputStableV2>,
        deadline: redux::Timestamp,
    },
}

impl redux::EnablingCondition<crate::State> for TransitionFrontierGenesisEffectfulAction {
    fn is_enabled(&self, _state: &crate::State, _time: redux::Timestamp) -> bool {
        true
    }
}

impl From<TransitionFrontierGenesisEffectfulAction> for crate::Action {
    fn from(value: TransitionFrontierGenesisEffectfulAction) -> Self {
        crate::transition_frontier::TransitionFrontierAction::GenesisEffect(value).into()
    }
}
