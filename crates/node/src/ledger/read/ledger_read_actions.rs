use serde::{Deserialize, Serialize};

use super::{
    LedgerReadId, LedgerReadInitCallback, LedgerReadRequest, LedgerReadRequestState,
    LedgerReadResponse,
};

pub type LedgerReadActionWithMetaRef<'a> = redux::ActionWithMeta<&'a LedgerReadAction>;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum LedgerReadAction {
    FindTodos,
    Init {
        request: LedgerReadRequest,
        callback: LedgerReadInitCallback,
    },
    Pending {
        id: LedgerReadId,
        request: LedgerReadRequest,
    },
    Success {
        id: LedgerReadId,
        response: LedgerReadResponse,
    },
    Prune {
        id: LedgerReadId,
    },
}

impl redux::EnablingCondition<crate::State> for LedgerReadAction {
    fn is_enabled(&self, state: &crate::State, _time: redux::Timestamp) -> bool {
        match self {
            LedgerReadAction::FindTodos => state.ledger.read.is_total_cost_under_limit(),
            // Keep RPC requests in Init and peer requests pending. FindTodos
            // retries them after restore or the deferred genesis load finishes.
            LedgerReadAction::Init { request, .. } => {
                state.transition_frontier.genesis_ledgers_ready()
                    && state.ledger.read.can_admit(request)
            }
            LedgerReadAction::Pending { id, request } => {
                state.ledger.read.can_admit(request)
                    && !state.ledger.read.contains(*id)
                    && state.ledger.read.next_req_id() == *id
            }
            LedgerReadAction::Success { id, response } => match &state.ledger.read.get(*id) {
                Some(LedgerReadRequestState::Pending { request, .. }) => {
                    request.kind() == response.kind()
                }
                _ => false,
            },
            LedgerReadAction::Prune { id } => matches!(
                state.ledger.read.get(*id),
                Some(LedgerReadRequestState::Success { .. })
            ),
        }
    }
}

impl From<LedgerReadAction> for crate::Action {
    fn from(value: LedgerReadAction) -> Self {
        Self::Ledger(value.into())
    }
}
