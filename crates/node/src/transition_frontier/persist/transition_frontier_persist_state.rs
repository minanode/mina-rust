use mina_p2p_messages::v2::StateHash;
use redux::Timestamp;
use serde::{Deserialize, Serialize};

use crate::persist::{
    event::{PersistSnapshotInfo, PersistStoreSummary},
    load::PersistLoadTimings,
};

/// R20 D4: a checkpoint every 30 minutes; an older one only lengthens the
/// replay of a restore.
pub const DEFAULT_CHECKPOINT_INTERVAL_SECS: u64 = 30 * 60;

/// I5d3: after a restore, a checkpoint is due as soon as the node is synced
/// if the stored chain is more than `k + 1` and this many blocks long, so that
/// the replay of the next restore stays about `k` blocks however often the
/// node restarts. OCaml's persistent frontier moves its root with every root
/// transition, flushed at the latest after 5 block windows
/// (`persistent_frontier/diff_buffer.ml`, `max_latency`), so its restarts
/// replay about `k` blocks too.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_frontier/persistent_frontier/diff_buffer.ml#L6-L23>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_frontier/persistent_frontier/diff_buffer.ml#L120-L143>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_frontier/persistent_frontier/database.ml#L405-L424>
pub const RESTORE_CHECKPOINT_SLACK_BLOCKS: u32 = 5;

/// After a checkpoint fails, the next one waits this long.
pub const CHECKPOINT_RETRY_SECS: u64 = 5 * 60;

/// I5d6: opening, or loading without completed work, gets this long.
/// Every completed read/decode/hash/proof batch resets the idle deadline.
/// There is no absolute load deadline. The CLI allows 1..=300 seconds;
/// direct callers are clamped to that range too.
pub const LOAD_TIMEOUT_SECS: u64 = 60;
pub const LOAD_MAX_IDLE_TIMEOUT_SECS: u64 = 300;
/// Bound only the drain after cancellation won against ledger handoff. A
/// blocked read cannot hold bootstrap forever; a progressing load has no cap.
pub const LOAD_CANCEL_TIMEOUT_SECS: u64 = 30;
/// A stored tip more than 4k slots behind the clock needs bootstrap.
pub const STALE_SLOTS_PER_K: u32 = 4;

/// The web node's load timeout (I5e): its checks run in WASM, on the
/// visitor's cores, and opening the store may wait for a reloaded page's old
/// workers to let go of it (R20 D5 gave the browser 120 s).
pub const WEB_LOAD_TIMEOUT_SECS: u64 = 180;

fn restore_by_default() -> bool {
    true
}

fn load_timeout_by_default() -> u64 {
    LOAD_TIMEOUT_SECS
}

/// The frontier store's settings (`--frontier-store`, `--checkpoint-interval`,
/// `--frontier-store-no-restore`). Off by default.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TransitionFrontierPersistConfig {
    pub enabled: bool,
    pub checkpoint_interval_secs: u64,
    /// Restore from the store at startup (I5d1), rather than only write it.
    #[serde(default = "restore_by_default")]
    pub restore: bool,
    /// Maximum idle seconds without completed load work. Bootstrap starts
    /// after cancellation drains the worker, or after LOAD_CANCEL_TIMEOUT_SECS.
    /// There is no wall-clock cap on a progressing load.
    #[serde(default = "load_timeout_by_default")]
    pub load_timeout_secs: u64,
}

impl Default for TransitionFrontierPersistConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            checkpoint_interval_secs: DEFAULT_CHECKPOINT_INTERVAL_SECS,
            restore: true,
            load_timeout_secs: LOAD_TIMEOUT_SECS,
        }
    }
}

