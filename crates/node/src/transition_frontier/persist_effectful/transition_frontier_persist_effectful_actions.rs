use mina_core::{ActionEvent, ChainId};
use mina_p2p_messages::v2::StateHash;
use serde::{Deserialize, Serialize};

use crate::persist::load::PersistLoadRequest;

pub type TransitionFrontierPersistEffectfulActionWithMeta =
    redux::ActionWithMeta<TransitionFrontierPersistEffectfulAction>;

#[derive(Serialize, Deserialize, Debug, Clone, ActionEvent)]
#[action_event(level = trace)]
pub enum TransitionFrontierPersistEffectfulAction {
    Open {
        chain_id: ChainId,
        genesis_state_hash: StateHash,
    },
    /// Hand the current best chain to the store thread.
    ChainPut,
    /// Load the store for a restore (I5d1).
    Load { request: PersistLoadRequest },
    /// The node stopped waiting for the load: what it loads is dropped.
    LoadCancel,
    /// The restore is committed: clear the restore attempts record.
    RestoreDone,
    /// A restore failed: delete the store.
    Discard { reason: String },
}

impl redux::EnablingCondition<crate::State> for TransitionFrontierPersistEffectfulAction {
    fn is_enabled(&self, _state: &crate::State, _time: redux::Timestamp) -> bool {
        true
    }
}

impl From<TransitionFrontierPersistEffectfulAction> for crate::Action {
    fn from(value: TransitionFrontierPersistEffectfulAction) -> Self {
        crate::transition_frontier::TransitionFrontierAction::PersistEffect(value).into()
    }
}
