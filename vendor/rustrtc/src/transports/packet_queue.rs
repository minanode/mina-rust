//! Bounded raw dispatch, shared by the std and embedded transports.
//!
//! Local overflow fails its owner. Aggregate pressure sheds the largest queued
//! association and reclaims its packets before admitting another association.
//! No live association loses packets silently. Processing packets and transport
//! state are outside this queued-payload accounting.
use crate::platform::sync::Mutex;
use alloc::{
    collections::VecDeque,
    sync::{Arc, Weak},
    vec::Vec,
};
use bytes::Bytes;
use core::{
    future::poll_fn,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::{Poll, Waker},
};

pub const MAX_ITEMS: usize = 4096;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PACKET: usize = 65535;
pub const GLOBAL_BYTES: usize = 64 * 1024 * 1024;
/// The three dispatch stages cannot let one association fill the shared pool.
pub const ASSOCIATION_BYTES: usize = 3 * MAX_BYTES;
const OVERHEAD: usize = 64;

#[derive(Clone, Copy)]
pub enum Kind {
    DtlsWire,
    DtlsData,
    SctpOutput,
}

static BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
static DROPS: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
static PEAK_ITEMS: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
static PEAK_QUEUE_BYTES: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
static TEARDOWNS: AtomicUsize = AtomicUsize::new(0);
static GLOBAL_SHEDS: AtomicUsize = AtomicUsize::new(0);
static POOL: Mutex<Pool> = Mutex::new(Pool { queues: Vec::new() });

pub struct Metrics {
    pub bytes: usize,
    pub peak_bytes: usize,
    pub drops: [usize; 3],
    pub peak_items: [usize; 3],
    pub peak_queue_bytes: [usize; 3],
    pub teardowns: usize,
    pub global_sheds: usize,
}
pub fn metrics() -> Metrics {
    Metrics {
        bytes: BYTES.load(Ordering::Relaxed),
        peak_bytes: PEAK_BYTES.load(Ordering::Relaxed),
        drops: core::array::from_fn(|i| DROPS[i].load(Ordering::Relaxed)),
        peak_items: core::array::from_fn(|i| PEAK_ITEMS[i].load(Ordering::Relaxed)),
        peak_queue_bytes: core::array::from_fn(|i| PEAK_QUEUE_BYTES[i].load(Ordering::Relaxed)),
        teardowns: TEARDOWNS.load(Ordering::Relaxed),
        global_sheds: GLOBAL_SHEDS.load(Ordering::Relaxed),
    }
}
pub(crate) fn record_teardown() {
    TEARDOWNS.fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendError {
    Overloaded,
    /// This association was selected as the largest holder of the shared pool.
    GlobalPressure,
    Closed,
}

pub(crate) struct Association {
    bytes: AtomicUsize,
    closed: AtomicBool,
    on_shed: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
impl Association {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            bytes: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            on_shed: Mutex::new(None),
        })
    }
    pub(crate) fn on_shed(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.on_shed.lock() = Some(callback);
    }
    pub(crate) fn channel(self: &Arc<Self>, kind: Kind) -> (Sender, Receiver) {
        let queue = Arc::new(Queue {
            owner: self.clone(),
            state: Mutex::new(State {
                queue: VecDeque::new(),
                bytes: 0,
                closed: false,
                senders: 1,
                waker: None,
            }),
        });
        let mut pool = POOL.lock();
        pool.queues.retain(|q| q.strong_count() > 0);
        pool.queues.push(Arc::downgrade(&queue));
        (
            Sender {
                queue: queue.clone(),
                kind,
            },
            Receiver { queue, receives: 0 },
        )
    }
    pub(crate) fn close(&self) {
        let mut wakers = Vec::new();
        POOL.lock().close(self, &mut wakers);
        for waker in wakers {
            waker.wake();
        }
    }
}
struct Pool {
    queues: Vec<Weak<Queue>>,
}
impl Pool {
    fn largest(&self) -> Option<Arc<Association>> {
        self.queues
            .iter()
            .filter_map(Weak::upgrade)
            .map(|queue| queue.owner.clone())
            .filter(|owner| owner.bytes.load(Ordering::Relaxed) > 0)
            .max_by_key(|owner| owner.bytes.load(Ordering::Relaxed))
    }
    fn close(&mut self, owner: &Association, wakers: &mut Vec<Waker>) {
        owner.closed.store(true, Ordering::Release);
        for queue in self.queues.iter().filter_map(Weak::upgrade) {
            if !core::ptr::eq(queue.owner.as_ref(), owner) {
                continue;
            }
            let mut state = queue.state.lock();
            state.closed = true;
            clear(&mut state, owner);
            if let Some(waker) = state.waker.take() {
                wakers.push(waker);
            }
        }
    }
}
struct State {
    queue: VecDeque<Bytes>,
    bytes: usize,
    closed: bool,
    senders: usize,
    waker: Option<Waker>,
}
struct Queue {
    owner: Arc<Association>,
    state: Mutex<State>,
}
pub struct Sender {
    queue: Arc<Queue>,
    kind: Kind,
}
pub struct Receiver {
    queue: Arc<Queue>,
    receives: usize,
}