/// The frontier store, as the node sees it (R20 D5).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TransitionFrontierPersistState {
    /// Off, or turned off for this run (the store is locked by another
    /// process, of a newer format, or the disk is full).
    Disabled {
        reason: String,
    },
    /// On; the store opens once the chain id is known.
    Idle,
    OpenPending {
        time: Timestamp,
    },
    Ready(Box<PersistReadyState>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistReadyState {
    pub opened_at: Timestamp,
    pub dir: String,
    /// What opening found: `empty`, `opened generation N` or `reset: ..`.
    pub open_outcome: String,
    /// The newest manifest, as the store thread last reported it.
    pub store: PersistStoreSummary,
    /// The tip of the last best chain handed to the store thread.
    pub chain_put_sent: Option<StateHash>,
    /// Chain puts handed over and not reported yet.
    pub chain_puts_pending: u32,
    pub checkpoint: PersistCheckpointState,
    /// The last checkpoint written in this run.
    pub last_checkpoint: Option<PersistLastCheckpoint>,
    /// No checkpoint is started before this, after one failed.
    pub retry_after: Option<Timestamp>,
    pub stats: PersistStats,
    /// The restore from the store at startup (I5d1).
    #[serde(default)]
    pub restore: PersistRestoreState,
    /// Set when a restore commits (I5d3): while the stored chain is longer
    /// than this, a checkpoint is due at once. The next checkpoint clears it.
    #[serde(default)]
    pub checkpoint_chain_len_limit: Option<u32>,
}

/// The restore from the store at startup (R20 D5, I5d1): load, replay the
/// stored chain through the sync, then follow the network as usual.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub enum PersistRestoreState {
    /// Not started: the load comes once the genesis block is in the
    /// frontier.
    #[default]
    Idle,
    /// The store thread is loading the store.
    LoadPending {
        time: Timestamp,
        #[serde(default)]
        completed_stages: u64,
        #[serde(default)]
        last_progress_time: Option<Timestamp>,
    },
    /// Cancellation requested; bootstrap stays held until the worker has
    /// dropped all intermediates and stopped its parallel work, for at most
    /// LOAD_CANCEL_TIMEOUT_SECS. A stuck worker then disables persistence for
    /// this run and cannot hand off its ledgers even if its read returns.
    Cancelling { time: Timestamp },
    /// The loaded chain is being replayed through the sync.
    Replaying {
        time: Timestamp,
        info: Box<PersistRestoreInfo>,
    },
    Done {
        time: Timestamp,
        outcome: PersistRestoreOutcome,
    },
}

/// A loaded store, in short.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistRestoreInfo {
    pub generation: u64,
    pub attempt: u32,
    pub root: StateHash,
    pub root_height: u32,
    pub tip: StateHash,
    pub tip_height: u32,
    /// Blocks replayed after the root.
    pub blocks: u32,
    pub ledgers: u32,
    /// When the load was asked for.
    pub load_started: Timestamp,
    /// On the store thread.
    pub timings: PersistLoadTimings,
}

/// How the restore ended (each one a reason the node logs).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum PersistRestoreOutcome {
    /// Not asked for (`--frontier-store-no-restore`).
    Off,
    /// The stored chain was replayed and committed: the node follows the
    /// network from the stored tip.
    Restored {
        info: Box<PersistRestoreInfo>,
        /// From the replay's start to its commit.
        replay_ms: f64,
    },
    /// Only the snarked ledgers were loaded, as copy origins: the node
    /// bootstraps.
    LedgersOnly { reason: String, ledgers: u32 },
    /// Nothing to restore: the node bootstraps.
    Absent { reason: String },
    /// The store failed a check, or too many restores of it didn't finish:
    /// it was deleted, and the node bootstraps.
    Discarded { reason: String },
    /// The load or the replay failed: the node bootstraps.
    Failed { error: String },
    /// The load crossed its idle timeout and cancellation was acknowledged.
    TimedOut,
}

