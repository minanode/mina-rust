//! The loader (R20 D3, D5 step 3, I5d1): when a node with a frontier store
//! starts, it reads the store on the store thread, decides whether to trust
//! it, and builds what a restore needs: the snarked ledgers, the root staged
//! ledger, the root block, the blocks to replay from the root to the stored
//! tip, and the protocol states the root's scan state needs. The node then
//! replays the blocks through its normal sync (`TransitionFrontierSyncAction::RestoreInit`)
//! and catches up from the restored tip.
//!
//! # The policy
//!
//! Opening the store (`FrontierStore::open`) has already applied the version
//! and chain rules (D6): a store of an older format version, or written for
//! another chain id or genesis, was deleted; one of a newer version doesn't
//! open, and persistence is off for the run. Stores of other chain ids live
//! in other directories and are never read. The loader then decides, in this
//! order:
//!
//! | What it finds                                                    | Outcome                                                         |
//! | ---------------------------------------------------------------- | --------------------------------------------------------------- |
//! | no valid manifest                                                | [`LoadOutcome::Absent`]: bootstrap                              |
//! | [`DISCARD_AFTER_ATTEMPTS`] unfinished restores of this generation | the store is deleted ([`LoadOutcome::Discarded`]): bootstrap    |
//! | [`LEDGERS_ONLY_AFTER_ATTEMPTS`] unfinished restores               | [`LoadOutcome::LedgersOnly`]: the snarked ledgers, as copy origins |
//! | no checkpoint                                                    | ledgers only                                                    |
//! | the stored tip more than `stale_slots` (4k) behind the clock     | ledgers only: the network's root has left the stored chain      |
//! | the stored tip ahead of the clock                                | ledgers only: the clock or the store is wrong                   |
//! | the tip's staking ledger or the root's snarked ledger not stored | ledgers only: the commit needs both                             |
//! | a checksum, decode, hash, link or staged ledger hash check fails | the generation is given up and the previous one tried; with none left, the store is deleted |
//! | an I/O error other than a short file                             | an error: bootstrap, the store is kept                          |
//! | everything checks                                                | [`LoadOutcome::Restored`]                                       |
//!
//! The restore attempts record (`ATTEMPTS`) is written, counted up, before
//! anything is decoded. The node clears it once the restored chain is
//! committed; a ledgers-only load clears it once the ledgers are handed over.
//! So a store whose load or replay panics the node (a bug, or damage the
//! checks miss) is restored at most twice, then used only for its ledgers,
//! then deleted, instead of crashing the node at every start.
//!
//! A clean shutdown (SIGTERM or SIGINT) during a restore isn't the store's
//! fault: [`RestoreGuard::take_back`] writes the record back as it was before
//! the load, so `docker stop` during a restore doesn't count (RC7-FIXES). A
//! start refused before the store is opened never reaches the record.
//!
//! # What is checked
//!
//! Nothing restored is committed or served unless it passed the checks a
//! bootstrap applies to what peers send: the store is not trusted, even with
//! valid checksums (the I5d1 review's rule). Before anything is handed to the
//! ledger thread:
//!
//! - every file's length and CRC32 before it is decoded (accidental damage);
//! - every block decodes, its state hash recomputed from its header is its
//!   name, its height and slot are the recorded ones, and it links to the one
//!   before; the first is the checkpoint's root, with the checkpoint's ledger
//!   hashes;
//! - every block, the root included, passes a received block's checks that
//!   don't depend on the time: its body hashes to the header's body reference,
//!   its delta block chain proof leads to its parent, its protocol version and
//!   stop slots are valid, and its genesis state and constants are ours;
//! - every block's blockchain proof verifies (batched: one accumulator check,
//!   kimchi batch verification in parallel). A peer's best tip is verified so,
//!   and the blocks below it are bound to it by their hashes; here each block
//!   is checked, so that none whose proof was replaced can be served;
//! - every ledger, the root staged ledger's included, is rehashed from its
//!   accounts and every stored node hash compared (D2's full mode; a diff's
//!   changed paths over its base, itself rehashed first): a cached hash is
//!   never trusted, so a ledger whose accounts changed is rejected, and a
//!   ledgers-only load hands over only rehashed ledgers. With each root
//!   checked against its name, and the names the chain uses checked against
//!   its proven headers, the ledgers are those of the chain;
//! - every ledger's derived account-ID and optional token-owner indexes,
//!   account count and last-filled location agree with its verified leaves;
//!   Merkle verification alone cannot detect damaged lookup indexes. The root
//!   staged ledger is a mask over the root snarked ledger (I5d5): its own
//!   indexes are checked against its own accounts, and each of its accounts
//!   is where the snarked ledger has it or new to it;
//! - the root staged ledger, built from its diff over the root snarked ledger
//!   and the stored scan state and pending coinbase, has the complete staged
//!   ledger hash of the root block (I5ab's gate);
//! - the stored protocol states hash to themselves and include every one the
//!   root's scan state needs.
//!
//! The replay then applies each block after the root with the staged ledger
//! hash check, and the node only follows a network best tip it verified.

use std::{
    collections::{BTreeMap, HashMap},
    io,
    sync::{Arc, Mutex},
};

use ledger::{
    proofs::verifiers::BlockVerifier,
    scan_state::{pending_coinbase::PendingCoinbase, scan_state::ScanState},
    staged_ledger::staged_ledger::StagedLedger,
    Mask,
};
use mina_core::block::{prevalidate, ArcBlockWithHash};
use mina_curves::pasta::Fp;
use mina_p2p_messages::{binprot::BinProtRead, v2};
use mina_snark::VerifierSRS;
use rayon::prelude::*;
// `std::time::Instant` isn't there in the browser (I5e).
use redux::Instant;
use serde::{Deserialize, Serialize};

