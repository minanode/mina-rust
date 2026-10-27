//! The store thread (R20 D3): the only writer of a node's frontier store.
//!
//! It owns the [`FrontierStore`], which is built on this thread because a
//! backend need not be `Send`. The node hands it jobs over a channel: open the
//! store once the chain id is known, load it for a restore (I5d1, see
//! [`super::load`]), put each new best chain, and write the checkpoints the
//! ledger thread snapshots. It reports each result as a [`PersistEvent`].
//!
//! A load runs here, not on the ledger thread or the node thread (D3): it
//! takes seconds on mainnet. The ledgers it builds are handed to the ledger
//! thread whole, and this thread doesn't touch them again (a `Mask` must only
//! be used by one thread).
//!
//! Jobs run in the order they were sent. A checkpoint is sent by the ledger
//! thread after the node sent the chain put of the same best chain, so the
//! chain's blocks are stored by the time the checkpoint needs them.

use std::{
    collections::BTreeSet,
    io::{self, Write},
    sync::{mpsc, Arc, Mutex},
};

use ledger::{
    scan_state::{pending_coinbase::PendingCoinbase, scan_state::ScanState},
    staged_ledger::staged_ledger::StagedLedger,
    Mask,
};
use mina_core::{block::ArcBlockWithHash, ChainId};
use mina_p2p_messages::{binprot::BinProtWrite, list::List, v2};
// `std::time::Instant` isn't there in the browser (I5e).
use redux::Instant;

use super::{
    backend::StoreBackend,
    diff::{LedgerDiff, LedgerWrite},
    event::{
        PersistChainPutDone, PersistCheckpointDone, PersistEvent, PersistLoadResult, PersistOpened,
        PersistRestored, PersistSnapshotInfo, PersistStoreSummary, PersistStoredLedger,
        PersistWriteError, PersistWritten,
    },
    format::{encode_manifest, parts_name, ChainEntry, Checkpoint, Manifest},
    load::{self, BlockProofVerifier, LoadOutcome, PersistLoadRequest, RestoreGuard, Restored},
    store::{Blob, CheckpointWrite, FrontierStore, OpenOutcome, StoreConfig},
    PersistError,
};
use crate::p2p::channels::rpc::StagedLedgerAuxAndPendingCoinbases;

/// What the store thread is asked to do.
pub enum PersistJob {
    /// Open (or create) the store of this chain. Before this, other jobs fail.
    Open {
        chain_id: ChainId,
        genesis_state_hash: v2::StateHash,
    },
    /// Store a new best chain, root first, with the blocks not stored yet
    /// (any already stored are skipped).
    ChainPut {
        chain: Vec<ChainEntry>,
        blocks: Vec<ArcBlockWithHash>,
    },
    Checkpoint(Box<CheckpointJob>),
    /// Load the store for a restore (I5d1), hand what it loaded to the
    /// ledger thread, and report.
    Load(Box<LoadJob>),
    /// The restored chain is committed: clear the restore attempts record.
    RestoreDone,
    /// A restore failed after the load: delete the store.
    Discard {
        reason: String,
    },
}

/// What a load hands the ledger thread (R20 D3's `PersistInsert`): built on
/// the store thread, then only the ledger thread's.
pub struct PersistInsert {
    pub snarked: Vec<(v2::LedgerHash, Mask)>,
    pub staged: Option<(v2::MinaBaseStagedLedgerHashStableV1, StagedLedger)>,
}

/// Hands a load's ledgers to the ledger thread and waits for it to take
/// them, so that they are there before any request the node makes after the
/// load's event. Returns the ledger thread's time on them (milliseconds).
pub type PersistInserter = Box<dyn FnOnce(PersistInsert) -> Result<f64, String> + Send>;

pub struct LoadJob {
    pub request: PersistLoadRequest,
    /// The node's blockchain verifier, for the stored blocks' proofs; `None`
    /// when the node runs with proof verification off.
    pub verifier: Option<BlockProofVerifier>,
    pub insert: PersistInserter,
    /// Cancellation races atomically with final handoff. The node waits for
    /// its acknowledgement before starting bootstrap.
    pub control: Arc<super::load_control::LoadControl>,
}

