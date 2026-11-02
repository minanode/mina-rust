//! Bounded WebRTC work, including work already dequeued by a sender.
//!
//! A reservation follows a message until processing finishes. Cloning an event
//! shares its reservation; serializing it does not serialize process resources.
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use mina_core::channels::{mpsc, Aborted, Aborter};
use serde::{Deserialize, Serialize};

// The browser main thread cannot park in Atomics.wait. Every protected section
// here is synchronous and contains no callback or await; workers can therefore
// finish it while the main thread retries without using a blocking wait.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    #[cfg(not(target_arch = "wasm32"))]
    return mutex.lock().unwrap();
    #[cfg(target_arch = "wasm32")]
    loop {
        match mutex.try_lock() {
            Ok(guard) => return guard,
            Err(std::sync::TryLockError::WouldBlock) => std::hint::spin_loop(),
            Err(std::sync::TryLockError::Poisoned(error)) => panic!("{error}"),
        }
    }
}

const GLOBAL_BYTES: usize = 512 * 1024 * 1024;
static BYTES: AtomicUsize = AtomicUsize::new(0);
static BYTES_PEAK: AtomicUsize = AtomicUsize::new(0);
static SIGNAL_REJECTIONS: AtomicUsize = AtomicUsize::new(0);
static DROPS: AtomicUsize = AtomicUsize::new(0);
static TEARDOWNS: AtomicUsize = AtomicUsize::new(0);
static PEAKS: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];

#[derive(Clone, Copy)]
pub enum Kind {
    Admission,
    Command,
    Internal,
    Event,
    Ready,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Metrics {
    pub signaling_response_rejections: usize,
    pub drops: usize,
    pub teardowns: usize,
    pub bytes: usize,
    pub peak_bytes: usize,
    pub peak_items: [usize; 5],
}

pub fn record_backend_overflow() {
    DROPS.fetch_add(1, Ordering::Relaxed);
    TEARDOWNS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_signaling_response_rejection() {
    SIGNAL_REJECTIONS.fetch_add(1, Ordering::Relaxed);
}

pub fn metrics() -> Metrics {
    Metrics {
        signaling_response_rejections: SIGNAL_REJECTIONS.load(Ordering::Relaxed),
        drops: DROPS.load(Ordering::Relaxed),
        teardowns: TEARDOWNS.load(Ordering::Relaxed),
        bytes: BYTES.load(Ordering::Relaxed),
        peak_bytes: BYTES_PEAK.load(Ordering::Relaxed),
        peak_items: std::array::from_fn(|i| PEAKS[i].load(Ordering::Relaxed)),
    }
}

/// Native transport counters, with zeroes on backends that do not use rustrtc.
#[derive(Default)]
pub struct RawPacketMetrics {
    pub bytes: usize,
    pub peak_bytes: usize,
    pub drops: [usize; 3],
    pub peak_items: [usize; 3],
    pub peak_queue_bytes: [usize; 3],
    pub teardowns: usize,
    pub global_sheds: usize,
}

pub fn raw_packet_metrics() -> RawPacketMetrics {
    #[cfg(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-rustrtc-lib"))]
    {
        let m = rustrtc::transports::packet_queue::metrics();
        return RawPacketMetrics {
            bytes: m.bytes,
            peak_bytes: m.peak_bytes,
            drops: m.drops,
            peak_items: m.peak_items,
            peak_queue_bytes: m.peak_queue_bytes,
            teardowns: m.teardowns,
            global_sheds: m.global_sheds,
        };
    }
    #[cfg(not(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-rustrtc-lib")))]
    RawPacketMetrics::default()
}

/// A synchronous callback can cancel its owner even when every queue is full.
pub struct Failure {
    aborter: Mutex<Option<Aborter>>,
    aborted: Aborted,
    failed: AtomicBool,
}

impl Default for Failure {
    fn default() -> Self {
        let aborter = Aborter::default();
        let aborted = aborter.aborted();
        Self {
            aborter: Mutex::new(Some(aborter)),
            aborted,
            failed: AtomicBool::new(false),
        }
    }
}

impl Failure {
    pub fn fail(&self) {
        DROPS.fetch_add(1, Ordering::Relaxed);
        if !self.failed.swap(true, Ordering::AcqRel) {
            TEARDOWNS.fetch_add(1, Ordering::Relaxed);
            lock(&self.aborter).take();
        }
    }

    pub fn cancel(&self) {
        lock(&self.aborter).take();
    }

    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    pub async fn wait(&self) {
        self.aborted.wait().await;
    }
}

pub struct Budget {
    kind: Kind,
    max_items: usize,
    max_bytes: usize,
    used: Mutex<(usize, usize)>,
}

impl Budget {
    pub fn new(kind: Kind, max_items: usize, max_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            kind,
            max_items,
            max_bytes,
            used: Mutex::new((0, 0)),
        })
    }