use super::{
    backend::StoreBackend,
    format::{block_name, parts_name, staged_name, Attempts, Checkpoint, Manifest},
    ledger_codec::CodecError,
    load_control::LoadControl,
    store::{decode_ledgers_controlled, LedgerFileBytes, LedgerFiles},
    FrontierStore, PersistError, VerifyMode,
};
use crate::{
    p2p::channels::rpc::StagedLedgerAuxAndPendingCoinbases,
    transition_frontier::body_reference::validate_body_reference,
};

/// After this many unfinished restores of one generation, only its snarked
/// ledgers are loaded (R20 D5).
pub const LEDGERS_ONLY_AFTER_ATTEMPTS: u32 = 2;

/// After this many, the store is deleted.
pub const DISCARD_AFTER_ATTEMPTS: u32 = 3;

/// The counted attempt of a restore in progress, which a clean shutdown takes
/// back (RC7-FIXES). Armed with the record as it was when the loader counts
/// the attempt; ended when the restore is committed, given up or failed, or
/// the record is cleared or deleted. A crash (a panic, an abort, the kernel's
/// OOM kill) never takes it back, so a store that crashes the node on every
/// restore is still given up. Clones share the restore; the native node hands
/// one to its signal thread.
///
/// Counting and shutdown are ordered by one lock. The loader writes the new
/// count beside the record, then, under the lock, commits it (a rename) and
/// arms the take-back together, unless a shutdown has begun. A shutdown
/// marks itself under the lock and takes what is armed. So a stop before the
/// commit leaves the record untouched, and one after it takes the count back.
#[derive(Clone, Default)]
pub struct RestoreGuard(Arc<Mutex<GuardState>>);

#[derive(Default)]
struct GuardState {
    shutting_down: bool,
    armed: Option<TakeBack>,
}

/// Writes the attempts record back as it was before the restore.
pub type TakeBack = Box<dyn FnOnce() -> io::Result<()> + Send>;

impl std::fmt::Debug for RestoreGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.lock();
        f.debug_struct("RestoreGuard")
            .field("shutting_down", &state.shutting_down)
            .field("armed", &state.armed.is_some())
            .finish()
    }
}

impl RestoreGuard {
    fn lock(&self) -> std::sync::MutexGuard<'_, GuardState> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Commits a counted attempt with `commit` and arms `take_back`, both
    /// under the lock a shutdown takes. `false`, with nothing committed, once
    /// a shutdown has begun. A fall-back to the other generation keeps the
    /// first take-back: the record as it was before the restore.
    pub(crate) fn commit_counted(
        &self,
        take_back: TakeBack,
        commit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<bool> {
        let mut state = self.lock();
        if state.shutting_down {
            return Ok(false);
        }
        commit()?;
        state.armed.get_or_insert(take_back);
        Ok(true)
    }

    /// The restore is over: its attempt stands, or the record is gone.
    pub fn end(&self) {
        self.lock().armed.take();
    }

    /// Whether a restore's attempt is counted and not yet ended.
    pub fn in_progress(&self) -> bool {
        self.lock().armed.is_some()
    }

    /// For a clean shutdown: from now on no attempt is committed. Returns the
    /// take-back of the restore in progress, if there is one, to run off the
    /// lock.
    pub fn begin_shutdown(&self) -> Option<TakeBack> {
        let mut state = self.lock();
        state.shutting_down = true;
        state.armed.take()
    }

    /// [`Self::begin_shutdown`] if no restore is armed and the lock is free
    /// now, without waiting for it; whether it did.
    pub fn shutdown_if_idle(&self) -> bool {
        let mut state = match self.0.try_lock() {
            Ok(state) => state,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return false,
        };
        if state.armed.is_some() {
            return false;
        }
        state.shutting_down = true;
        true
    }

    /// [`Self::begin_shutdown`], then the take-back and its result.
    pub fn take_back(&self) -> Option<io::Result<()>> {
        self.begin_shutdown().map(|take_back| take_back())
    }
}

/// How many slots a stored tip may be ahead of the clock: clocks differ, but
/// not by minutes.
pub const AHEAD_OF_CLOCK_SLOTS: u32 = 10;

/// The most threads a load's checks use (PC3-KILL). Each thread verifying a
/// batch of proofs or rehashing a ledger holds its own working memory, so on
/// the node-wide pool (one thread fewer than the CPUs) a restore's peak grew
/// with the core count: 785 to 914 MiB of anonymous memory on 32 CPUs against
/// 562 MiB on 4, under a 1 GB limit, where a bootstrap peaks at about 440.
pub const LOAD_MAX_THREADS: usize = 4;

/// The load's threads for `available_cpus`: one fewer, as the node-wide
/// pool has, at most [`LOAD_MAX_THREADS`] and at least 1.
pub fn load_threads(available_cpus: usize) -> usize {
    available_cpus.saturating_sub(1).clamp(1, LOAD_MAX_THREADS)
}

/// The pool a load's parallel checks run on: [`load_threads`] threads of its
/// own, so that the proofs, the ledgers' rehash and the parts' hashing,
/// with the rayon work they start inside (`verify_blocks`, the ledger codec),
/// stay off the node-wide pool and within the bound. Its threads end with the
/// load. In the browser, or if the threads can't be started, the work runs
/// where it is called, on the node-wide pool, as before.
struct LoadPool {
    #[cfg(not(target_family = "wasm"))]
    pool: Option<rayon::ThreadPool>,
}

impl LoadPool {
    fn new() -> Self {
        #[cfg(not(target_family = "wasm"))]
        {
            let cpus = std::thread::available_parallelism().map_or(1, |cpus| cpus.get());
            let threads = load_threads(cpus);
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|i| format!("mina_load_{i}"))
                .build();
            match &pool {
                Ok(_) => mina_core::info!(
                    mina_core::log::system_time();
                    kind = "PersistLoad",
                    summary = format!("frontier store load checks on {threads} threads"),
                    threads = threads,
                    available_cpus = cpus,
                ),
                Err(error) => mina_core::warn!(
                    mina_core::log::system_time();
                    kind = "PersistLoad",
                    summary = "the load's threads couldn't start; its checks use the node's pool",
                    error = error.to_string(),
                ),
            }
            Self { pool: pool.ok() }
        }
        #[cfg(target_family = "wasm")]
        Self {}
    }

