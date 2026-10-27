//! What the store thread reports to the node (R20 D3), as
//! `Event::Persist`, and what the ledger thread reports about a checkpoint's
//! snapshot.

use std::collections::BTreeMap;

use mina_core::block::ArcBlockWithHash;
use mina_p2p_messages::v2;
use serde::{Deserialize, Serialize};

use super::load::PersistLoadTimings;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum PersistEvent {
    Opened(PersistOpened),
    OpenFailed {
        error: String,
    },
    ChainPut(Result<PersistChainPutDone, PersistWriteError>),
    Checkpoint(Result<PersistCheckpointDone, PersistWriteError>),
    /// What a load for a restore found (I5d1). The ledgers it loaded are
    /// with the ledger thread before this is sent.
    Loaded(Box<PersistLoadResult>),
    /// The store's newest manifest after a load that didn't restore, sent
    /// before its [`Self::Loaded`]: a load that gives up a damaged generation
    /// falls back to the older one, which may store other ledgers than the
    /// store opened with, and the next checkpoint must know which (I5e).
    Summary(PersistStoreSummary),
}

/// The result of a load (I5d1), as the node hears it.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum PersistLoadResult {
    /// The store passed every check: its ledgers are with the ledger thread,
    /// and the chain is to be replayed.
    Restored(PersistRestored),
    /// Only the snarked ledgers were loaded, as copy origins for a bootstrap.
    LedgersOnly {
        reason: String,
        generation: u64,
        ledgers: Vec<v2::LedgerHash>,
        timings: PersistLoadTimings,
    },
    /// Nothing to restore.
    Absent { reason: String },
    /// The store failed a check, or too many restores of it didn't finish:
    /// it was deleted.
    Discarded { reason: String },
    /// The store couldn't be read this time (an I/O error); it is kept.
    Error { error: String },
    /// The node gave up waiting: nothing was handed to the ledger thread.
    Cancelled,
}

/// A checked store: the chain to replay and what the node needs with it.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistRestored {
    pub generation: u64,
    /// This load's number among the generation's restore attempts, from 1.
    pub attempt: u32,
    /// The checkpoint's root, whose staged ledger the ledger thread has.
    pub root: ArcBlockWithHash,
    /// The stored chain after the root, oldest first.
    pub blocks: Vec<ArcBlockWithHash>,
    /// The protocol states the root's scan state needs.
    pub needed_protocol_states: BTreeMap<v2::StateHash, v2::MinaStateProtocolStateValueStableV2>,
    /// The snarked ledgers handed to the ledger thread.
    pub ledgers: Vec<v2::LedgerHash>,
    pub timings: PersistLoadTimings,
}

impl PersistRestored {
    pub fn tip(&self) -> &ArcBlockWithHash {
        self.blocks.last().unwrap_or(&self.root)
    }
}

/// A ledger the newest manifest lists, and the base its file is a diff over,
/// if it is one.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct PersistStoredLedger {
    pub hash: v2::LedgerHash,
    pub base: Option<v2::LedgerHash>,
}

/// The newest manifest, in short.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct PersistStoreSummary {
    pub generation: u64,
    /// The checkpoint's root, if the store has a checkpoint.
    pub checkpoint_root: Option<v2::StateHash>,
    pub chain_len: u32,
    pub tip: Option<v2::StateHash>,
    pub ledgers: Vec<PersistStoredLedger>,
    /// Files the manifest references, and their total size.
    pub files: u32,
    pub bytes: u64,
}

impl PersistStoreSummary {
    pub fn has_checkpoint(&self) -> bool {
        self.checkpoint_root.is_some()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistOpened {
    pub dir: String,
    /// `empty`, `opened` or `reset: <reason>`.
    pub outcome: String,
    /// Files the garbage collection at open deleted.
    pub deleted: u32,
    pub store: PersistStoreSummary,
    pub elapsed_ms: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistWriteError {
    pub error: String,
    /// The disk is full: persistence is off for the rest of the run.
    pub disable: bool,
}

/// What a write added to the store.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct PersistWritten {
    /// Data files written, and their total size.
    pub files: u32,
    pub bytes: u64,
    /// The manifest slot written.
    pub manifest_bytes: u64,
    /// Data files the garbage collection after the write deleted.
    pub deleted: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistChainPutDone {
    pub tip: v2::StateHash,
    pub generation: u64,
    /// The chain's root wasn't on the stored chain: the store no longer has a
    /// checkpoint, and the node should take one.
    pub checkpoint_invalidated: bool,
    pub written: PersistWritten,
    pub store: PersistStoreSummary,
    /// On the store thread, encoding included.
    pub elapsed_ms: f64,
}

/// How a checkpoint wrote one ledger.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum PersistLedgerWriteKind {
    /// Stored already, not written again.
    Stored,
    /// Written whole, read from the ledger thread a piece at a time.
    Full,
    /// Written as a diff over a stored ledger, taken on the ledger thread.
    Diff { base: v2::LedgerHash, accounts: u64 },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistLedgerWriteInfo {
    pub hash: v2::LedgerHash,
    pub kind: PersistLedgerWriteKind,
}

/// The ledger thread's part of a checkpoint (D3): a snapshot right after a
/// commit, taken while no other write runs.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct PersistSnapshotInfo {
    pub ledgers: Vec<PersistLedgerWriteInfo>,
    /// Accounts and hashes in the root staged ledger's diff over the root
    /// snarked ledger.
    pub staged_accounts: u64,
    pub staged_hashes: u64,
    /// Time on the ledger thread: the staged diff, the ledger diffs, and in
    /// all, cloning the scan state and pinning included.
    pub staged_diff_ms: f64,
    pub ledger_diffs_ms: f64,
    pub total_ms: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistCheckpointDone {
    pub root: v2::StateHash,
    pub root_height: u32,
    pub generation: u64,
    pub written: PersistWritten,
    pub store: PersistStoreSummary,
    /// The parts (`StagedLedgerAuxAndPendingCoinbases`): converted to wire
    /// types and encoded on the store thread.
    pub parts_bytes: u64,
    pub parts_encode_ms: f64,
    /// Pieces of full ledgers the ledger thread encoded, and its time on them
    /// in all and at most for one.
    pub stream_parts: u32,
    pub stream_ms: f64,
    pub stream_max_ms: f64,
    /// On the store thread, from taking the job to the manifest written.
    pub elapsed_ms: f64,
    pub snapshot: PersistSnapshotInfo,
}