static_assertions::assert_impl_all!(LoadJob: Send);

/// The parts of the root's staged ledger a checkpoint stores, as the ledger
/// thread hands them over: the store thread converts them to wire types and
/// encodes them (D3), which takes a large share of a second on mainnet.
///
/// The scan state is a shallow clone: its jobs' witnesses (`SparseLedger`)
/// are shared with the ledger thread's staged ledger, which may convert or
/// copy them meanwhile. That is safe only because their lock blocks; the store
/// thread must not be given anything whose lock fails on contention (`Mask`,
/// `Database`).
pub struct PartsSnapshot {
    pub scan_state: ScanState,
    pub pending_coinbase: PendingCoinbase,
    pub staged_ledger_hash: v2::LedgerHash,
    /// The protocol states the scan state needs, in the order a node serves
    /// them.
    pub needed_blocks: List<v2::MinaStateProtocolStateValueStableV2>,
}

impl PartsSnapshot {
    /// The parts in wire types, as the store thread converts them (D3).
    pub fn into_wire(self) -> StagedLedgerAuxAndPendingCoinbases {
        StagedLedgerAuxAndPendingCoinbases {
            scan_state: (&self.scan_state).into(),
            staged_ledger_hash: self.staged_ledger_hash,
            pending_coinbase: (&self.pending_coinbase).into(),
            needed_blocks: self.needed_blocks,
        }
    }
}

/// The ledger thread's time on the pieces of streamed full ledgers.
#[derive(Debug, Default, Clone, Copy)]
pub struct StreamStats {
    pub parts: u32,
    pub ms: f64,
    pub max_ms: f64,
}

impl StreamStats {
    pub fn add(&mut self, ms: f64) {
        self.parts += 1;
        self.ms += ms;
        self.max_ms = self.max_ms.max(ms);
    }
}

pub type SharedStreamStats = Arc<Mutex<StreamStats>>;

/// A checkpoint snapshotted by the ledger thread (D3, D4).
pub struct CheckpointJob {
    pub checkpoint: Checkpoint,
    pub ledgers: Vec<(v2::LedgerHash, Option<LedgerWrite>)>,
    pub staged: LedgerDiff,
    pub parts: PartsSnapshot,
    pub chain: Vec<ChainEntry>,
    pub snapshot: PersistSnapshotInfo,
    pub stream_stats: SharedStreamStats,
    /// Released once the job is done, whatever the outcome, before the result
    /// is reported, or when a job that never ran is dropped: it releases the
    /// ledger thread's pins.
    pub on_done: Option<DoneGuard>,
}

/// Runs its function when dropped.
pub struct DoneGuard(Option<Box<dyn FnOnce() + Send>>);

impl DoneGuard {
    pub fn new(on_done: impl FnOnce() + Send + 'static) -> Self {
        Self(Some(Box::new(on_done)))
    }
}

impl Drop for DoneGuard {
    fn drop(&mut self) {
        if let Some(on_done) = self.0.take() {
            on_done();
        }
    }
}

/// A handle on the store thread. Cloned by the node service and the ledger
/// thread.
#[derive(Clone)]
pub struct PersistWriter {
    sender: mpsc::Sender<PersistJob>,
}

impl std::fmt::Debug for PersistWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PersistWriter")
    }
}

impl PersistWriter {
    /// Hands a job to the store thread; `false` if it has stopped.
    pub fn send(&self, job: PersistJob) -> bool {
        self.sender.send(job).is_ok()
    }

    /// A writer whose jobs go to `receiver`, for tests that run a
    /// [`StoreWorker`] themselves.
    pub fn channel() -> (Self, mpsc::Receiver<PersistJob>) {
        let (sender, receiver) = mpsc::channel();
        (Self { sender }, receiver)
    }