    fn run<R: Send>(&self, work: impl FnOnce() -> R + Send) -> R {
        #[cfg(not(target_family = "wasm"))]
        if let Some(pool) = &self.pool {
            return pool.install(work);
        }
        work()
    }
}

/// What the node asks the loader for.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistLoadRequest {
    /// The global slot the clock is at (as `ChainEntry::global_slot`), if the
    /// node knows it.
    pub current_slot: Option<u32>,
    /// A stored tip more than this many slots behind the clock is too old to
    /// replay: R20 D5's 4k (1,160 slots, 29 hours on mainnet).
    pub stale_slots: u32,
    /// Our genesis block: every stored block must name it as its genesis
    /// state and have its constants. `None` skips those two checks.
    pub genesis: Option<ArcBlockWithHash>,
}

/// The blockchain verifier the stored blocks' proofs are checked with (the
/// node's, `state.snark.block_verify`). Without one (`load(.., None)`, as a
/// node run with proof verification off does) the proofs are not checked.
#[derive(Clone)]
pub struct BlockProofVerifier {
    pub index: BlockVerifier,
    pub srs: Arc<VerifierSRS>,
}

/// Where a load's time went, on the store thread (milliseconds).
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct PersistLoadTimings {
    /// Reading, decoding, rehashing and linking the blocks.
    pub blocks_ms: f64,
    /// The blocks' bodies and header checks.
    #[serde(default)]
    pub block_checks_ms: f64,
    /// The blocks' blockchain proofs (0 when not checked).
    #[serde(default)]
    pub proofs_ms: f64,
    #[serde(default)]
    pub proofs_checked: u32,
    /// Reading the ledger and parts files and checking their lengths and
    /// CRC32s (I/O, before the checks).
    #[serde(default)]
    pub files_ms: f64,
    /// The checks after the files are read, from start to end: the blocks'
    /// checks and proofs, the ledgers and the parts run at once, so this is
    /// about the longest of them, not their sum (I5d4).
    #[serde(default)]
    pub checks_ms: f64,
    /// Decoding and rehashing the snarked ledgers.
    pub ledgers_ms: f64,
    /// CPU wall time summed across the snarked ledgers' full hash checks.
    #[serde(default)]
    pub ledger_verify_ms: f64,
    /// Sum of file decode and ledger construction time outside hash checks.
    #[serde(default)]
    pub ledger_rebuild_ms: f64,
    /// Decoding and rehashing the root staged ledger.
    pub staged_ms: f64,
    /// Time rehashing the root staged ledger's changed paths.
    #[serde(default)]
    pub staged_verify_ms: f64,
    /// Checking derived indexes against the verified leaves (also included
    /// in `total_ms`, separate from ledger decoding and rehashing).
    #[serde(default)]
    pub indexes_ms: f64,
    #[serde(default)]
    pub accounts_checked: u64,
    pub parts_ms: f64,
    /// The accounts the root's scan state references that the root staged
    /// ledger's mask now holds a copy of, and the time that took
    /// ([`hold_referenced_accounts`], I5d5).
    #[serde(default)]
    pub held_accounts: u64,
    #[serde(default)]
    pub hold_ms: f64,
    /// `from_persisted_unchecked` and the complete staged ledger hash.
    pub hash_ms: f64,
    /// Handing the ledgers to the ledger thread, waiting for its answer.
    pub insert_ms: f64,
    /// The ledger thread's own time on the insert.
    pub ledger_thread_ms: f64,
    pub total_ms: f64,
}

/// A store the loader trusts: what the restore needs.
pub struct Restored {
    pub generation: u64,
    /// This load's number among the generation's restore attempts, from 1.
    pub attempt: u32,
    /// The snarked ledgers the manifest lists, by hash.
    pub ledgers: BTreeMap<v2::LedgerHash, Mask>,
    /// The root staged ledger, and its complete hash (the root block's).
    pub staged_ledger_hash: v2::MinaBaseStagedLedgerHashStableV1,
    pub staged: StagedLedger,
    pub root: ArcBlockWithHash,
    /// The stored chain after the root, oldest first.
    pub blocks: Vec<ArcBlockWithHash>,
    /// The protocol states the root's scan state needs.
    pub needed_protocol_states: BTreeMap<v2::StateHash, v2::MinaStateProtocolStateValueStableV2>,
    pub timings: PersistLoadTimings,
}

pub enum LoadOutcome {
    /// Nothing to load.
    Absent {
        reason: String,
    },
    /// Only the snarked ledgers, as copy origins for the bootstrap that
    /// follows.
    LedgersOnly {
        reason: String,
        generation: u64,
        ledgers: BTreeMap<v2::LedgerHash, Mask>,
        timings: PersistLoadTimings,
    },
    Restored(Box<Restored>),
    /// The store failed a check, or too many restores of it didn't finish:
    /// it was deleted.
    Discarded {
        reason: String,
    },
}

impl LoadOutcome {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Absent { .. } => "absent",
            Self::LedgersOnly { .. } => "ledgers only",
            Self::Restored(_) => "restored",
            Self::Discarded { .. } => "discarded",
        }
    }
}