    pub fn reserve(self: &Arc<Self>, bytes: usize) -> Option<Arc<Reservation>> {
        // Include a fixed allowance for queue nodes and decoded small messages.
        let bytes = bytes.checked_add(256)?;
        let mut used = lock(&self.used);
        if used.0 >= self.max_items || bytes > self.max_bytes.saturating_sub(used.1) {
            return None;
        }
        let before = BYTES
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |n| {
                n.checked_add(bytes).filter(|n| *n <= GLOBAL_BYTES)
            })
            .ok()?;
        used.0 += 1;
        used.1 += bytes;
        PEAKS[self.kind as usize].fetch_max(used.0, Ordering::Relaxed);
        BYTES_PEAK.fetch_max(before + bytes, Ordering::Relaxed);
        Some(Arc::new(Reservation {
            budget: self.clone(),
            bytes,
        }))
    }

    pub fn len(&self) -> usize {
        lock(&self.used).0
    }
}

pub struct Reservation {
    budget: Arc<Budget>,
    bytes: usize,
}
impl std::fmt::Debug for Reservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebrtcReservation")
            .field("bytes", &self.bytes)
            .finish()
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut used = lock(&self.budget.used);
        used.0 -= 1;
        used.1 -= self.bytes;
        BYTES.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

pub trait Weight {
    fn weight(&self) -> usize;
}
pub struct Entry<T>(pub T, pub Arc<Reservation>);
pub struct Receiver<T> {
    inner: mpsc::Receiver<Entry<T>>,
    gate: Arc<Mutex<bool>>,
}
impl<T> Receiver<T> {
    pub async fn recv(&mut self) -> Option<Entry<T>> {
        self.inner.recv().await
    }
    pub fn try_recv(&mut self) -> Result<Entry<T>, mpsc::TryRecvError> {
        self.inner.try_recv()
    }
}
impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        // Flume retains queued values while senders survive. Close admission
        // and drain here so cancellation also releases queued payloads.
        let mut closed = lock(&self.gate);
        *closed = true;
        while self.inner.try_recv().is_ok() {}
    }
}
pub struct Sender<T> {
    sender: mpsc::Sender<Entry<T>>,
    budget: Arc<Budget>,
    failure: Arc<Failure>,
    admission: bool,
    gate: Arc<Mutex<bool>>,
}
impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            budget: self.budget.clone(),
            failure: self.failure.clone(),
            admission: self.admission,
            gate: self.gate.clone(),
        }
    }
}
#[cfg(all(test, not(target_arch = "wasm32")))]
thread_local! {
    static LAST_GATE_NS: std::cell::Cell<u128> = const { std::cell::Cell::new(0) };
}
#[cfg(all(test, not(target_arch = "wasm32")))]
struct GateTimer(std::time::Instant);
#[cfg(all(test, not(target_arch = "wasm32")))]
impl Drop for GateTimer {
    fn drop(&mut self) {
        LAST_GATE_NS.set(self.0.elapsed().as_nanos());
    }
}
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn last_gate_ns() -> u128 {
    LAST_GATE_NS.get()
}