    /// Starts the store thread. `make_backend` gives the backend of a chain's
    /// store when it is opened; `emit` reports results to the node. In the
    /// browser the thread is a web worker, which may block as a native thread
    /// does (I5e): its backend waits there for the OPFS worker.
    pub fn spawn<B, F, E>(make_backend: F, options: WriterOptions, emit: E) -> Self
    where
        B: StoreBackend,
        F: Fn(&ChainId) -> io::Result<(B, String)> + Send + 'static,
        E: Fn(PersistEvent) + Send + 'static,
    {
        let (writer, receiver) = Self::channel();
        mina_core::thread::Builder::new()
            .name("frontier-store".into())
            .spawn(move || {
                let mut worker = StoreWorker::new(make_backend, options, emit);
                while let Ok(job) = receiver.recv() {
                    worker.handle(job);
                }
            })
            .expect("Failed: frontier store thread");
        writer
    }
}

#[derive(Debug, Clone, Default)]
pub struct WriterOptions {
    /// Delete the store when it is opened (`--frontier-store-reset`).
    pub reset: bool,
    /// Recorded in each manifest, for diagnostics.
    pub node_commit: String,
    /// The store's restore guard, shared with the node's signal thread.
    pub restore_guard: RestoreGuard,
}

/// I/O errors in a row, other than a full disk, after which persistence is
/// turned off for the run: a disk that fails every write (EIO, a read-only
/// remount, lost permissions) would otherwise fail every block and pay for a
/// checkpoint's snapshot and stream again at every retry.
pub const MAX_IO_ERRORS_IN_A_ROW: u32 = 5;

/// The store thread's work, one job at a time.
pub struct StoreWorker<B: StoreBackend, F, E> {
    make_backend: F,
    options: WriterOptions,
    emit: E,
    store: Option<FrontierStore<B>>,
    /// I/O errors since the last job that succeeded.
    io_errors: u32,
}

/// The wall clock, in milliseconds since the Unix epoch, on any thread of a
/// native or a web node.
pub fn now_ms() -> u64 {
    u64::from(mina_core::log::system_time()) / 1_000_000
}