/// Why a generation didn't load.
enum Failure {
    /// The store is damaged, or not what its manifest says: give up this
    /// generation.
    Corrupt(String),
    /// Reading failed for a reason that may pass (an I/O error): keep the
    /// store, bootstrap this time.
    Error(PersistError),
}

impl From<CodecError> for Failure {
    fn from(error: CodecError) -> Self {
        PersistError::from(error).into()
    }
}

impl From<PersistError> for Failure {
    fn from(error: PersistError) -> Self {
        if is_transient(&error) {
            Self::Error(error)
        } else {
            Self::Corrupt(error.to_string())
        }
    }
}

/// An I/O error that says nothing about the store's content. A short read or
/// invalid data is damage; anything else (EIO, a permission, a busy handle)
/// may pass, and must not get a valid store deleted.
pub(crate) fn is_transient(error: &PersistError) -> bool {
    fn transient(error: &io::Error) -> bool {
        !matches!(
            error.kind(),
            io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData
        )
    }
    match error {
        PersistError::Io(error) => transient(error),
        PersistError::Codec(CodecError::Io(error)) => transient(error),
        PersistError::Locked | PersistError::Codec(CodecError::Cancelled) => true,
        _ => false,
    }
}

fn elapsed_ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Loads the store, applying the policy above. `Err` only for an error that
/// leaves the store as it is (an I/O error); every other failure is an
/// outcome. Without a `verifier` the blocks' proofs are not checked.
pub fn load<B: StoreBackend>(
    store: &mut FrontierStore<B>,
    request: &PersistLoadRequest,
    verifier: Option<&BlockProofVerifier>,
) -> Result<LoadOutcome, PersistError> {
    load_with_progress(store, request, verifier, &LoadControl::default())
}

/// The production loader publishes completed work for the node's idle wait
/// policy: reads, decode batches, hash batches and verified proofs. Completion
/// of one piece never implies the complete store has passed validation.
pub fn load_with_progress<B: StoreBackend>(
    store: &mut FrontierStore<B>,
    request: &PersistLoadRequest,
    verifier: Option<&BlockProofVerifier>,
    control: &LoadControl,
) -> Result<LoadOutcome, PersistError> {
    let mut given_up = Vec::new();
    let pool = LoadPool::new();
    loop {
        control.check()?;
        match load_generation(store, request, verifier, control, &pool) {
            Ok(outcome) => return Ok(outcome),
            Err(Failure::Error(error)) => return Err(error),
            Err(Failure::Corrupt(reason)) => {
                let generation = store.generation();
                mina_core::warn!(
                    mina_core::log::system_time();
                    kind = "PersistLoad",
                    summary = format!("frontier store generation {generation} failed a check"),
                    reason = reason.clone(),
                );
                given_up.push(format!("generation {generation}: {reason}"));
                // The other slot, if it holds a valid generation; at most
                // two generations exist.
                control.check()?;
                if given_up.len() < 2 && store.fall_back()? {
                    continue;
                }
                control.check()?;
                store.reset()?;
                return Ok(LoadOutcome::Discarded {
                    reason: given_up.join("; "),
                });
            }
        }
    }
}