/// Standalone queue; transport stages instead share one Association.
pub fn channel(kind: Kind) -> (Sender, Receiver) {
    Association::new().channel(kind)
}

impl Sender {
    pub(crate) fn association(&self) -> Arc<Association> {
        self.queue.owner.clone()
    }
    pub fn send(&self, packet: Bytes) -> Result<(), SendError> {
        let mut wakers = Vec::new();
        let mut victims = Vec::new();
        // Lock order is pool then queue for admission, dequeue and cancellation.
        // Neither callbacks nor executor wakers run under either lock.
        let result = self.send_locked(&mut POOL.lock(), packet, &mut wakers, &mut victims);
        for victim in victims {
            let callback = victim.on_shed.lock().clone();
            if let Some(callback) = callback {
                callback();
            }
        }
        for waker in wakers {
            waker.wake();
        }
        result
    }
    fn send_locked(
        &self,
        pool: &mut Pool,
        packet: Bytes,
        wakers: &mut Vec<Waker>,
        victims: &mut Vec<Arc<Association>>,
    ) -> Result<(), SendError> {
        let state = self.queue.state.lock();
        let owner = &self.queue.owner;
        if state.closed || owner.closed.load(Ordering::Acquire) {
            return Err(SendError::Closed);
        }
        let bytes = packet.len().saturating_add(OVERHEAD);
        if packet.len() > MAX_PACKET
            || state.queue.len() == MAX_ITEMS
            || bytes > MAX_BYTES.saturating_sub(state.bytes)
            || bytes > ASSOCIATION_BYTES.saturating_sub(owner.bytes.load(Ordering::Relaxed))
        {
            DROPS[self.kind as usize].fetch_add(1, Ordering::Relaxed);
            return Err(SendError::Overloaded);
        }
        drop(state);
        while bytes > GLOBAL_BYTES.saturating_sub(BYTES.load(Ordering::Relaxed)) {
            // Positive accounting has a live registered queue. Admission and
            // release use this same lock, so the selected occupancy is stable.
            let Some(victim) = pool.largest() else {
                return Err(SendError::Closed);
            };
            pool.close(&victim, wakers);
            GLOBAL_SHEDS.fetch_add(1, Ordering::Relaxed);
            victims.push(victim);
            if owner.closed.load(Ordering::Acquire) {
                return Err(SendError::GlobalPressure);
            }
        }
        let mut state = self.queue.state.lock();
        // Own the admitted payload: a slice cannot retain a larger allocation.
        state.queue.push_back(Bytes::copy_from_slice(&packet));
        state.bytes += bytes;
        owner.bytes.fetch_add(bytes, Ordering::Relaxed);
        let before = BYTES.fetch_add(bytes, Ordering::AcqRel);
        PEAK_BYTES.fetch_max(before + bytes, Ordering::Relaxed);
        PEAK_ITEMS[self.kind as usize].fetch_max(state.queue.len(), Ordering::Relaxed);
        PEAK_QUEUE_BYTES[self.kind as usize].fetch_max(state.bytes, Ordering::Relaxed);
        if let Some(waker) = state.waker.take() {
            wakers.push(waker);
        }
        Ok(())
    }
}
impl Clone for Sender {
    fn clone(&self) -> Self {
        self.queue.state.lock().senders += 1;
        Self {
            queue: self.queue.clone(),
            kind: self.kind,
        }
    }
}
impl Drop for Sender {
    fn drop(&mut self) {
        let mut state = self.queue.state.lock();
        state.senders -= 1;
        let waker = if state.senders == 0 {
            state.waker.take()
        } else {
            None
        };
        drop(state);
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}
fn clear(state: &mut State, owner: &Association) {
    // Free payloads AND the queue allocation before making budget reusable.
    drop(core::mem::take(&mut state.queue));
    owner.bytes.fetch_sub(state.bytes, Ordering::Relaxed);
    BYTES.fetch_sub(state.bytes, Ordering::AcqRel);
    state.bytes = 0;
}
fn pop(state: &mut State, owner: &Association) -> Option<Bytes> {
    let packet = state.queue.pop_front()?;
    let bytes = packet.len() + OVERHEAD;
    state.bytes -= bytes;
    owner.bytes.fetch_sub(bytes, Ordering::Relaxed);
    BYTES.fetch_sub(bytes, Ordering::AcqRel);
    Some(packet)
}
impl Receiver {
    pub async fn recv(&mut self) -> Option<Bytes> {
        self.receives += 1;
        if self.receives == 64 {
            self.receives = 0;
            crate::platform::task::yield_now().await;
        }
        poll_fn(|cx| {
            let _pool = POOL.lock();
            let mut state = self.queue.state.lock();
            if let Some(packet) = pop(&mut state, &self.queue.owner) {
                return Poll::Ready(Some(packet));
            }
            if state.closed || state.senders == 0 {
                return Poll::Ready(None);
            }
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }
    pub fn try_recv(&mut self) -> Result<Bytes, ()> {
        let _pool = POOL.lock();
        pop(&mut self.queue.state.lock(), &self.queue.owner).ok_or(())
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        let _pool = POOL.lock();
        let mut state = self.queue.state.lock();
        state.closed = true;
        clear(&mut state, &self.queue.owner);
        state.waker = None;
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn packet_bounds_and_cancelled_receive() {
        let (tx, mut rx) = channel(Kind::DtlsWire);
        assert!(
            tokio::time::timeout(core::time::Duration::from_millis(1), rx.recv())
                .await
                .is_err()
        );
        for _ in 0..MAX_ITEMS {
            tx.send(Bytes::from_static(b"x")).unwrap();
        }
        assert_eq!(tx.send(Bytes::new()), Err(SendError::Overloaded));
        assert_eq!(rx.recv().await.unwrap(), Bytes::from_static(b"x"));
        tx.send(Bytes::new()).unwrap();
        drop(rx);
        assert_eq!(tx.queue.state.lock().bytes, 0);
        assert!(tx.queue.state.lock().queue.is_empty());
        assert_eq!(tx.send(Bytes::new()), Err(SendError::Closed));
    }
    #[test]
    fn global_byte_limit_and_release() {
        // Exact global accounting needs an isolated test process.
        const CHILD: &str = "RUSTRTC_RAW_QUEUE_GLOBAL_TEST";
        if std::env::var_os(CHILD).is_none() {
            let name = alloc::format!(
                "{}::global_byte_limit_and_release",
                module_path!().split_once("::").unwrap().1
            );
            assert!(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", &name])
                    .env(CHILD, "1")
                    .status()
                    .unwrap()
                    .success()
            );
            return;
        }
        let mut queues = alloc::vec::Vec::new();
        for _ in 0..9 {
            let (tx, rx) = channel(Kind::DtlsWire);
            while metrics().bytes + MAX_PACKET + OVERHEAD <= GLOBAL_BYTES
                && tx.queue.state.lock().bytes + MAX_PACKET + OVERHEAD <= MAX_BYTES
            {
                tx.send(Bytes::from(alloc::vec![0; MAX_PACKET])).unwrap();
            }
            queues.push((tx, rx));
        }
        // Fill the final remainder, including the 64-byte item allowance.
        let remainder = GLOBAL_BYTES - metrics().bytes;
        assert_eq!(remainder, 1087);
        queues
            .last()
            .unwrap()
            .0
            .send(Bytes::from(alloc::vec![0; remainder - OVERHEAD]))
            .unwrap();
        assert_eq!(metrics().bytes, GLOBAL_BYTES);
        assert_eq!(metrics().peak_bytes, GLOBAL_BYTES);
        // A small newcomer reclaims from a budget holder, rather than failing.
        let (light, mut light_rx) = channel(Kind::DtlsWire);
        light.send(Bytes::from_static(b"ok")).unwrap();
        assert_eq!(light_rx.try_recv().unwrap(), Bytes::from_static(b"ok"));
        assert_eq!(metrics().global_sheds, 1);
        drop(queues);
        drop((light, light_rx));
        assert_eq!(metrics().bytes, 0);

        fn fill(tx: &Sender, mut bytes: usize) {
            while bytes > 0 {
                let charge = bytes.min(MAX_PACKET + OVERHEAD);
                assert!(charge >= OVERHEAD);
                tx.send(Bytes::from(alloc::vec![0; charge - OVERHEAD]))
                    .unwrap();
                bytes -= charge;
            }
        }
        let owner = Association::new();
        let mut owned = Vec::new();
        for kind in [Kind::DtlsWire, Kind::DtlsData, Kind::SctpOutput] {
            let (tx, rx) = owner.channel(kind);
            fill(&tx, MAX_BYTES);
            owned.push((tx, rx));
        }
        assert_eq!(owner.bytes.load(Ordering::Relaxed), ASSOCIATION_BYTES);
        assert!(ASSOCIATION_BYTES < GLOBAL_BYTES);
        let (extra, extra_rx) = owner.channel(Kind::DtlsWire);
        assert_eq!(extra.send(Bytes::new()), Err(SendError::Overloaded));
        drop((owned, extra, extra_rx));
        assert_eq!(metrics().bytes, 0);

        // Sum all three stages when selecting the largest holder. None of
        // these queues alone is as large as the unrelated 6 MiB queues.
        for self_admission in [false, true] {
            let owner = Association::new();
            let notified = Arc::new(AtomicBool::new(false));
            let flag = notified.clone();
            let weak = Arc::downgrade(&owner);
            owner.on_shed(Arc::new(move || {
                flag.store(true, Ordering::Relaxed);
                // Teardown can re-enter the pool: no callback may hold its lock.
                weak.upgrade().unwrap().close();
            }));
            let mut owned = Vec::new();
            for kind in [Kind::DtlsWire, Kind::DtlsData, Kind::SctpOutput] {
                let (tx, rx) = owner.channel(kind);
                fill(&tx, 4 * 1024 * 1024);
                owned.push((tx, rx));
            }
            let mut others = Vec::new();
            while metrics().bytes < GLOBAL_BYTES {
                let (tx, rx) = channel(Kind::DtlsWire);
                fill(&tx, (GLOBAL_BYTES - metrics().bytes).min(6 * 1024 * 1024));
                others.push((tx, rx));
            }
            assert_eq!(metrics().bytes, GLOBAL_BYTES);
            let (light, mut light_rx) = channel(Kind::DtlsWire);
            if self_admission {
                assert_eq!(
                    owned[0].0.send(Bytes::new()),
                    Err(SendError::GlobalPressure)
                );
            } else {
                light.send(Bytes::from_static(b"ok")).unwrap();
                assert_eq!(light_rx.try_recv().unwrap(), Bytes::from_static(b"ok"));
            }
            assert!(notified.load(Ordering::Relaxed));
            assert_eq!(owner.bytes.load(Ordering::Relaxed), 0);
            for (tx, rx) in &mut owned {
                assert!(rx.try_recv().is_err());
                assert_eq!(tx.send(Bytes::new()), Err(SendError::Closed));
            }
            assert_eq!(metrics().bytes, GLOBAL_BYTES - 12 * 1024 * 1024);
            drop((owned, others, light, light_rx));
            assert_eq!(metrics().bytes, 0);
        }
    }

    #[test]
    fn concurrent_producers_obey_item_limit() {
        let (tx, rx) = channel(Kind::DtlsWire);
        let admitted = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let tx = tx.clone();
                let admitted = admitted.clone();
                scope.spawn(move || {
                    for _ in 0..MAX_ITEMS {
                        if tx.send(Bytes::new()).is_ok() {
                            admitted.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        assert_eq!(admitted.load(Ordering::Relaxed), MAX_ITEMS);
        drop(rx);
        assert_eq!(tx.queue.state.lock().bytes, 0);
    }

    #[tokio::test]
    async fn last_sender_wakes_receiver() {
        let (tx, mut rx) = channel(Kind::SctpOutput);
        let copy = tx.clone();
        drop(tx);
        let task = tokio::spawn(async move { rx.recv().await });
        tokio::task::yield_now().await;
        drop(copy);
        assert!(
            tokio::time::timeout(core::time::Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn byte_and_packet_limits() {
        let (tx, _rx) = channel(Kind::DtlsData);
        assert_eq!(
            tx.send(Bytes::from(alloc::vec![0; MAX_PACKET + 1])),
            Err(SendError::Overloaded)
        );
        let count = MAX_BYTES / (MAX_PACKET + OVERHEAD);
        for _ in 0..count {
            tx.send(Bytes::from(alloc::vec![0; MAX_PACKET])).unwrap();
        }
        assert_eq!(
            tx.send(Bytes::from(alloc::vec![0; MAX_PACKET])),
            Err(SendError::Overloaded)
        );
    }
}
