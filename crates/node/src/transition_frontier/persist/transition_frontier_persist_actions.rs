use mina_core::ActionEvent;
use mina_p2p_messages::v2::StateHash;
use serde::{Deserialize, Serialize};

use super::{
    PersistCheckpointState, PersistRestoreState, TransitionFrontierPersistState,
    LOAD_CANCEL_TIMEOUT_SECS, LOAD_MAX_IDLE_TIMEOUT_SECS,
};
use crate::{
    ledger::write::LedgerWriteState,
    persist::event::{
        PersistChainPutDone, PersistCheckpointDone, PersistOpened, PersistRestored,
        PersistSnapshotInfo, PersistStoreSummary, PersistWriteError,
    },
};

pub type TransitionFrontierPersistActionWithMeta =
    redux::ActionWithMeta<TransitionFrontierPersistAction>;
pub type TransitionFrontierPersistActionWithMetaRef<'a> =
    redux::ActionWithMeta<&'a TransitionFrontierPersistAction>;

/// The frontier store (R20 D4, D5): opening it, the restore from it at
/// startup (I5d1), and its writes.
#[derive(Serialize, Deserialize, Debug, Clone, ActionEvent)]
#[action_event(level = debug)]
pub enum TransitionFrontierPersistAction {
    /// Open the store once the chain id is known.
    #[action_event(level = debug)]
    OpenInit,
    #[action_event(level = debug, fields(dir = display(&opened.dir), outcome = display(&opened.outcome)))]
    OpenSuccess {
        opened: Box<PersistOpened>,
    },
    #[action_event(level = warn, fields(error))]
    OpenError {
        error: String,
    },
    /// The store thread didn't open the store in time: persistence is off,
    /// and the node bootstraps.
    #[action_event(level = warn)]
    OpenTimeout,
    /// Load the store for a restore, once it is open and the genesis block
    /// is in the frontier (R20 D5 step 2).
    #[action_event(level = debug)]
    LoadInit,
    /// The store passed the loader's checks and its ledgers are with the
    /// ledger thread: replay its chain.
    #[action_event(level = debug, fields(
        root = display(restored.root.hash()),
        root_height = restored.root.height(),
        tip = display(restored.tip().hash()),
        tip_height = restored.tip().height(),
        blocks = restored.blocks.len(),
        load_ms = restored.timings.total_ms,
    ))]
    LoadSuccess {
        restored: Box<PersistRestored>,
    },
    /// Only the snarked ledgers were loaded, as copy origins for a
    /// bootstrap.
    #[action_event(level = warn, fields(reason, ledgers))]
    LoadLedgersOnly {
        reason: String,
        ledgers: u32,
    },
    /// Nothing to restore.
    #[action_event(level = debug, fields(reason))]
    LoadAbsent {
        reason: String,
    },
    /// The store failed a check, or too many restores didn't finish: the
    /// store thread deleted it.
    #[action_event(level = warn, fields(reason))]
    LoadDiscarded {
        reason: String,
    },
    /// The store couldn't be read this time; it is kept.
    #[action_event(level = warn, fields(error))]
    LoadError {
        error: String,
    },
    /// A checked stage completed on the store thread.
    #[action_event(level = debug, fields(completed_stages))]
    LoadProgress {
        completed_stages: u64,
    },
    /// The store's newest manifest after a load that didn't restore: a load
    /// that gave up a damaged generation fell back to the older one.
    #[action_event(level = info, fields(generation = store.generation))]
    StoreSummary {
        store: Box<PersistStoreSummary>,
    },
    /// The load crossed its idle timeout: request cancellation. Once
    /// cancellation is accepted, bound the wait for its acknowledgement.
    #[action_event(level = warn)]
    LoadTimeout,
    /// Cancellation won against ledger handoff. Keep sync held.
    LoadCancelPending,
    /// All abandoned work and its intermediates have been dropped.
    LoadCancelled,
    /// The restored chain is committed: the restore is done.
    #[action_event(level = info)]
    ReplayDone,
    /// A restored block failed to apply: drop the store and bootstrap.
    #[action_event(level = warn, fields(error))]
    ReplayFailed {
        error: String,
    },
    /// Hand the best chain to the store thread: done whenever it changes.
    ChainPutInit,
    ChainPutSuccess {
        done: Box<PersistChainPutDone>,
    },
    #[action_event(level = warn, fields(error = display(&error.error)))]
    ChainPutError {
        error: PersistWriteError,
    },
    /// Ask the ledger thread for a checkpoint's snapshot, when one is due.
    #[action_event(level = debug)]
    CheckpointInit,
    /// The snapshot request went to the ledger thread.
    CheckpointSnapshotSent,
    /// The snapshot is taken; the store thread is writing it.
    #[action_event(level = debug, fields(root = display(root), ledger_thread_ms = snapshot.total_ms))]
    CheckpointPending {
        root: StateHash,
        snapshot: Box<PersistSnapshotInfo>,
    },
    #[action_event(level = info, fields(
        root = display(&done.root),
        generation = done.generation,
        bytes_written = done.written.bytes,
        elapsed_ms = done.elapsed_ms,
    ))]
    CheckpointSuccess {
        done: Box<PersistCheckpointDone>,
    },
    #[action_event(level = warn, fields(error = display(&error.error)))]
    CheckpointError {
        error: PersistWriteError,
    },
}