fn load_generation<B: StoreBackend>(
    store: &mut FrontierStore<B>,
    request: &PersistLoadRequest,
    verifier: Option<&BlockProofVerifier>,
    control: &LoadControl,
    pool: &LoadPool,
) -> Result<LoadOutcome, Failure> {
    control.check()?;
    let started = Instant::now();
    let Some(manifest) = store.manifest().cloned() else {
        return Ok(LoadOutcome::Absent {
            reason: "the store is empty".into(),
        });
    };
    let generation = manifest.generation;

    // The attempts record first, so that a load that panics counts.
    let previous = store.attempts()?;
    let prior = previous
        .filter(|attempts| attempts.generation == generation)
        .map_or(0, |attempts| attempts.count);
    if prior >= DISCARD_AFTER_ATTEMPTS {
        store.reset()?;
        return Ok(LoadOutcome::Discarded {
            reason: format!(
                "{prior} restores of generation {generation} didn't finish; the store is deleted"
            ),
        });
    }
    store.count_attempt(
        previous,
        Attempts {
            generation,
            count: prior + 1,
        },
    )?;

    let mut timings = PersistLoadTimings::default();
    if let Some(reason) = ledgers_only_reason(&manifest, request, prior) {
        if manifest.ledgers.is_empty() {
            return Ok(LoadOutcome::Absent { reason });
        }
        return ledgers_only(store, &manifest, reason, started, control, pool);
    }
    let checkpoint = manifest
        .checkpoint
        .clone()
        .ok_or_else(|| Failure::Corrupt("no checkpoint".into()))?;

    // The blocks: their hashes recomputed, linked, the first the root.
    let reading = Instant::now();
    let blocks = read_chain(store, &manifest, control)?;
    timings.blocks_ms = elapsed_ms(reading);
    let (root, blocks) = blocks
        .split_first()
        .map(|(root, rest)| (root.clone(), rest.to_vec()))
        .ok_or_else(|| Failure::Corrupt("an empty chain".into()))?;
    if root.hash() != &checkpoint.root
        || root.snarked_ledger_hash() != &checkpoint.root_snarked_ledger
        || root.staged_ledger_hashes() != &checkpoint.root_staged_ledger_hash
        || root.height() != checkpoint.root_height
    {
        return Err(Failure::Corrupt(
            "the chain's first block is not the checkpoint's root".into(),
        ));
    }
    let tip = blocks.last().unwrap_or(&root);
    if !manifest.has_ledger(tip.staking_epoch_ledger_hash()) {
        return ledgers_only(
            store,
            &manifest,
            format!(
                "the tip's staking ledger {} is not stored",
                tip.staking_epoch_ledger_hash()
            ),
            started,
            control,
            pool,
        );
    }
    control.completed(1)?;

    // Every file the checks need, read and checked against its length and
    // CRC32 before anything decodes it (I/O, on this thread). Then the
    // checks, which don't depend on each other, at once (I5d4): the blocks'
    // bodies, headers and proofs; the ledgers, each rehashed; the parts. On
    // the load's bounded pool, so that the memory they hold at once doesn't
    // grow with the core count (PC3-KILL).
    let reading = Instant::now();
    let ledger_files = store.read_ledger_files_controlled(&manifest, &manifest.ledgers, control)?;
    let staged_file = store
        .read_staged_file_controlled(&manifest, control)
        .map_err(|error| staged_failure(&checkpoint.root, error))?;
    let (parts_name, parts_bytes) = read_parts_file(store, &manifest, control)?;
    timings.files_ms = elapsed_ms(reading);
    control.completed(1)?;

    let checking = Instant::now();
    let chain = std::iter::once(&root)
        .chain(blocks.iter())
        .collect::<Vec<_>>();
    let ((checked, loaded), parts) = pool.run(|| {
        rayon::join(
            || {
                rayon::join(
                    || check_chain(&chain, request.genesis.as_ref(), verifier, control),
                    || load_ledgers(ledger_files, staged_file, &checkpoint, control),
                )
            },
            || {
                let loading = Instant::now();
                let parts = decode_parts(
                    &parts_name,
                    parts_bytes,
                    &checkpoint.root_staged_ledger_hash,
                    control,
                );
                (parts, elapsed_ms(loading))
            },
        )
    });
    control.check()?;
    timings.checks_ms = elapsed_ms(checking);
    // In the order they ran in one after another, so that a store failing
    // more than one reports the same failure.
    let checked = checked?;
    timings.block_checks_ms = checked.block_checks_ms;
    timings.proofs_ms = checked.proofs_ms;
    timings.proofs_checked = checked.proofs_checked;
    let LoadedLedgers {
        ledgers,
        staged: staged_mask,
        ledgers_ms,
        ledger_verify_ms,
        ledger_rebuild_ms,
        staged_ms,
        staged_verify_ms,
        indexes_ms,
        accounts_checked,
    } = loaded?;
    timings.ledgers_ms = ledgers_ms;
    timings.ledger_verify_ms = ledger_verify_ms;
    timings.ledger_rebuild_ms = ledger_rebuild_ms;
    timings.staged_ms = staged_ms;
    timings.staged_verify_ms = staged_verify_ms;
    timings.indexes_ms = indexes_ms;
    timings.accounts_checked = accounts_checked;
    let (parts, parts_ms) = parts;
    let (scan_state, pending_coinbase, needed_protocol_states) = parts?;
    timings.parts_ms = parts_ms;
    control.completed(1)?;
    // After the parts and the ledgers, before the hash gate (I5d5).
    let holding = Instant::now();
    timings.held_accounts = hold_referenced_accounts(&staged_mask, &scan_state) as u64;
    timings.hold_ms = elapsed_ms(holding);

    // I5ab's gate: the complete staged ledger hash (ledger, scan state,
    // pending coinbase) is the root block's.
    let hashing = Instant::now();
    let mut staged = StagedLedger::from_persisted_unchecked(
        mina_core::constants::constraint_constants(),
        scan_state,
        staged_mask,
        pending_coinbase,
    );
    let staged_ledger_hash = v2::MinaBaseStagedLedgerHashStableV1::from(&staged.hash());
    timings.hash_ms = elapsed_ms(hashing);
    if &staged_ledger_hash != root.staged_ledger_hashes() {
        return Err(Failure::Corrupt(
            "the root staged ledger's complete hash is not the root block's".into(),
        ));
    }
    control.completed(1)?;
    timings.total_ms = elapsed_ms(started);
    Ok(LoadOutcome::Restored(Box::new(Restored {
        generation,
        attempt: prior + 1,
        ledgers,
        staged_ledger_hash,
        staged,
        root,
        blocks,
        needed_protocol_states,
        timings,
    })))
}

/// Why only the ledgers are loaded, if they are: too many unfinished
/// restores, no checkpoint, or a stored tip too far from the clock.
fn ledgers_only_reason(
    manifest: &Manifest,
    request: &PersistLoadRequest,
    prior_attempts: u32,
) -> Option<String> {
    if prior_attempts >= LEDGERS_ONLY_AFTER_ATTEMPTS {
        return Some(format!(
            "{prior_attempts} restores of generation {} didn't finish",
            manifest.generation
        ));
    }
    if manifest.checkpoint.is_none() {
        return Some("the store has no checkpoint".into());
    }
    let tip = manifest.chain.last()?;
    let now = request.current_slot?;
    if now > tip.global_slot.saturating_add(request.stale_slots) {
        return Some(format!(
            "the stored tip is at slot {}, {} slots behind the clock (more than {})",
            tip.global_slot,
            now - tip.global_slot,
            request.stale_slots
        ));
    }
    if tip.global_slot > now.saturating_add(AHEAD_OF_CLOCK_SLOTS) {
        return Some(format!(
            "the stored tip is at slot {}, ahead of the clock's {now}",
            tip.global_slot
        ));
    }
    None
}