fn elapsed_ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Whether the error is the disk (or the browser's quota) being full: D5
/// turns persistence off for the run rather than retrying a large write.
pub fn is_out_of_space(error: &PersistError) -> bool {
    fn full(error: &io::Error) -> bool {
        matches!(
            error.kind(),
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
        )
    }
    match error {
        PersistError::Io(error) => full(error),
        PersistError::Codec(super::ledger_codec::CodecError::Io(error)) => full(error),
        _ => false,
    }
}

/// Whether the error is an I/O error, of the store's files or of a streamed
/// ledger's.
fn is_io(error: &PersistError) -> bool {
    matches!(
        error,
        PersistError::Io(_) | PersistError::Codec(super::ledger_codec::CodecError::Io(_))
    )
}

/// Whether a streamed ledger's source is gone for good: the ledger thread has
/// stopped, so no checkpoint can be written again.
fn is_source_gone(error: &PersistError) -> bool {
    matches!(
        error,
        PersistError::Codec(super::ledger_codec::CodecError::SourceGone(_))
    )
}

fn block_blob(block: ArcBlockWithHash) -> Blob {
    Box::new(move |mut w: &mut dyn Write| block.block.binprot_write(&mut w))
}

/// Every file either manifest references.
fn referenced(manifests: [Option<&Manifest>; 2]) -> BTreeSet<String> {
    manifests
        .into_iter()
        .flatten()
        .flat_map(|manifest| manifest.files.iter().map(|file| file.name().to_owned()))
        .collect()
}

impl<B, F, E> StoreWorker<B, F, E>
where
    B: StoreBackend,
    F: Fn(&ChainId) -> io::Result<(B, String)>,
    E: Fn(PersistEvent),
{
    pub fn new(make_backend: F, options: WriterOptions, emit: E) -> Self {
        Self {
            make_backend,
            options,
            emit,
            store: None,
            io_errors: 0,
        }
    }

    /// A job's result as the node hears it: any success clears the count of
    /// I/O errors in a row; an error turns persistence off if the disk is
    /// full, the ledger thread is gone, or it is the
    /// [`MAX_IO_ERRORS_IN_A_ROW`]th I/O error in a row.
    fn reported<T>(&mut self, result: Result<T, PersistError>) -> Result<T, PersistWriteError> {
        let error = match result {
            Ok(done) => {
                self.io_errors = 0;
                return Ok(done);
            }
            Err(error) => error,
        };
        if is_io(&error) {
            self.io_errors = self.io_errors.saturating_add(1);
        }
        let disable = is_out_of_space(&error)
            || is_source_gone(&error)
            || self.io_errors >= MAX_IO_ERRORS_IN_A_ROW;
        let error = if disable && !is_out_of_space(&error) && is_io(&error) {
            format!("{error} ({} I/O errors in a row)", self.io_errors)
        } else {
            error.to_string()
        };
        Err(PersistWriteError { disable, error })
    }

    pub fn store(&self) -> Option<&FrontierStore<B>> {
        self.store.as_ref()
    }

    pub fn handle(&mut self, job: PersistJob) {
        match job {
            PersistJob::Open {
                chain_id,
                genesis_state_hash,
            } => {
                let event = match self.open(chain_id, genesis_state_hash) {
                    Ok(opened) => PersistEvent::Opened(opened),
                    Err(error) => PersistEvent::OpenFailed {
                        error: error.to_string(),
                    },
                };
                (self.emit)(event);
            }
            PersistJob::ChainPut { chain, blocks } => {
                let result = self.chain_put(chain, blocks);
                let result = self.reported(result);
                (self.emit)(PersistEvent::ChainPut(result));
            }
            PersistJob::Checkpoint(job) => {
                let mut job = *job;
                let on_done = job.on_done.take();
                let result = self.checkpoint(job);
                // The pins go before the node hears the result, so that the
                // next checkpoint's snapshot finds none left.
                drop(on_done);
                let result = self.reported(result);
                (self.emit)(PersistEvent::Checkpoint(result));
            }
            PersistJob::Load(job) => {
                let result = self.load(*job);
                // Anything but a restore to replay is over: a ledgers-only
                // load cleared its record; an error, a cancelled or an absent
                // load leaves its attempt counted.
                if let (false, Some(store)) = (
                    matches!(result, PersistLoadResult::Restored(_)),
                    self.store.as_ref(),
                ) {
                    store.restore_guard().end();
                }
                let restored = matches!(
                    result,
                    PersistLoadResult::Restored(_) | PersistLoadResult::Cancelled
                );
                if let (false, Some(store)) = (restored, self.store.as_mut()) {
                    // Best effort: without it the next checkpoint may fail
                    // once, and be retried.
                    if let Ok(summary) = summary(store) {
                        (self.emit)(PersistEvent::Summary(summary));
                    }
                }
                (self.emit)(PersistEvent::Loaded(Box::new(result)));
            }
            PersistJob::RestoreDone => {
                if let Some(store) = self.store.as_mut() {
                    // Best effort: a stale record only counts against the
                    // generation it names.
                    if let Err(error) = store.clear_attempts() {
                        let missing = matches!(
                            &error,
                            PersistError::Io(e) if e.kind() == io::ErrorKind::NotFound
                        );
                        if !missing {
                            mina_core::warn!(
                                mina_core::log::system_time();
                                kind = "PersistRestoreDone",
                                summary = "the restore attempts record wasn't cleared",
                                error = error.to_string(),
                            );
                        }
                    }
                }
            }
            PersistJob::Discard { reason } => {
                if let Some(store) = self.store.as_mut() {
                    let result = store.reset();
                    mina_core::warn!(
                        mina_core::log::system_time();
                        kind = "PersistDiscard",
                        summary = "frontier store deleted after a failed restore",
                        reason = reason,
                        deleted = result.as_ref().map_or(0, Vec::len),
                        error = result.err().map(|error| error.to_string()),
                    );
                }
            }
        }
    }

    /// Loads the store for a restore (R20 D5 step 3) and hands what it loaded
    /// to the ledger thread, unless the node has stopped waiting.
    fn load(&mut self, job: LoadJob) -> PersistLoadResult {
        let LoadJob {
            request,
            verifier,
            insert,
            control,
        } = job;
        let started = Instant::now();
        let Some(store) = self.store.as_mut() else {
            return PersistLoadResult::Error {
                error: "the store is not open".into(),
            };
        };
        let outcome = load::load_with_progress(store, &request, verifier.as_ref(), &control);
        if control.is_cancelled() {
            // The joins have returned and every task and intermediate is
            // dropped before the acknowledgement releases bootstrap.
            drop(outcome);
            mina_core::warn!(
                mina_core::log::system_time();
                kind = "PersistLoad",
                summary = "frontier store load cancelled: worker drained, ledgers dropped",
                elapsed_ms = elapsed_ms(started),
            );
            return PersistLoadResult::Cancelled;
        }
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                let result = PersistLoadResult::Error {
                    error: error.to_string(),
                };
                log_load(&result, elapsed_ms(started));
                return result;
            }
        };
        // One atomic winner: cancellation drops all results, or handoff
        // owns them and the node must await its event. No late insertion.
        if !control.begin_handoff() {
            drop(outcome);
            return PersistLoadResult::Cancelled;
        }
        let result = match outcome {
            LoadOutcome::Absent { reason } => PersistLoadResult::Absent { reason },
            LoadOutcome::Discarded { reason } => PersistLoadResult::Discarded { reason },
            LoadOutcome::LedgersOnly {
                reason,
                generation,
                ledgers,
                mut timings,
            } => {
                let hashes = ledgers.keys().cloned().collect();
                let inserting = Instant::now();
                match insert(PersistInsert {
                    snarked: ledgers.into_iter().collect(),
                    staged: None,
                }) {
                    Ok(ledger_thread_ms) => {
                        timings.insert_ms = elapsed_ms(inserting);
                        timings.ledger_thread_ms = ledger_thread_ms;
                        timings.total_ms = elapsed_ms(started);
                        // The decoding is behind us: what follows is a
                        // bootstrap, which this record isn't about.
                        let _ = store.clear_attempts();
                        PersistLoadResult::LedgersOnly {
                            reason,
                            generation,
                            ledgers: hashes,
                            timings,
                        }
                    }
                    Err(error) => PersistLoadResult::Error {
                        error: format!("handing the ledgers to the ledger thread: {error}"),
                    },
                }
            }
            LoadOutcome::Restored(restored) => {
                let Restored {
                    generation,
                    attempt,
                    ledgers,
                    staged_ledger_hash,
                    staged,
                    root,
                    blocks,
                    needed_protocol_states,
                    mut timings,
                } = *restored;
                let hashes = ledgers.keys().cloned().collect();
                let inserting = Instant::now();
                match insert(PersistInsert {
                    snarked: ledgers.into_iter().collect(),
                    staged: Some((staged_ledger_hash, staged)),
                }) {
                    Ok(ledger_thread_ms) => {
                        timings.insert_ms = elapsed_ms(inserting);
                        timings.ledger_thread_ms = ledger_thread_ms;
                        timings.total_ms = elapsed_ms(started);
                        PersistLoadResult::Restored(PersistRestored {
                            generation,
                            attempt,
                            root,
                            blocks,
                            needed_protocol_states,
                            ledgers: hashes,
                            timings,
                        })
                    }
                    Err(error) => PersistLoadResult::Error {
                        error: format!("handing the ledgers to the ledger thread: {error}"),
                    },
                }
            }
        };
        log_load(&result, elapsed_ms(started));
        result
    }

    fn open(
        &mut self,
        chain_id: ChainId,
        genesis_state_hash: v2::StateHash,
    ) -> Result<PersistOpened, PersistError> {
        let started = Instant::now();
        self.store = None;
        let (backend, dir) = (self.make_backend)(&chain_id)?;
        let config = StoreConfig {
            chain_id,
            genesis_state_hash,
            node_commit: self.options.node_commit.clone(),
            now_ms: Box::new(now_ms),
        };
        let (mut store, report) = FrontierStore::open(backend, config)?;
        store.set_restore_guard(self.options.restore_guard.clone());
        let mut deleted = report.deleted.len() as u32;
        let outcome = if self.options.reset {
            deleted += store.reset()?.len() as u32;
            "reset: requested".to_owned()
        } else {
            match report.outcome {
                OpenOutcome::Empty => "empty".to_owned(),
                OpenOutcome::Opened { generation } => format!("opened generation {generation}"),
                OpenOutcome::Reset { reason } => format!("reset: {reason}"),
            }
        };
        let summary = summary(&mut store)?;
        mina_core::info!(
            mina_core::log::system_time();
            kind = "PersistOpened",
            summary = format!("frontier store {outcome}"),
            dir = dir.clone(),
            slots = format!("{:?}", report.slots),
            deleted = deleted,
            generation = summary.generation,
            checkpoint_root = summary.checkpoint_root.as_ref().map(ToString::to_string),
            ledgers = summary.ledgers.len(),
            files = summary.files,
            bytes = summary.bytes,
        );
        self.store = Some(store);
        Ok(PersistOpened {
            dir,
            outcome,
            deleted,
            store: summary,
            elapsed_ms: elapsed_ms(started),
        })
    }

    fn store_mut(&mut self) -> Result<&mut FrontierStore<B>, PersistError> {
        self.store
            .as_mut()
            .ok_or_else(|| PersistError::Invalid("the store is not open".into()))
    }

    fn chain_put(
        &mut self,
        chain: Vec<ChainEntry>,
        blocks: Vec<ArcBlockWithHash>,
    ) -> Result<PersistChainPutDone, PersistError> {
        let started = Instant::now();
        let tip = chain
            .last()
            .ok_or_else(|| PersistError::Invalid("an empty chain".into()))?
            .hash
            .clone();
        let store = self.store_mut()?;
        let before = referenced([store.manifest(), store.previous_manifest()]);
        let blocks = blocks
            .into_iter()
            .map(|block| (block.hash().clone(), block_blob(block)))
            .collect();
        let result = store.chain_put(chain, blocks)?;
        let written = written(store, &before);
        let summary = summary(store)?;
        let done = PersistChainPutDone {
            tip,
            generation: result.generation,
            checkpoint_invalidated: result.checkpoint_invalidated,
            written,
            store: summary,
            elapsed_ms: elapsed_ms(started),
        };
        mina_core::debug!(
            mina_core::log::system_time();
            kind = "PersistChainPut",
            summary = format!("best chain stored, tip {}", done.tip),
            generation = done.generation,
            checkpoint_invalidated = done.checkpoint_invalidated,
            files_written = done.written.files,
            bytes_written = done.written.bytes,
            manifest_bytes = done.written.manifest_bytes,
            chain_len = done.store.chain_len,
            store_bytes = done.store.bytes,
            elapsed_ms = done.elapsed_ms,
        );
        Ok(done)
    }

    fn checkpoint(&mut self, job: CheckpointJob) -> Result<PersistCheckpointDone, PersistError> {
        let started = Instant::now();
        let CheckpointJob {
            checkpoint,
            ledgers,
            staged,
            parts,
            chain,
            snapshot,
            stream_stats,
            on_done: _,
        } = job;
        let root = checkpoint.root.clone();
        let root_height = checkpoint.root_height;
        let store = self
            .store
            .as_mut()
            .ok_or_else(|| PersistError::Invalid("the store is not open".into()))?;

        // Wire types first, on this thread (D3); encoded as the file is
        // written, so the encoding is never in memory whole.
        let convert_started = Instant::now();
        let parts = parts.into_wire();
        let convert_ms = elapsed_ms(convert_started);
        let encode_ms = Arc::new(Mutex::new(0.0));
        let parts_blob: Blob = {
            let encode_ms = encode_ms.clone();
            Box::new(move |mut w: &mut dyn Write| {
                let started = Instant::now();
                let result = parts.binprot_write(&mut w);
                *encode_ms.lock().unwrap() = elapsed_ms(started);
                result
            })
        };

        let before = referenced([store.manifest(), store.previous_manifest()]);
        let write = CheckpointWrite {
            checkpoint,
            ledgers,
            staged,
            parts: parts_blob,
            chain,
            // The chain puts sent before this checkpoint stored them.
            blocks: Vec::new(),
        };
        let generation = store.checkpoint(write)?;
        let parts_bytes = store
            .manifest()
            .and_then(|manifest| manifest.file(&parts_name(&root)))
            .map_or(0, |file| file.len);
        let written = written(store, &before);
        let summary = summary(store)?;
        let stream = *stream_stats.lock().unwrap();
        let encode_ms = *encode_ms.lock().unwrap();
        let done = PersistCheckpointDone {
            root,
            root_height,
            generation,
            written,
            store: summary,
            parts_bytes,
            parts_encode_ms: convert_ms + encode_ms,
            stream_parts: stream.parts,
            stream_ms: stream.ms,
            stream_max_ms: stream.max_ms,
            elapsed_ms: elapsed_ms(started),
            snapshot,
        };
        mina_core::info!(
            mina_core::log::system_time();
            kind = "PersistCheckpoint",
            summary = format!("checkpoint written, root {} at {}", done.root, done.root_height),
            generation = done.generation,
            files_written = done.written.files,
            bytes_written = done.written.bytes,
            manifest_bytes = done.written.manifest_bytes,
            files_deleted = done.written.deleted,
            parts_bytes = done.parts_bytes,
            parts_encode_ms = done.parts_encode_ms,
            ledgers = format!("{:?}", done.snapshot.ledgers.iter().map(|l| (l.hash.to_string(), &l.kind)).collect::<Vec<_>>()),
            staged_accounts = done.snapshot.staged_accounts,
            ledger_thread_snapshot_ms = done.snapshot.total_ms,
            ledger_thread_staged_diff_ms = done.snapshot.staged_diff_ms,
            ledger_thread_ledger_diffs_ms = done.snapshot.ledger_diffs_ms,
            ledger_thread_stream_parts = done.stream_parts,
            ledger_thread_stream_ms = done.stream_ms,
            ledger_thread_stream_max_ms = done.stream_max_ms,
            store_files = done.store.files,
            store_bytes = done.store.bytes,
            elapsed_ms = done.elapsed_ms,
        );
        Ok(done)
    }
}