impl<T: Weight> Sender<T> {
    pub fn tracked_send(&self, value: T) -> Result<(), ()> {
        // Encoding is pure and can be expensive for a maximum RPC/staged-ledger
        // response. Only reservation and publication need admission serialization.
        let weight = value.weight();
        let closed = lock(&self.gate);
        #[cfg(all(test, not(target_arch = "wasm32")))]
        let _timer = GateTimer(std::time::Instant::now());
        if *closed {
            return Err(());
        }
        if !self.admission && self.failure.is_failed() {
            return Err(());
        }
        let Some(reservation) = self.budget.reserve(weight) else {
            self.overflow();
            return Err(());
        };
        self.sender
            .try_send(Entry(value, reservation))
            .map_err(|_| {
                self.overflow();
            })
    }
    fn overflow(&self) {
        // Admission is shared by all peers: its caller owns the rejected
        // peer's Failure and records both the drop and teardown there.
        if !self.admission {
            self.failure.fail();
        }
    }
    pub fn len(&self) -> usize {
        self.budget.len()
    }
}
pub fn channel<T>(
    kind: Kind,
    items: usize,
    bytes: usize,
    failure: Arc<Failure>,
) -> (Sender<T>, Receiver<T>) {
    let (sender, receiver) = mpsc::channel(items);
    let gate = Arc::new(Mutex::new(false));
    (
        Sender {
            sender,
            budget: Budget::new(kind, items, bytes),
            failure,
            admission: matches!(kind, Kind::Admission),
            gate: gate.clone(),
        },
        Receiver {
            inner: receiver,
            gate,
        },
    )
}

/// Count encoded bytes without allocating a second copy of a decoded message.
#[derive(Default)]
pub struct Counter(pub usize);
impl std::io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    impl Weight for u8 {
        fn weight(&self) -> usize {
            1
        }
    }

    #[test]
    fn encoding_never_holds_admission_gate() {
        struct InspectWeight(Arc<Mutex<bool>>);
        impl Weight for InspectWeight {
            fn weight(&self) -> usize {
                assert!(
                    self.0.try_lock().is_ok(),
                    "size traversal held admission gate"
                );
                1
            }
        }
        let (tx, rx) = channel(Kind::Command, 1, 1024, Arc::default());
        tx.tracked_send(InspectWeight(tx.gate.clone())).unwrap();
        drop(rx);
        assert!(tx.tracked_send(InspectWeight(tx.gate.clone())).is_err());
        assert_eq!(tx.len(), 0);
    }

    #[test]
    fn dequeued_and_cloned_work_keeps_its_reservation() {
        let failure = Arc::new(Failure::default());
        let (tx, mut rx) = channel(Kind::Command, 1, 1024, failure.clone());
        tx.tracked_send(1u8).unwrap();
        let entry = rx.try_recv().unwrap();
        let copy = entry.1.clone();
        drop(entry);
        assert_eq!(tx.len(), 1);
        assert!(tx.tracked_send(2).is_err());
        assert!(failure.is_failed());
        drop(copy);
        assert_eq!(tx.len(), 0);
    }

    #[test]
    fn byte_budget_and_receiver_drop_release_memory() {
        let budget = Budget::new(Kind::Event, 10, 1024);
        let a = budget.reserve(700).unwrap();
        assert!(budget.reserve(1).is_none());
        drop(a);
        assert!(budget.reserve(768).is_some());
        let (tx, rx) = channel(Kind::Command, 4, 2048, Arc::default());
        tx.tracked_send(1u8).unwrap();
        drop(rx);
        assert_eq!(tx.len(), 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn admission_rejection_counts_once() {
        // Other queue tests intentionally overflow in parallel. Run this
        // assertion in its own test process so the global deltas are exact.
        const CHILD: &str = "MINA_WEBRTC_ADMISSION_METRIC_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let test = format!(
                "{}::admission_rejection_counts_once",
                module_path!().split_once("::").unwrap().1
            );
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &test, "--nocapture"])
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        let shared = Arc::new(Failure::default());
        let rejected_peer = Failure::default();
        let (tx, mut rx) = channel(Kind::Admission, 1, 1024, shared.clone());
        tx.tracked_send(1u8).unwrap();
        let before = metrics();
        assert!(tx.tracked_send(2).is_err());
        // The incoming/outgoing admission caller tears down this peer only.
        rejected_peer.fail();
        let after = metrics();
        assert_eq!(after.drops - before.drops, 1);
        assert_eq!(after.teardowns - before.teardowns, 1);
        assert!(rejected_peer.is_failed());
        assert!(!shared.is_failed());
        drop(rx.try_recv().unwrap());
        tx.tracked_send(3).unwrap();
        drop(rx);
        assert_eq!(tx.len(), 0);
    }
}