/// Only the snarked ledgers, each rehashed in full: a copy origin's hashes
/// decide which subtrees a ledger sync skips, so they must be recomputed,
/// never taken from the store.
fn ledgers_only<B: StoreBackend>(
    store: &mut FrontierStore<B>,
    manifest: &Manifest,
    reason: String,
    started: Instant,
    control: &LoadControl,
    pool: &LoadPool,
) -> Result<LoadOutcome, Failure> {
    let reading = Instant::now();
    let files = store.read_ledger_files_controlled(manifest, &manifest.ledgers, control)?;
    let mut timings = PersistLoadTimings {
        files_ms: elapsed_ms(reading),
        ..Default::default()
    };
    control.completed(1)?;
    let loading = Instant::now();
    let (ledgers, ledger_verify_ms, ledger_rebuild_ms) =
        pool.run(|| decode_ledgers_controlled(files, VerifyMode::Full, control))?;
    timings.ledgers_ms = elapsed_ms(loading);
    timings.ledger_verify_ms = ledger_verify_ms;
    timings.ledger_rebuild_ms = ledger_rebuild_ms;
    let (indexes_ms, accounts_checked) =
        pool.run(|| check_all_indexes(ledgers.values(), control))?;
    timings.indexes_ms = indexes_ms;
    timings.accounts_checked = accounts_checked;
    control.completed(1)?;
    timings.total_ms = elapsed_ms(started);
    Ok(LoadOutcome::LedgersOnly {
        reason,
        generation: manifest.generation,
        ledgers,
        timings,
    })
}

/// A diff can restore every correct leaf and hash while damaging a lookup
/// index during account replacement. Fail this generation before any ledger
/// is handed over, including copy origins and the root staged ledger.
/// Returns the time it took and the accounts checked.
fn check_indexes(ledger: &Mask) -> Result<(f64, u64), Failure> {
    let checking = Instant::now();
    let count = ledger
        .validate_account_indexes()
        .map_err(|reason| Failure::Corrupt(format!("ledger derived indexes: {reason}")))?;
    Ok((elapsed_ms(checking), count as u64))
}

/// [`check_indexes`] of each ledger, in parallel (each ledger is its own
/// root, used by one thread); the first failure in order, the times summed.
fn check_all_indexes<'a>(
    ledgers: impl Iterator<Item = &'a Mask>,
    control: &LoadControl,
) -> Result<(f64, u64), Failure> {
    let ledgers = ledgers.collect::<Vec<_>>();
    let checked = ledgers
        .par_iter()
        .map(|ledger| -> Result<_, Failure> {
            control.check()?;
            let checked = check_indexes(ledger)?;
            control.completed(1)?;
            Ok(checked)
        })
        .collect::<Vec<_>>();
    checked
        .into_iter()
        .try_fold((0.0, 0), |(ms, count), checked| {
            let (more_ms, more) = checked?;
            Ok((ms + more_ms, count + more))
        })
}

/// What [`check_chain`] took.
struct ChainChecked {
    block_checks_ms: f64,
    proofs_ms: f64,
    proofs_checked: u32,
}

/// Each block, the root included, as a peer's would be checked: body,
/// header ([`check_blocks`]), and its blockchain proof, all of them at once,
/// unless the node runs without a verifier.
fn check_chain(
    chain: &[&ArcBlockWithHash],
    genesis: Option<&ArcBlockWithHash>,
    verifier: Option<&BlockProofVerifier>,
    control: &LoadControl,
) -> Result<ChainChecked, Failure> {
    let checking = Instant::now();
    check_blocks(chain, genesis, control)?;
    let mut checked = ChainChecked {
        block_checks_ms: elapsed_ms(checking),
        proofs_ms: 0.0,
        proofs_checked: 0,
    };
    if let Some(verifier) = verifier {
        let proving = Instant::now();
        let headers = chain.iter().map(|block| block.header()).collect::<Vec<_>>();
        // Bound uncancellable proof work to one kimchi batch. Nothing is
        // accepted unless all batches pass both accumulator and proof checks.
        for batch in headers.chunks(32) {
            control.check()?;
            if !ledger::proofs::verification::verify_blocks(batch, &verifier.index, &verifier.srs) {
                control.check()?;
                return Err(Failure::Corrupt(format!(
                    "the blockchain proofs of the {} stored blocks don't all verify",
                    headers.len()
                )));
            }
            control.completed(batch.len() as u64)?;
        }
        checked.proofs_ms = elapsed_ms(proving);
        checked.proofs_checked = headers.len() as u32;
    }
    Ok(checked)
}

/// The ledgers a restore hands over, and what loading them took.
struct LoadedLedgers {
    ledgers: BTreeMap<v2::LedgerHash, Mask>,
    staged: Mask,
    ledgers_ms: f64,
    ledger_verify_ms: f64,
    ledger_rebuild_ms: f64,
    staged_ms: f64,
    staged_verify_ms: f64,
    indexes_ms: f64,
    accounts_checked: u64,
}

/// The snarked ledgers, each rehashed, and their derived indexes checked;
/// then the root staged ledger over the root snarked one, rehashed, and its
/// indexes checked. The root staged ledger is a child mask of the root
/// snarked ledger (I5d5), so all of this runs on one thread, in this order:
/// a mask reads its parent, and `Mask::with` panics when two threads use a
/// mask and its parent at once.
fn load_ledgers(
    files: LedgerFiles,
    staged_file: LedgerFileBytes,
    checkpoint: &Checkpoint,
    control: &LoadControl,
) -> Result<LoadedLedgers, Failure> {
    let loading = Instant::now();
    let (ledgers, ledger_verify_ms, ledger_rebuild_ms) =
        decode_ledgers_controlled(files, VerifyMode::Full, control)?;
    let ledgers_ms = elapsed_ms(loading);
    // Each snarked ledger is its own root: their checks may run in parallel.
    let (mut indexes_ms, mut accounts_checked) = check_all_indexes(ledgers.values(), control)?;
    let root_snarked = ledgers
        .get(&checkpoint.root_snarked_ledger)
        .ok_or_else(|| Failure::Corrupt("the root snarked ledger is not loaded".into()))?;
    let loading = Instant::now();
    // A child mask of the root snarked ledger with the diff applied, as
    // OCaml's persistent frontier builds the root staged ledger over a mask of
    // the root ledger database (I5d5; `persistent_frontier.ml`,
    // `construct_staged_ledger_at_root`). It holds only the diff's accounts
    // and hashes. The commit after the replay folds the new root's mask chain
    // into it and stops there, under the snarked ledger
    // (`LedgerCtx::commit_root_staged_ledger`, R38).
    let (staged, staged_verify_ms) = staged_file
        .base_child(root_snarked)
        .and_then(|base| staged_file.decode_controlled(Some(base), VerifyMode::Full, control))
        .map_err(|error| staged_failure(&checkpoint.root, error))?;
    let staged_ms = elapsed_ms(loading);
    let (staged_indexes_ms, staged_accounts) = check_staged_indexes(&staged)?;
    indexes_ms += staged_indexes_ms;
    accounts_checked += staged_accounts;
    Ok(LoadedLedgers {
        ledgers,
        staged,
        ledgers_ms,
        ledger_verify_ms,
        ledger_rebuild_ms,
        staged_ms,
        staged_verify_ms,
        indexes_ms,
        accounts_checked,
    })
}

