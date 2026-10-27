//! Delivers the archive outbox: the oldest pending entry first, then retains
//! acknowledged diffs for bounded replay. Failures retry
//! with back-off for as long as it takes.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, OnceLock,
    },
    time::Duration,
};

use serde::Serialize;

use super::outbox::{AppendOutcome, BlockRef, Outbox, OutboxStats};

/// Where entries go. `deliver` returns once the receiver acknowledged the
/// payload; an error leaves the entry first in line.
pub trait ArchiveSink: Send {
    fn deliver(&mut self, payload: &[u8]) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(60),
        }
    }
}

impl Backoff {
    /// The wait after `failures` consecutive failures (at least one).
    pub fn delay(&self, failures: u32) -> Duration {
        let factor = 1u32 << failures.saturating_sub(1).min(16);
        self.initial.saturating_mul(factor).min(self.max)
    }
}

/// Delivery state the status reports, besides the outbox's own numbers.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DeliveryState {
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
    pub last_attempt_at_ms: Option<u64>,
    pub last_success_at_ms: Option<u64>,
    /// The entry being retried, when delivery is failing.
    pub stuck_on: Option<BlockRef>,
}

/// `GET /archive/status`.
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveStatus {
    pub outbox_dir: String,
    pub archive_address: Option<String>,
    #[serde(flatten)]
    pub outbox: OutboxStats,
    #[serde(flatten)]
    pub delivery: DeliveryState,
    /// What the gaps mean for whoever runs the archive.
    pub note: &'static str,
}

const STATUS_NOTE: &str = "RPC acknowledgements precede database commits; delivered and \
     last_delivered mean acknowledged, not verified stored. Pending diffs retry \
     indefinitely unless capacity eviction or corruption removes them. Acknowledged \
     diffs are retained in delivered/ while less than retain_blocks below \
     highest_accepted_height, subject to the shared max_entries/max_bytes bounds \
     for diff files (journal/quarantine excluded; an oversized incoming diff or \
     protected in-flight entry can exceed tiny limits). Startup and POST /archive/resend requeue all retained diffs, even outside the \
     frontier. Expiry removes this repair opportunity without verifying storage; \
     no database reconciliation is performed, so gaps_total=0 does not prove archive \
     completeness. Gaps record unavailable or potentially missing history; interrupted \
     operations can leave both a diff and its gap (duplicate records are possible). \
     Corrupt gaps at height 0 have an unparseable filename. Audit with \
     mina-missing-blocks-auditor; missing blocks outside the retained/pending window, \
     evicted/corrupt diffs, history before feed_started_at, bootstrap roots and \
     unapplied forks need precomputed-block backfill via mina-archive-blocks --precomputed.";

struct Inner {
    outbox: Outbox,
    delivery: DeliveryState,
}

/// The outbox shared by the intake (the archive service thread) and the
/// delivery thread.
#[derive(Clone)]
pub struct SharedOutbox {
    inner: Arc<(Mutex<Inner>, Condvar)>,
    stop: Arc<AtomicBool>,
    archive_address: Option<String>,
}

static STATUS: OnceLock<SharedOutbox> = OnceLock::new();

/// The status of this process's archive outbox, if it has one.
pub fn status() -> Option<ArchiveStatus> {
    STATUS.get().map(SharedOutbox::status)
}