/// What the last write added: the files the newest manifest references that
/// no manifest referenced before, and the files gone since.
fn written<B: StoreBackend>(store: &FrontierStore<B>, before: &BTreeSet<String>) -> PersistWritten {
    let Some(manifest) = store.manifest() else {
        return PersistWritten::default();
    };
    let mut written = PersistWritten {
        manifest_bytes: encode_manifest(manifest).len() as u64,
        ..Default::default()
    };
    for file in &manifest.files {
        if !before.contains(file.name()) {
            written.files += 1;
            written.bytes += file.len;
        }
    }
    let after = referenced([store.manifest(), store.previous_manifest()]);
    written.deleted = before.difference(&after).count() as u32;
    written
}

fn summary<B: StoreBackend>(
    store: &mut FrontierStore<B>,
) -> Result<PersistStoreSummary, PersistError> {
    let Some(manifest) = store.manifest().cloned() else {
        return Ok(PersistStoreSummary::default());
    };
    let ledgers = store
        .stored_ledgers()?
        .into_iter()
        .map(|(hash, base)| PersistStoredLedger { hash, base })
        .collect();
    Ok(PersistStoreSummary {
        generation: manifest.generation,
        checkpoint_root: manifest.checkpoint.as_ref().map(|c| c.root.clone()),
        chain_len: manifest.chain.len() as u32,
        tip: manifest.chain.last().map(|entry| entry.hash.clone()),
        ledgers,
        files: manifest.files.len() as u32,
        bytes: manifest.files.iter().map(|file| file.len).sum(),
    })
}