/// A failure of the root staged ledger's file, named.
fn staged_failure(root: &v2::StateHash, error: PersistError) -> Failure {
    match Failure::from(error) {
        Failure::Corrupt(reason) => Failure::Corrupt(format!("{}: {reason}", staged_name(root))),
        other => other,
    }
}

/// [`check_indexes`] for the root staged ledger, a mask over the root
/// snarked ledger (whose own indexes are checked with the other ledgers'):
/// its own indexes, and its accounts against the snarked ledger's. Run after
/// the snarked ledger's check, not beside it: the mask reads its parent.
/// Returns the time it took and the accounts checked.
fn check_staged_indexes(staged: &Mask) -> Result<(f64, u64), Failure> {
    let checking = Instant::now();
    let count = staged
        .validate_child_account_indexes()
        .map_err(|reason| Failure::Corrupt(format!("ledger derived indexes: {reason}")))?;
    Ok((elapsed_ms(checking), count as u64))
}

/// Makes the restored root staged ledger's mask hold its own copy of every
/// account the root's scan state's transactions reference, as a staged ledger
/// rebuilt over the snarked ledger holds every account it set (R38's
/// bootstrap; OCaml's `construct_staged_ledger_at_root` applies them to a
/// mask over the root ledger). The diff holds only the accounts that differ
/// from the snarked ledger: one the transactions changed and changed back
/// would otherwise be read through from the snarked ledger, and a later root
/// snarked ledger part way through those transactions would show through the
/// root staged ledger (`compact_snarked_ledgers`). Holding an account the
/// transactions didn't set is harmless: until the root moves past it only
/// they change it. Doesn't change what the mask reads. Returns how many
/// accounts it now holds that it didn't.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_frontier/persistent_frontier/persistent_frontier.ml#L59-L73>
pub(crate) fn hold_referenced_accounts(staged: &Mask, scan_state: &ScanState) -> usize {
    staged.hold_parent_accounts(scan_state.staged_accounts_referenced())
}

/// The checks of a received block that don't depend on the time
/// (`prevalidate_block` less its timing check), and its body against its
/// header, for every stored block, the root included (the replay applies only
/// the blocks after it).
fn check_blocks(
    chain: &[&ArcBlockWithHash],
    genesis: Option<&ArcBlockWithHash>,
    control: &LoadControl,
) -> Result<(), Failure> {
    chain.par_iter().try_for_each(|block| {
        control.check()?;
        let check = || -> Result<(), prevalidate::BlockPrevalidationError> {
            validate_body_reference(&block.block)?;
            prevalidate::validate_stop_slots(block, mina_core::constants::stop_slots())?;
            prevalidate::validate_delta_block_chain(block.header())?;
            prevalidate::validate_protocol_versions(block)?;
            if let Some(genesis) = genesis {
                prevalidate::validate_genesis_state(block, genesis)?;
                prevalidate::validate_constants(block, genesis)?;
            }
            Ok(())
        };
        check().map_err(|error| {
            Failure::Corrupt(format!(
                "block {} at {}: {error:?}",
                block.hash(),
                block.height()
            ))
        })?;
        control.completed(1)?;
        Ok(())
    })
}

/// Every block of the manifest's chain, checked against its entry and linked
/// to the one before.
fn read_chain<B: StoreBackend>(
    store: &mut FrontierStore<B>,
    manifest: &Manifest,
    control: &LoadControl,
) -> Result<Vec<ArcBlockWithHash>, Failure> {
    let mut blocks: Vec<ArcBlockWithHash> = Vec::with_capacity(manifest.chain.len());
    for entry in &manifest.chain {
        control.check()?;
        let name = block_name(&entry.hash);
        let file = manifest
            .file(&name)
            .cloned()
            .ok_or_else(|| Failure::Corrupt(format!("{name} is not referenced")))?;
        let bytes = store.read_blob_controlled(&file, control)?;
        let block = v2::MinaBlockBlockStableV2::binprot_read(&mut bytes.as_slice())
            .map_err(|error| Failure::Corrupt(format!("{name}: {error}")))?;
        let block = ArcBlockWithHash::try_new(Arc::new(block))
            .map_err(|error| Failure::Corrupt(format!("{name}: {error:?}")))?;
        if block.hash() != &entry.hash {
            return Err(Failure::Corrupt(format!(
                "{name} holds block {}",
                block.hash()
            )));
        }
        if block.height() != entry.height || block.global_slot() != entry.global_slot {
            return Err(Failure::Corrupt(format!(
                "{name}: height {} and slot {}, recorded {} and {}",
                block.height(),
                block.global_slot(),
                entry.height,
                entry.global_slot
            )));
        }
        if let Some(previous) = blocks.last() {
            if block.pred_hash() != previous.hash() || block.height() != previous.height() + 1 {
                return Err(Failure::Corrupt(format!(
                    "{name}: its parent is {}, not {}",
                    block.pred_hash(),
                    previous.hash()
                )));
            }
        }
        blocks.push(block);
        control.completed(1)?;
    }
    Ok(blocks)
}