impl PersistRestoreState {
    /// Whether the restore is still to be decided or under way.
    pub fn is_in_progress(&self) -> bool {
        matches!(
            self,
            Self::Idle
                | Self::LoadPending { .. }
                | Self::Cancelling { .. }
                | Self::Replaying { .. }
        )
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub enum PersistCheckpointState {
    #[default]
    Idle,
    /// The ledger thread is asked for the snapshot.
    Init { time: Timestamp, root: StateHash },
    /// The store thread is writing it.
    Pending {
        time: Timestamp,
        root: StateHash,
        snapshot: Box<PersistSnapshotInfo>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistLastCheckpoint {
    pub time: Timestamp,
    pub root: StateHash,
    pub root_height: u32,
    pub generation: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct PersistStats {
    pub chain_puts: u64,
    pub checkpoints: u64,
    pub errors: u64,
    pub last_error: Option<String>,
    /// Data files and manifests written in this run.
    pub bytes_written: u64,
}

impl TransitionFrontierPersistState {
    pub fn new(config: &TransitionFrontierPersistConfig) -> Self {
        if config.enabled {
            Self::Idle
        } else {
            Self::Disabled {
                reason: "off".to_owned(),
            }
        }
    }

    pub fn ready(&self) -> Option<&PersistReadyState> {
        match self {
            Self::Ready(ready) => Some(ready),
            _ => None,
        }
    }

    pub fn ready_mut(&mut self) -> Option<&mut PersistReadyState> {
        match self {
            Self::Ready(ready) => Some(ready),
            _ => None,
        }
    }

    pub fn is_checkpoint_pending(&self) -> bool {
        self.ready()
            .is_some_and(|ready| !matches!(ready.checkpoint, PersistCheckpointState::Idle))
    }

    pub fn restore(&self) -> Option<&PersistRestoreState> {
        self.ready().map(|ready| &ready.restore)
    }

    /// Whether the stored chain is being replayed (R20 D5): its blocks are
    /// trusted not to be a peer's fault, and are not sent to the archive
    /// again.
    pub fn is_replaying(&self) -> bool {
        matches!(self.restore(), Some(PersistRestoreState::Replaying { .. }))
    }

    /// Whether the stored chain was replayed and committed (I5d1).
    pub fn is_restored(&self) -> bool {
        matches!(
            self.restore(),
            Some(PersistRestoreState::Done {
                outcome: PersistRestoreOutcome::Restored { .. },
                ..
            })
        )
    }

    pub fn is_load_pending(&self) -> bool {
        matches!(
            self.restore(),
            Some(PersistRestoreState::LoadPending { .. })
        )
    }

    /// Whether the sync must not take a new target yet (R20 D5): from the
    /// start until the restore is decided, and through the replay. Without
    /// this a candidate verified meanwhile would start a bootstrap from
    /// genesis, or change the replay's chain. `restore` is the config's.
    pub fn holds_sync(&self, restore: bool) -> bool {
        match self {
            Self::Disabled { .. } => false,
            Self::Idle | Self::OpenPending { .. } => restore,
            Self::Ready(ready) => restore && ready.restore.is_in_progress(),
        }
    }
}

impl PersistReadyState {
    /// Whether a checkpoint is due (D4): when the store has none (the first
    /// run, or a chain put dropped it after a bootstrap or a deep
    /// reorganisation), on the first chance in a run that bootstrapped (its
    /// chain doesn't start on the stored one), and then every
    /// `interval_secs`. A run that restored from the store counts the
    /// interval from the restore (`last_checkpoint` is the stored one), but
    /// takes one at once while the stored chain is longer than
    /// `checkpoint_chain_len_limit` (I5d3).
    pub fn is_checkpoint_due(&self, now: Timestamp, interval_secs: u64) -> bool {
        if !matches!(self.checkpoint, PersistCheckpointState::Idle)
            || self.retry_after.is_some_and(|after| now < after)
        {
            return false;
        }
        if !self.store.has_checkpoint() {
            return true;
        }
        if self
            .checkpoint_chain_len_limit
            .is_some_and(|limit| self.store.chain_len > limit)
        {
            return true;
        }
        match &self.last_checkpoint {
            None => true,
            Some(last) => now
                .checked_sub(last.time)
                .is_some_and(|since| since.as_secs() >= interval_secs),
        }
    }
}