/// One log line per load, saying what the node does next and why.
fn log_load(result: &PersistLoadResult, elapsed_ms: f64) {
    match result {
        PersistLoadResult::Restored(restored) => mina_core::info!(
            mina_core::log::system_time();
            kind = "PersistLoad",
            summary = format!(
                "frontier store loaded: root {} at {}, {} blocks to replay to {} at {}",
                restored.root.hash(),
                restored.root.height(),
                restored.blocks.len(),
                restored.tip().hash(),
                restored.tip().height(),
            ),
            generation = restored.generation,
            attempt = restored.attempt,
            ledgers = restored.ledgers.len(),
            protocol_states = restored.needed_protocol_states.len(),
            blocks_ms = restored.timings.blocks_ms,
            files_ms = restored.timings.files_ms,
            checks_ms = restored.timings.checks_ms,
            block_checks_ms = restored.timings.block_checks_ms,
            proofs_ms = restored.timings.proofs_ms,
            proofs_checked = restored.timings.proofs_checked,
            ledgers_ms = restored.timings.ledgers_ms,
            ledger_verify_ms = restored.timings.ledger_verify_ms,
            ledger_rebuild_ms = restored.timings.ledger_rebuild_ms,
            staged_ms = restored.timings.staged_ms,
            staged_verify_ms = restored.timings.staged_verify_ms,
            indexes_ms = restored.timings.indexes_ms,
            accounts_checked = restored.timings.accounts_checked,
            parts_ms = restored.timings.parts_ms,
            held_accounts = restored.timings.held_accounts,
            hold_ms = restored.timings.hold_ms,
            hash_ms = restored.timings.hash_ms,
            insert_ms = restored.timings.insert_ms,
            ledger_thread_ms = restored.timings.ledger_thread_ms,
            elapsed_ms = elapsed_ms,
        ),
        PersistLoadResult::LedgersOnly {
            reason,
            generation,
            ledgers,
            timings,
        } => mina_core::warn!(
            mina_core::log::system_time();
            kind = "PersistLoad",
            summary = format!(
                "frontier store not restored, the node bootstraps with its {} ledgers as copy origins: {reason}",
                ledgers.len()
            ),
            generation = *generation,
            files_ms = timings.files_ms,
            ledgers_ms = timings.ledgers_ms,
            ledger_verify_ms = timings.ledger_verify_ms,
            ledger_rebuild_ms = timings.ledger_rebuild_ms,
            indexes_ms = timings.indexes_ms,
            accounts_checked = timings.accounts_checked,
            ledger_thread_ms = timings.ledger_thread_ms,
            elapsed_ms = elapsed_ms,
        ),
        PersistLoadResult::Absent { reason } => mina_core::info!(
            mina_core::log::system_time();
            kind = "PersistLoad",
            summary = format!("nothing to restore, the node bootstraps: {reason}"),
            elapsed_ms = elapsed_ms,
        ),
        PersistLoadResult::Discarded { reason } => mina_core::warn!(
            mina_core::log::system_time();
            kind = "PersistLoad",
            summary = format!("frontier store deleted, the node bootstraps: {reason}"),
            elapsed_ms = elapsed_ms,
        ),
        PersistLoadResult::Error { error } => mina_core::warn!(
            mina_core::log::system_time();
            kind = "PersistLoad",
            summary = format!("frontier store not loaded, the node bootstraps: {error}"),
            elapsed_ms = elapsed_ms,
        ),
        PersistLoadResult::Cancelled => {}
    }
}
