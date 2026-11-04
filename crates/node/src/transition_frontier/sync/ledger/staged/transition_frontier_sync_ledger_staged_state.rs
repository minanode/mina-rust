use std::{collections::BTreeMap, sync::Arc};

use mina_p2p_messages::v2::{MinaStateProtocolStateValueStableV2, StateHash};
use redux::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    p2p::{channels::rpc::P2pRpcId, PeerId},
    transition_frontier::sync::ledger::SyncLedgerTargetWithStaged,
};

use super::{
    PeerStagedLedgerPartsFetchError, StagedLedgerAuxAndPendingCoinbasesValid,
    StagedLedgerAuxAndPendingCoinbasesValidated,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TransitionFrontierSyncLedgerStagedState {
    /// Fetching pieces required to reconstruct staged ledger from
    /// snarked ledger.
    PartsFetchPending {
        time: Timestamp,
        target: SyncLedgerTargetWithStaged,
        attempts: BTreeMap<PeerId, PeerStagedLedgerPartsFetchState>,
    },
    /// Fetched pieces required to reconstruct staged ledger from
    /// snarked ledger.
    PartsFetchSuccess {
        time: Timestamp,
        target: SyncLedgerTargetWithStaged,
        parts: Arc<StagedLedgerAuxAndPendingCoinbasesValid>,
        sender: StagedLedgerPartsSender,
    },
    ReconstructEmpty {
        time: Timestamp,
        target: SyncLedgerTargetWithStaged,
    },
    ReconstructPending {
        time: Timestamp,
        target: SyncLedgerTargetWithStaged,
        parts: Option<Arc<StagedLedgerAuxAndPendingCoinbasesValid>>,
        /// For parts a peer sent, not for an empty staged ledger.
        sender: Option<StagedLedgerPartsSender>,
    },
    ReconstructError {
        time: Timestamp,
        target: SyncLedgerTargetWithStaged,
        parts: Option<Arc<StagedLedgerAuxAndPendingCoinbasesValid>>,
        error: String,
    },
    ReconstructSuccess {
        time: Timestamp,
        target: SyncLedgerTargetWithStaged,
        parts: Option<Arc<StagedLedgerAuxAndPendingCoinbasesValid>>,
    },
    Success {
        time: Timestamp,
        target: SyncLedgerTargetWithStaged,
        needed_protocol_states: BTreeMap<StateHash, MinaStateProtocolStateValueStableV2>,
    },
}

/// The peer whose parts the staged ledger is rebuilt from, and the attempts
/// of the other peers. The staged ledger hash doesn't cover all of the parts
/// (the scan state hash leaves out its completed jobs), so parts that make it
/// can still fail to rebuild the staged ledger. That peer's answer is then
/// rejected and the fetch goes on from these attempts, as OCaml's bootstrap
/// starts over when the parts it got fail to rebuild.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/bootstrap_controller/bootstrap_controller.ml#L478-L508>
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StagedLedgerPartsSender {
    pub peer_id: PeerId,
    pub other_attempts: BTreeMap<PeerId, PeerStagedLedgerPartsFetchState>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum PeerStagedLedgerPartsFetchState {
    Pending {
        time: Timestamp,
        rpc_id: P2pRpcId,
    },
    Error {
        time: Timestamp,
        rpc_id: P2pRpcId,
        error: PeerStagedLedgerPartsFetchError,
    },
    Success {
        time: Timestamp,
        parts: StagedLedgerAuxAndPendingCoinbasesValidated,
    },
    Invalid {
        time: Timestamp,
    },
    Valid {
        time: Timestamp,
        parts: Arc<StagedLedgerAuxAndPendingCoinbasesValid>,
    },
}

impl TransitionFrontierSyncLedgerStagedState {
    pub fn pending(time: Timestamp, target: SyncLedgerTargetWithStaged) -> Self {
        Self::PartsFetchPending {
            time,
            target,
            attempts: Default::default(),
        }
    }

    pub fn target(&self) -> &SyncLedgerTargetWithStaged {
        match self {
            Self::PartsFetchPending { target, .. } => target,
            Self::PartsFetchSuccess { target, .. } => target,
            Self::ReconstructEmpty { target, .. } => target,
            Self::ReconstructPending { target, .. } => target,
            Self::ReconstructError { target, .. } => target,
            Self::ReconstructSuccess { target, .. } => target,
            Self::Success { target, .. } => target,
        }
    }

    pub fn is_parts_fetched(&self) -> bool {
        !matches!(self, Self::PartsFetchPending { .. })
    }

    /// Whether a peer still has a request for the parts that may bring them:
    /// neither failed nor stale. Only one is sent at a time
    /// (`PartsPeerFetchInit`).
    pub fn is_parts_request_live(&self, now: Timestamp) -> bool {
        self.fetch_attempts().is_some_and(|attempts| {
            attempts
                .values()
                .any(|attempt| !attempt.is_failed_or_stale(now))
        })
    }

    pub fn target_with_parts(
        &self,
    ) -> Option<(
        &SyncLedgerTargetWithStaged,
        Option<&Arc<StagedLedgerAuxAndPendingCoinbasesValid>>,
    )> {
        Some(match self {
            Self::PartsFetchSuccess { target, parts, .. } => (target, Some(parts)),
            Self::ReconstructEmpty { target, .. } => (target, None),
            Self::ReconstructPending { target, parts, .. } => (target, parts.as_ref()),
            _ => return None,
        })
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    pub fn fetch_attempts(&self) -> Option<&BTreeMap<PeerId, PeerStagedLedgerPartsFetchState>> {
        match self {
            Self::PartsFetchPending { attempts, .. } => Some(attempts),
            _ => None,
        }
    }

    pub fn parts_fetch_rpc_id(&self, peer_id: &PeerId) -> Option<P2pRpcId> {
        self.fetch_attempts()?.get(peer_id)?.fetch_pending_rpc_id()
    }
}

impl PeerStagedLedgerPartsFetchState {
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. } | Self::Invalid { .. })
    }

    /// A lost response or validation follow-up must not block parts forever.
    pub fn is_failed_or_stale(&self, now: Timestamp) -> bool {
        self.is_error() || self.retry_ready(now)
    }

    pub fn retry_ready(&self, now: Timestamp) -> bool {
        match self {
            Self::Error { time, .. } | Self::Invalid { time } => now
                .checked_sub(*time)
                .is_some_and(|elapsed| elapsed >= super::super::snarked::LEDGER_QUERY_RETRY_DELAY),
            Self::Pending { time, .. } | Self::Success { time, .. } | Self::Valid { time, .. } => {
                now.checked_sub(*time)
                    .is_some_and(|elapsed| elapsed >= super::STAGED_LEDGER_PARTS_STALE_AFTER)
            }
        }
    }

    pub fn is_valid(&self) -> bool {
        matches!(self, Self::Valid { .. })
    }

    pub fn fetch_pending_rpc_id(&self) -> Option<P2pRpcId> {
        match self {
            Self::Pending { rpc_id, .. } => Some(*rpc_id),
            _ => None,
        }
    }
}
