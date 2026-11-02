//! Retained bootstrap diagnostics; updated by the reducer, never by reads.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum BootstrapPhase {
    Starting,
    WaitingForGenesis,
    WaitingForPeers,
    FetchingBestTip,
    SyncingRootLedger,
    DownloadingScanState,
    BuildingStagedLedger,
    SyncingEpochLedgers,
    LoadingFrontier,
    Participating,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum SyncedLedger {
    RootSnarked,
    StakingEpoch,
    NextEpoch,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LedgerSyncProgress {
    pub targets: i64,
    pub target_set_at: Option<u64>,
    pub pending_child_hashes: i64,
    pub pending_contents: i64,
    pub synced: bool,
    pub answers_accepted: i64,
    pub answers_ignored: i64,
    pub answers_rejected: i64,
    pub last_accepted_answer_at: Option<u64>,
    pub num_accounts: Option<i64>,
    pub accounts_settled: i64,
    pub accounts_received: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LedgerSync {
    pub ledger: SyncedLedger,
    pub target: Option<String>,
    pub active: bool,
    pub progress: LedgerSyncProgress,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BootstrapStatus {
    #[serde(default)]
    pub(crate) in_progress: bool,
    pub phase: BootstrapPhase,
    pub phase_started_at: u64,
    pub bootstrap_started_at: Option<u64>,
    pub bootstrap_attempts: i64,
    pub last_attempt_failure: Option<String>,
    pub ledger_syncs: Vec<LedgerSync>,
}

use crate::transition_frontier::{
    sync::{
        ledger::{
            snarked::{
                TransitionFrontierSyncLedgerSnarkedAction as A,
                TransitionFrontierSyncLedgerSnarkedState as Snarked,
            },
            staged::TransitionFrontierSyncLedgerStagedState as Staged,
            SyncLedgerTargetKind, TransitionFrontierSyncLedgerAction as L,
            TransitionFrontierSyncLedgerState as Ledger,
        },
        TransitionFrontierSyncAction as S, TransitionFrontierSyncState as Sync,
    },
    TransitionFrontierAction as F,
};

impl Default for BootstrapStatus {
    fn default() -> Self {
        Self::new(0)
    }
}

impl BootstrapStatus {
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/bootstrap_status/bootstrap_status.ml#L62-L108>
    pub fn new(time: u64) -> Self {
        Self {
            in_progress: false,
            phase: BootstrapPhase::Starting,
            phase_started_at: time,
            bootstrap_started_at: None,
            bootstrap_attempts: 0,
            last_attempt_failure: None,
            ledger_syncs: vec![],
        }
    }

    fn set_phase(&mut self, phase: BootstrapPhase, time: u64) {
        if phase != self.phase {
            self.phase = phase;
            self.phase_started_at = time;
        }
    }

    /// Keep the last progress of each ledger when the sync state is replaced.
    /// Rust syncs epoch ledgers before the root, and catches up blocks while its
    /// frontier is still serving; those are respectively SYNCING_EPOCH_LEDGERS
    /// and PARTICIPATING. A bootstrap attempt is a new Init after failure, not
    /// a moving target or an individual peer retry.
    pub fn observe(state: &mut crate::State, action: &crate::Action, time: redux::Timestamp) {
        if !matches!(
            action,
            crate::Action::CheckTimeouts(_)
                | crate::Action::TransitionFrontier(_)
                | crate::Action::P2p(_)
                | crate::Action::P2pCallbacks(_)
        ) {
            return;
        }
        record_ignored_answer(state, action);
        if matches!(action, crate::Action::TransitionFrontier(F::Synced { .. })) {
            state.transition_frontier.bootstrap.in_progress = false;
        }
        let phase = phase(state, time);
        let frontier = &mut state.transition_frontier;
        let telemetry = &mut frontier.bootstrap;
        let now = u64::from(time);
        let action = match action {
            crate::Action::TransitionFrontier(a) => Some(a),
            _ => None,
        };
        if matches!(action, Some(F::Sync(S::Init { .. }))) && phase != BootstrapPhase::Participating
        {
            if !telemetry.in_progress {
                telemetry.bootstrap_started_at = Some(now);
                telemetry.bootstrap_attempts = 0;
                telemetry.last_attempt_failure = None;
            }
            telemetry.in_progress = true;
            telemetry.bootstrap_attempts = telemetry.bootstrap_attempts.saturating_add(1);
        }
        match action {
            Some(F::SyncFailed { error, .. }) => telemetry.last_attempt_failure = Some(error.to_string()),
            Some(F::LedgerRecovery { error, .. }) => telemetry.last_attempt_failure = Some(error.clone()),
            Some(F::Sync(S::Ledger(L::Staged(crate::transition_frontier::sync::ledger::staged::TransitionFrontierSyncLedgerStagedAction::ReconstructError { error })))) => {
                telemetry.last_attempt_failure = Some(error.clone());
            }
            _ => {}
        }
        telemetry.set_phase(phase, now);
        // Only ledger transitions change this projection. Do not walk ledger
        // queues for network events, status polls or block verification.
        if action.is_none() {
            return;
        }
        let current = frontier.sync.ledger();
        let active_kind = current.map(|l| kind(l.target_kind()));
        for s in &mut telemetry.ledger_syncs {
            if Some(s.ledger) != active_kind {
                s.active = false;
            }
        }
        let Some(ledger) = current else {
            return;
        };
        let Some(snarked) = ledger.snarked() else {
            // Init may retarget the same synchronizer: preserve its cumulative
            // counters. Staged reconstruction and Success have ended it.
            if !matches!(ledger, Ledger::Init { .. }) {
                for s in &mut telemetry.ledger_syncs {
                    if Some(s.ledger) == active_kind {
                        s.active = false;
                    }
                }
            }
            return;
        };
        let which = kind(snarked.target().kind);
        let target = snarked.target().snarked_ledger_hash.to_string();
        let index = if let Some(i) = telemetry
            .ledger_syncs
            .iter()
            .position(|s| s.ledger == which)
        {
            i
        } else {
            telemetry.ledger_syncs.push(LedgerSync {
                ledger: which,
                target: None,
                active: true,
                progress: Default::default(),
            });
            telemetry.ledger_syncs.len() - 1
        };
        let s = &mut telemetry.ledger_syncs[index];
        if !s.active && snarked.is_pending() {
            s.progress = Default::default();
            s.target = None;
        }
        s.active = snarked.is_pending();
        if s.target.as_ref() != Some(&target) {
            s.target = Some(target);
            s.progress.targets = s.progress.targets.saturating_add(1);
            s.progress.target_set_at = Some(now);
            s.progress.num_accounts = None;
            s.progress.accounts_settled = 0;
        }
        s.progress.synced = !snarked.is_pending();
        s.progress.pending_child_hashes = 0;
        s.progress.pending_contents = 0;
        if let Snarked::MerkleTreeSyncPending {
            queue,
            pending_addresses,
            ..
        } = snarked
        {
            for address in queue.keys().chain(pending_addresses.keys()) {
                if address.length()
                    < crate::ledger::LEDGER_DEPTH
                        - crate::transition_frontier::sync::ledger::snarked::ACCOUNT_SUBTREE_HEIGHT
                {
                    s.progress.pending_child_hashes =
                        s.progress.pending_child_hashes.saturating_add(1);
                } else {
                    s.progress.pending_contents = s.progress.pending_contents.saturating_add(1);
                }
            }
        }
        if let Some(F::Sync(S::Ledger(L::Snarked(a)))) = action {
            match a {
                A::NumAccountsAccepted { num_accounts, .. } => {
                    s.progress.num_accounts = Some(count(*num_accounts));
                    accepted(&mut s.progress, now);
                }
                A::ChildHashesAccepted {
                    address,
                    hashes,
                    previous_hashes,
                    ..
                } => {
                    accepted(&mut s.progress, now);
                    let depth = address.length() + hashes.len().trailing_zeros() as usize;
                    for (address, (hash, previous)) in address
                        .iter_children(depth)
                        .zip(hashes.iter().zip(previous_hashes))
                    {
                        if hash == previous {
                            settle(&mut s.progress, &address);
                        }
                    }
                }
                A::ChildAccountsReceived { accounts, .. } => {
                    s.progress.accounts_received = s
                        .progress
                        .accounts_received
                        .saturating_add(count(accounts.len() as u64));
                }
                A::ChildAccountsAccepted { count: n, .. } => {
                    accepted(&mut s.progress, now);
                    s.progress.accounts_settled =
                        s.progress.accounts_settled.saturating_add(count(*n));
                }
                A::NumAccountsRejected { .. }
                | A::ChildHashesRejected { .. }
                | A::ChildAccountsRejected { .. } => {
                    s.progress.answers_rejected = s.progress.answers_rejected.saturating_add(1);
                }
                _ => {}
            }
        }
        // A completed sync has every account even when matching subtrees
        // allowed the synchronizer to skip downloading some of them.
        if s.progress.synced {
            if let Some(total) = s.progress.num_accounts {
                s.progress.accounts_settled = total;
            }
        }
        telemetry.ledger_syncs.sort_by_key(|s| match s.ledger {
            SyncedLedger::RootSnarked => 0,
            SyncedLedger::StakingEpoch => 1,
            SyncedLedger::NextEpoch => 2,
        });
    }
}

fn accepted(p: &mut LedgerSyncProgress, time: u64) {
    p.answers_accepted = p.answers_accepted.saturating_add(1);
    p.last_accepted_answer_at = Some(time);
}
pub fn count(n: u64) -> i64 {
    n.min(i64::MAX as u64) as i64
}
fn kind(kind: SyncLedgerTargetKind) -> SyncedLedger {
    match kind {
        SyncLedgerTargetKind::Root => SyncedLedger::RootSnarked,
        SyncLedgerTargetKind::StakingEpoch => SyncedLedger::StakingEpoch,
        SyncLedgerTargetKind::NextEpoch => SyncedLedger::NextEpoch,
    }
}

fn phase(state: &crate::State, now: redux::Timestamp) -> BootstrapPhase {
    use BootstrapPhase::*;
    let f = &state.transition_frontier;
    if let Some(genesis) = state.genesis_block() {
        if now < genesis.genesis_timestamp() {
            return WaitingForGenesis;
        }
    } else {
        return Starting;
    }
    if !f.bootstrap.in_progress && !f.is_bootstrapping(now) {
        return Participating;
    }
    match &f.sync {
        Sync::StakingLedgerPending(_)
        | Sync::NextEpochLedgerPending(_)
        | Sync::StakingLedgerSuccess { .. }
        | Sync::NextEpochLedgerSuccess { .. } => SyncingEpochLedgers,
        Sync::RootLedgerPending(p) => match &p.ledger {
            Ledger::Staged(Staged::PartsFetchPending { .. }) => DownloadingScanState,
            Ledger::Staged(_) => BuildingStagedLedger,
            _ => SyncingRootLedger,
        },
        Sync::RootLedgerSuccess { .. }
        | Sync::BlocksPending { .. }
        | Sync::BlocksSuccess { .. }
        | Sync::CommitPending { .. }
        | Sync::CommitSuccess { .. } => LoadingFrontier,
        _ if state.p2p.ready_peers_iter().next().is_none() => WaitingForPeers,
        _ => FetchingBestTip,
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/syncable_ledger/syncable_ledger.ml#L598-L605>
fn settle(p: &mut LedgerSyncProgress, address: &crate::ledger::LedgerAddress) {
    if let Some(total) = p.num_accounts {
        let width = 1u64 << (crate::ledger::LEDGER_DEPTH - address.length());
        let first = address.to_index().0 * width;
        let held = (total as u64).min(first + width).saturating_sub(first);
        p.accounts_settled = p.accounts_settled.saturating_add(count(held));
    }
}

/// Replies no longer associated with pending work are discarded by Rust's
/// enabling conditions. Account for them at ingress, before that discard.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/syncable_ledger/syncable_ledger.ml#L869-L907>
fn record_ignored_answer(state: &mut crate::State, action: &crate::Action) {
    use crate::p2p::{
        callbacks::P2pCallbacksAction,
        channels::rpc::{P2pRpcRequest, P2pRpcResponse},
    };
    use mina_p2p_messages::v2::MinaLedgerSyncLedgerAnswerStableV2 as Answer;
    let crate::Action::P2pCallbacks(P2pCallbacksAction::P2pChannelsRpcResponseReceived {
        peer_id,
        id,
        response: Some(response),
    }) = action
    else {
        return;
    };
    let P2pRpcResponse::LedgerQuery(answer) = response.as_ref() else {
        return;
    };
    let snarked = state
        .transition_frontier
        .sync
        .ledger()
        .and_then(|l| l.snarked());
    let pending = snarked.is_some_and(|s| match answer {
        Answer::NumAccounts(..) => s
            .peer_num_account_query_get(peer_id, *id)
            .and_then(|p| p.attempts.get(peer_id))
            .is_some_and(|a| a.is_pending()),
        _ => s
            .peer_address_query_get(peer_id, *id)
            .and_then(|(_, p)| p.attempts.get(peer_id))
            .is_some_and(|a| a.is_pending()),
    });
    if pending {
        return;
    }
    let target = state
        .p2p
        .get_ready_peer(peer_id)
        .and_then(|p| p.channels.rpc.local_responded_request())
        .filter(|(rpc_id, _)| *rpc_id == *id)
        .and_then(|(_, request)| match request {
            P2pRpcRequest::LedgerQuery(hash, _) => Some(hash.to_string()),
            _ => None,
        });
    let active = snarked.map(|s| kind(s.target().kind));
    let syncs = &mut state.transition_frontier.bootstrap.ledger_syncs;
    let index = syncs
        .iter()
        .position(|s| target.is_some() && s.target == target)
        .or_else(|| syncs.iter().position(|s| Some(s.ledger) == active));
    if let Some(i) = index {
        syncs[i].progress.answers_ignored = syncs[i].progress.answers_ignored.saturating_add(1);
    }
}