impl redux::EnablingCondition<crate::State> for TransitionFrontierPersistAction {
    fn is_enabled(&self, state: &crate::State, time: redux::Timestamp) -> bool {
        let tf = &state.transition_frontier;
        let persist = &tf.persist;
        let load_timeout_secs = tf
            .config
            .persist
            .load_timeout_secs
            .clamp(1, LOAD_MAX_IDLE_TIMEOUT_SECS);
        match self {
            Self::OpenInit => {
                matches!(persist, TransitionFrontierPersistState::Idle)
                    && state.p2p.ready().is_some()
                    && tf.genesis.block_with_real_or_dummy_proof().is_some()
            }
            Self::OpenSuccess { .. } | Self::OpenError { .. } => {
                matches!(persist, TransitionFrontierPersistState::OpenPending { .. })
            }
            Self::OpenTimeout => match persist {
                TransitionFrontierPersistState::OpenPending { time: since } => {
                    time.checked_sub(*since)
                        .is_some_and(|waited| waited.as_secs() >= load_timeout_secs)
                }
                _ => false,
            },
            Self::LoadInit => {
                // At the genesis block, synced there and nothing else under
                // way: the restore replaces that frontier.
                persist
                    .restore()
                    .is_some_and(|restore| matches!(restore, PersistRestoreState::Idle))
                    && tf.config.persist.restore
                    && tf.sync.is_synced()
                    && tf.root().is_some_and(|root| root.is_genesis())
            }
            Self::LoadSuccess { .. } => {
                persist.is_load_pending()
                    && tf.sync.is_synced()
                    && tf.root().is_some_and(|root| root.is_genesis())
            }
            Self::StoreSummary { .. } => persist.ready().is_some(),
            Self::LoadLedgersOnly { .. }
            | Self::LoadAbsent { .. }
            | Self::LoadDiscarded { .. }
            | Self::LoadError { .. } => persist.is_load_pending(),
            Self::LoadProgress { completed_stages } => matches!(
                persist.restore(),
                Some(PersistRestoreState::LoadPending { completed_stages: current, .. })
                    if completed_stages > current
            ),
            Self::LoadCancelPending => persist.is_load_pending(),
            Self::LoadCancelled => matches!(persist.restore(), Some(PersistRestoreState::Cancelling { .. })),
            Self::LoadTimeout => match persist.restore() {
                Some(PersistRestoreState::LoadPending { time: since, last_progress_time, .. }) => {
                    time.checked_sub(last_progress_time.unwrap_or(*since))
                        .is_some_and(|idle| idle.as_secs() >= load_timeout_secs)
                }
                Some(PersistRestoreState::Cancelling { time: since }) => {
                    time.checked_sub(*since)
                        .is_some_and(|wait| wait.as_secs() >= LOAD_CANCEL_TIMEOUT_SECS)
                }
                _ => false,
            },
            Self::ReplayDone => {
                // The restore's commit: the only sync while replaying.
                persist.is_replaying()
                    && tf.sync.is_synced()
                    && tf.root().is_some_and(|root| !root.is_genesis())
            }
            Self::ReplayFailed { .. } => persist.is_replaying(),
            Self::ChainPutInit => {
                let Some(ready) = persist.ready() else {
                    return false;
                };
                // A chain rooted at genesis is not stored: it would drop a
                // stored checkpoint before the node has synced (D4).
                tf.sync.is_synced()
                    && tf.root().is_some_and(|root| !root.is_genesis())
                    && tf
                        .best_tip()
                        .is_some_and(|tip| ready.chain_put_sent.as_ref() != Some(tip.hash()))
            }
            Self::ChainPutSuccess { .. } | Self::ChainPutError { .. } => persist.ready().is_some(),
            Self::CheckpointInit => {
                let Some(ready) = persist.ready() else {
                    return false;
                };
                // Synced at a root that isn't genesis (a chain younger than k
                // blocks needs no checkpoint), the ledger write slot free, and
                // the store thread handed this chain already, so that its
                // blocks are stored before the checkpoint needs them.
                tf.sync.is_synced()
                    && tf.root().is_some_and(|root| !root.is_genesis())
                    && matches!(
                        state.ledger.write,
                        LedgerWriteState::Idle | LedgerWriteState::Success { .. }
                    )
                    && tf
                        .best_tip()
                        .is_some_and(|tip| ready.chain_put_sent.as_ref() == Some(tip.hash()))
                    && ready
                        .is_checkpoint_due(time, tf.config.persist.checkpoint_interval_secs)
            }
            Self::CheckpointSnapshotSent => persist
                .ready()
                .is_some_and(|ready| matches!(ready.checkpoint, PersistCheckpointState::Init { .. })),
            Self::CheckpointPending { root, .. } => persist.ready().is_some_and(|ready| {
                matches!(&ready.checkpoint, PersistCheckpointState::Init { root: r, .. } if r == root)
            }),
            Self::CheckpointSuccess { .. } | Self::CheckpointError { .. } => {
                persist.is_checkpoint_pending()
            }
        }
    }
}

impl From<TransitionFrontierPersistAction> for crate::Action {
    fn from(value: TransitionFrontierPersistAction) -> Self {
        crate::transition_frontier::TransitionFrontierAction::Persist(value).into()
    }
}