/// Operator-triggered replay of this process's retained acknowledgements.
pub fn resend_retained() -> Option<std::io::Result<usize>> {
    STATUS.get().map(SharedOutbox::resend_retained)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl SharedOutbox {
    pub fn new(outbox: Outbox, archive_address: Option<String>) -> Self {
        Self {
            inner: Arc::new((
                Mutex::new(Inner {
                    outbox,
                    delivery: DeliveryState::default(),
                }),
                Condvar::new(),
            )),
            stop: Arc::new(AtomicBool::new(false)),
            archive_address,
        }
    }

    /// Makes this outbox the one `GET /archive/status` reports. The first
    /// call wins.
    pub fn register_global(&self) {
        let _ = STATUS.set(self.clone());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves consistent on-disk state; the
        // in-memory index is rebuilt on restart.
        self.inner.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn append(
        &self,
        block: BlockRef,
        parent_hash: &str,
        payload: &[u8],
    ) -> std::io::Result<AppendOutcome> {
        let outcome = self.lock().outbox.append(block, parent_hash, payload)?;
        self.inner.1.notify_all();
        Ok(outcome)
    }

    pub fn resend_retained(&self) -> std::io::Result<usize> {
        let result = self.lock().outbox.resend_retained();
        self.inner.1.notify_all();
        result
    }

    pub fn status(&self) -> ArchiveStatus {
        let inner = self.lock();
        ArchiveStatus {
            outbox_dir: inner.outbox.dir().display().to_string(),
            archive_address: self.archive_address.clone(),
            outbox: inner.outbox.stats().clone(),
            delivery: inner.delivery.clone(),
            note: STATUS_NOTE,
        }
    }

    /// Ends [`Self::run_delivery`] after the attempt in progress. Entries
    /// stay on disk for the next start.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.inner.1.notify_all();
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Sleeps `delay`, waking early if stopped.
    fn pause(&self, delay: Duration) {
        let inner = self.lock();
        if !self.stopped() {
            let _unused = self
                .inner
                .1
                .wait_timeout_while(inner, delay, |_| !self.stopped())
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    pub fn pending_len(&self) -> usize {
        self.lock().outbox.len()
    }

    /// Waits up to `timeout` for the outbox to have no pending entries.
    pub fn wait_empty(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut inner = self.lock();
        while !inner.outbox.is_empty() {
            let now = std::time::Instant::now();
            if now >= deadline {
                return false;
            }
            inner = self
                .inner
                .1
                .wait_timeout(inner, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        true
    }

    /// One delivery attempt of the oldest entry. `Ok(None)` when the outbox
    /// is empty, `Ok(Some(Ok(())))` when an entry was delivered.
    pub fn deliver_one(
        &self,
        sink: &mut dyn ArchiveSink,
    ) -> std::io::Result<Option<Result<(), String>>> {
        let Some(entry) = self.lock().outbox.next_for_delivery()? else {
            return Ok(None);
        };
        // Sent without the lock: intake must never wait for the archive.
        let result = sink.deliver(&entry.payload);
        let mut inner = self.lock();
        inner.delivery.last_attempt_at_ms = Some(now_ms());
        match &result {
            Ok(()) => {
                inner.outbox.confirm(entry.meta.seq)?;
                inner.delivery.consecutive_failures = 0;
                inner.delivery.last_error = None;
                inner.delivery.stuck_on = None;
                inner.delivery.last_success_at_ms = Some(now_ms());
            }
            Err(e) => {
                inner.outbox.release();
                inner.delivery.consecutive_failures += 1;
                inner.delivery.last_error = Some(e.clone());
                inner.delivery.stuck_on = Some(entry.meta.block.clone());
            }
        }
        drop(inner);
        self.inner.1.notify_all();
        Ok(Some(result))
    }

    /// Delivers until stopped: sends the oldest entry, waits for new ones
    /// when empty, backs off after failures. Never drops an entry for
    /// failing.
    pub fn run_delivery(&self, sink: &mut dyn ArchiveSink, backoff: Backoff) {
        while !self.stopped() {
            match self.deliver_one(sink) {
                Ok(None) => {
                    let inner = self.lock();
                    if inner.outbox.is_empty() && !self.stopped() {
                        let _unused = self
                            .inner
                            .1
                            .wait_timeout_while(inner, Duration::from_secs(5), |inner| {
                                inner.outbox.is_empty() && !self.stopped()
                            })
                            .unwrap_or_else(|e| e.into_inner());
                    }
                }
                Ok(Some(Ok(()))) => {
                    let inner = self.lock();
                    if let Some(block) = &inner.outbox.stats().last_delivered {
                        mina_node::core::info!(
                            mina_node::core::log::system_time();
                            summary = "archive diff delivered",
                            height = block.height,
                            state_hash = block.state_hash.clone(),
                            pending = inner.outbox.len(),
                        );
                    }
                }
                Ok(Some(Err(error))) => {
                    let (failures, pending, stuck) = {
                        let inner = self.lock();
                        (
                            inner.delivery.consecutive_failures,
                            inner.outbox.len(),
                            inner.delivery.stuck_on.clone(),
                        )
                    };
                    let delay = backoff.delay(failures);
                    mina_node::core::warn!(
                        mina_node::core::log::system_time();
                        summary = "archive delivery failed, will retry",
                        error = error,
                        attempt = failures,
                        pending = pending,
                        height = stuck.as_ref().map_or(0, |b| b.height),
                        state_hash = stuck.map(|b| b.state_hash).unwrap_or_default(),
                        retry_in_ms = delay.as_millis() as u64,
                    );
                    self.pause(delay);
                }
                Err(e) => {
                    let delay = backoff.max;
                    mina_node::core::error!(
                        mina_node::core::log::system_time();
                        summary = "archive outbox I/O error, will retry",
                        error = e.to_string(),
                        retry_in_ms = delay.as_millis() as u64,
                    );
                    self.pause(delay);
                }
            }
        }
    }

    /// Starts [`Self::run_delivery`] on its own thread.
    pub fn spawn_delivery(
        &self,
        mut sink: Box<dyn ArchiveSink>,
        backoff: Backoff,
    ) -> std::thread::JoinHandle<()> {
        let this = self.clone();
        std::thread::Builder::new()
            .name("mina_archive_delivery".to_owned())
            .spawn(move || this.run_delivery(sink.as_mut(), backoff))
            .expect("spawn archive delivery thread")
    }
}