type Parts = (
    ScanState,
    PendingCoinbase,
    BTreeMap<v2::StateHash, v2::MinaStateProtocolStateValueStableV2>,
);

/// The checkpoint's parts file, read and checked, and its name.
fn read_parts_file<B: StoreBackend>(
    store: &mut FrontierStore<B>,
    manifest: &Manifest,
    control: &LoadControl,
) -> Result<(String, Vec<u8>), Failure> {
    let checkpoint = manifest
        .checkpoint
        .as_ref()
        .ok_or_else(|| Failure::Corrupt("no checkpoint".into()))?;
    let name = parts_name(&checkpoint.root);
    let file = manifest
        .file(&name)
        .cloned()
        .ok_or_else(|| Failure::Corrupt(format!("{name} is not referenced")))?;
    let bytes = store.read_blob_controlled(&file, control)?;
    Ok((name, bytes))
}

/// The root's scan state, pending coinbase and protocol states, checked to
/// be of the root's staged ledger and to hold every protocol state the scan
/// state needs.
fn decode_parts(
    name: &str,
    bytes: Vec<u8>,
    root_staged_ledger_hash: &v2::MinaBaseStagedLedgerHashStableV1,
    control: &LoadControl,
) -> Result<Parts, Failure> {
    control.check()?;
    let corrupt = |reason: String| Failure::Corrupt(format!("{name}: {reason}"));
    let parts = StagedLedgerAuxAndPendingCoinbases::binprot_read(&mut bytes.as_slice())
        .map_err(|error| corrupt(error.to_string()))?;
    drop(bytes);
    if parts.staged_ledger_hash != root_staged_ledger_hash.non_snark.ledger_hash {
        return Err(corrupt("the parts are of another staged ledger".into()));
    }
    let scan_state: ScanState = (&parts.scan_state)
        .try_into()
        .map_err(|error| corrupt(format!("scan state: {error:?}")))?;
    let pending_coinbase: PendingCoinbase = (&parts.pending_coinbase)
        .try_into()
        .map_err(|error| corrupt(format!("pending coinbase: {error:?}")))?;
    // Each protocol state hashed (in parallel: 556 to 563 of them on
    // mainnet), in order, so that the first failure is the one reported.
    let hashed = parts
        .needed_blocks
        .par_iter()
        .map(|state| {
            control.check()?;
            let hash = state
                .try_hash()
                .map_err(|error| corrupt(format!("protocol state: {error:?}")))?;
            let field = hash
                .to_field::<Fp>()
                .map_err(|error| corrupt(format!("protocol state: {error:?}")))?;
            control.completed(1)?;
            Ok((hash, field))
        })
        .collect::<Vec<Result<_, Failure>>>();
    let mut by_field = HashMap::with_capacity(parts.needed_blocks.len());
    let mut states = BTreeMap::new();
    for (state, hashed) in parts.needed_blocks.iter().zip(hashed) {
        let (hash, field) = hashed?;
        by_field.insert(field, ());
        states.insert(hash, state.clone());
    }
    let missing = scan_state
        .required_state_hashes()
        .into_iter()
        .filter(|hash| !by_field.contains_key(hash))
        .count();
    if missing > 0 {
        return Err(corrupt(format!(
            "{missing} protocol states the scan state needs are missing"
        )));
    }
    Ok((scan_state, pending_coinbase, states))
}

#[cfg(test)]
mod pool_tests {
    use super::*;

    #[test]
    fn load_threads_are_one_fewer_than_the_cpus_at_most_four() {
        for (cpus, threads) in [
            (0, 1),
            (1, 1),
            (2, 1),
            (3, 2),
            (4, 3),
            (5, 4),
            (16, 4),
            (32, 4),
            (256, 4),
        ] {
            assert_eq!(load_threads(cpus), threads, "{cpus} CPUs");
        }
    }

    /// The bound holds for the rayon work the checks start inside, as
    /// `verify_blocks` and the ledger codec do: nested joins and parallel
    /// iterators run on the load's own threads, never on the node-wide pool.
    #[test]
    fn a_loads_nested_rayon_work_stays_on_its_bounded_pool() {
        let pool = LoadPool::new();
        let cpus = std::thread::available_parallelism().map_or(1, |cpus| cpus.get());
        let (threads, names) = pool.run(|| {
            let threads = rayon::current_num_threads();
            let ((a, b), c) = rayon::join(
                || {
                    rayon::join(
                        || {
                            (0..512)
                                .into_par_iter()
                                .map(|_| std::thread::current().name().map(str::to_owned))
                                .collect::<Vec<_>>()
                        },
                        || vec![std::thread::current().name().map(str::to_owned)],
                    )
                },
                || {
                    (0..512)
                        .into_par_iter()
                        .map(|_| std::thread::current().name().map(str::to_owned))
                        .collect::<Vec<_>>()
                },
            );
            (threads, [a, b, c].concat())
        });
        assert_eq!(threads, load_threads(cpus));
        assert!(threads <= LOAD_MAX_THREADS);
        let distinct = names.iter().collect::<std::collections::BTreeSet<_>>();
        assert!(distinct.len() <= LOAD_MAX_THREADS, "{distinct:?}");
        for name in names {
            assert!(
                name.as_deref()
                    .is_some_and(|name| name.starts_with("mina_load_")),
                "ran on {name:?}"
            );
        }
    }
}
