use crate::errors::RtcError;
use crate::platform::sync::{AsyncMutex as TokioMutex, mpsc};
use crate::platform::sync::{Mutex, Notify};
use crate::platform::time::Instant;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use core::time::Duration;

/// Local result alias (the module used `anyhow` before the no_std port).
pub(crate) type Result<T> = core::result::Result<T, RtcError>;

// DCEP Constants
pub const DATA_CHANNEL_PPID_DCEP: u32 = 50;
pub const DATA_CHANNEL_PPID_STRING: u32 = 51;
pub const DATA_CHANNEL_PPID_BINARY: u32 = 53;

pub const DCEP_TYPE_OPEN: u8 = 0x03;
pub const DCEP_TYPE_ACK: u8 = 0x02;

#[derive(Debug, Clone)]
pub struct DataChannelOpen {
    pub message_type: u8,
    pub channel_type: u8,
    pub priority: u16,
    pub reliability_parameter: u32,
    pub label: String,
    pub protocol: String,
}

impl DataChannelOpen {
    pub fn marshal(&self) -> Vec<u8> {
        let mut buf = BytesMut::new();
        buf.put_u8(self.message_type);
        buf.put_u8(self.channel_type);
        buf.put_u16(self.priority);
        buf.put_u32(self.reliability_parameter);

        let label_bytes = self.label.as_bytes();
        buf.put_u16(label_bytes.len() as u16);

        let protocol_bytes = self.protocol.as_bytes();
        buf.put_u16(protocol_bytes.len() as u16);

        buf.put_slice(label_bytes);
        buf.put_slice(protocol_bytes);

        buf.to_vec()
    }

    pub fn unmarshal(data: &[u8]) -> Result<Self> {
        let mut buf = Bytes::copy_from_slice(data);
        if buf.remaining() < 12 {
            return Err(RtcError::Internal(
                "DCEP Open message too short".to_string(),
            ));
        }

        let message_type = buf.get_u8();
        if message_type != DCEP_TYPE_OPEN {
            return Err(RtcError::Internal("Invalid DCEP message type".to_string()));
        }

        let channel_type = buf.get_u8();
        let priority = buf.get_u16();
        let reliability_parameter = buf.get_u32();
        let label_len = buf.get_u16() as usize;
        let protocol_len = buf.get_u16() as usize;

        if buf.remaining() < label_len + protocol_len {
            return Err(RtcError::Internal(
                "DCEP Open message too short for payload".to_string(),
            ));
        }

        let label_bytes = buf.split_to(label_len);
        let protocol_bytes = buf.split_to(protocol_len);

        let label = String::from_utf8(label_bytes.to_vec())?;
        let protocol = String::from_utf8(protocol_bytes.to_vec())?;

        Ok(Self {
            message_type,
            channel_type,
            priority,
            reliability_parameter,
            label,
            protocol,
        })
    }
}

#[derive(Debug, Clone)]
pub struct DataChannelAck {
    pub message_type: u8,
}

impl DataChannelAck {
    pub fn marshal(&self) -> Vec<u8> {
        vec![self.message_type]
    }

    pub fn unmarshal(data: &[u8]) -> Result<Self> {
        if data.is_empty() {
            return Err(RtcError::Internal("DCEP Ack message too short".to_string()));
        }
        let message_type = data[0];
        if message_type != DCEP_TYPE_ACK {
            return Err(RtcError::Internal("Invalid DCEP message type".to_string()));
        }
        Ok(Self { message_type })
    }
}

#[derive(Debug, Clone)]
pub enum DataChannelEvent {
    Open,
    Message(Bytes),
    Close,
    /// The per-channel buffered amount dropped to (or below) the configured
    /// low threshold after having been above it (W3C `bufferedamountlow`).
    /// Carries the current buffered amount in bytes.
    BufferedAmountLow(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum DataChannelState {
    Connecting = 0,
    Open = 1,
    Closing = 2,
    Closed = 3,
}

impl From<usize> for DataChannelState {
    fn from(v: usize) -> Self {
        match v {
            0 => DataChannelState::Connecting,
            1 => DataChannelState::Open,
            2 => DataChannelState::Closing,
            3 => DataChannelState::Closed,
            _ => DataChannelState::Closed,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DataChannelConfig {
    pub label: String,
    pub protocol: String,
    pub ordered: bool,
    pub max_retransmits: Option<u16>,
    pub max_packet_life_time: Option<u16>,
    pub max_payload_size: Option<usize>,
    pub negotiated: Option<u16>,
    /// W3C `bufferedAmountLowThreshold`: when the per-channel buffered amount
    /// falls back to (or below) this value after having exceeded it, a
    /// `DataChannelEvent::BufferedAmountLow` is emitted. `0` (default) never
    /// fires the event.
    pub buffered_amount_low_threshold: usize,
}

pub struct DataChannel {
    pub id: u16,
    pub label: String,
    pub protocol: String,
    pub ordered: bool,
    pub max_retransmits: Option<u16>,
    pub max_packet_life_time: Option<u16>,
    pub max_payload_size: usize,
    pub negotiated: bool,
    pub state: AtomicUsize,
    pub next_ssn: AtomicU16,
    tx: Mutex<Option<mpsc::UnboundedSender<DataChannelEvent>>>,
    rx: TokioMutex<mpsc::UnboundedReceiver<DataChannelEvent>>,
    pub(crate) reassembly_buffer: Mutex<BytesMut>,
    pub(crate) send_lock: TokioMutex<()>,
    /// Bytes sent on this channel that SCTP has not acknowledged yet
    /// (queued + in flight). Mirrored from the transport's per-stream
    /// accounting; read lock-free by `buffered_amount()`.
    buffered_amount: AtomicUsize,
    buffered_amount_low_threshold: AtomicUsize,
    /// Edge state for `BufferedAmountLow`: `true` once the amount has risen
    /// above the threshold and the drop event has not been emitted yet.
    above_low_threshold: AtomicBool,
    /// What the messages delivered to the event queue and not yet taken by
    /// `recv` hold, `MESSAGE_OVERHEAD` each included (vendored change: the
    /// receive window and a hard limit count them).
    unread_bytes: AtomicUsize,
    /// Set by the SCTP transport before the first message: the association's
    /// receive window, which counts this channel's unread bytes too.
    pub(crate) receive_window: Mutex<Option<Arc<ReceiveWindow>>>,
}

/// What a queued message holds besides its payload, about: an empty message
/// is not free (vendored change).
pub const MESSAGE_OVERHEAD: usize = 64;

/// Coalesce repeated reader updates, including drains between SACKs, without
/// waiting for another DATA packet (RFC 9260 section 6.2).
const WINDOW_UPDATE_INTERVAL: Duration = Duration::from_millis(10);

/// The last offered window and an optional reader-triggered update. Keeping
/// the snapshot together also lets advertise() catch a read that raced with
/// construction of its SACK (vendored change: flow control).
struct WindowAdvertisement {
    available: usize,
    rwnd: usize,
    update_at: Option<Instant>,
    last_update: Option<Instant>,
}

/// Association-wide unread accounting and receiver SWS avoidance. Ordinary
/// DATA SACKs can carry an update; otherwise the SCTP timer sends it promptly.
pub(crate) struct ReceiveWindow {
    update_step: usize,
    unread: AtomicUsize,
    advertisement: Mutex<WindowAdvertisement>,
    wake: Arc<Notify>,
    /// SACKs built and reader-triggered updates taken by the transport.
    pub(crate) sacks: AtomicUsize,
    pub(crate) updates: AtomicUsize,
}

impl ReceiveWindow {
    pub(crate) fn new(capacity: usize, pmtu: usize, wake: Arc<Notify>) -> Self {
        Self {
            // RFC 9260 section 6.2 and RFC 1122 section 4.2.3.3:
            // advance by min(one PMTU, half the receive buffer).
            update_step: pmtu.min(capacity / 2).max(1),
            unread: AtomicUsize::new(0),
            advertisement: Mutex::new(WindowAdvertisement {
                available: capacity,
                rwnd: capacity,
                update_at: None,
                last_update: None,
            }),
            wake,
            sacks: AtomicUsize::new(0),
            updates: AtomicUsize::new(0),
        }
    }

    pub(crate) fn unread(&self) -> usize {
        self.unread.load(Ordering::SeqCst)
    }

    fn needs_update(&self, adv: &WindowAdvertisement) -> bool {
        // Compare actual available space to what the peer knows, not just
        // bytes drained: a full window can have overshot due to message cost.
        adv.available
            .saturating_sub(self.unread())
            .saturating_sub(adv.rwnd)
            >= self.update_step
    }

    fn refresh_pending(&self, adv: &mut WindowAdvertisement) {
        if !self.needs_update(adv) {
            adv.update_at = None;
        } else if adv.update_at.is_none() {
            let now = Instant::now();
            // A near-zero window needs prompt reopening. With room still
            // available, let the next DATA SACK carry the update first.
            let coalesce_until = if adv.rwnd > self.update_step {
                now + WINDOW_UPDATE_INTERVAL
            } else {
                now
            };
            let rate_limit = adv.last_update.map(|last| last + WINDOW_UPDATE_INTERVAL);
            adv.update_at = Some(coalesce_until.max(rate_limit.unwrap_or(now)));
            self.wake.notify_one();
        }
    }

    /// A SACK advertises `rwnd`. `available` is the space for unread messages
    /// after SCTP's reassembly byte and chunk limits have been applied.
    pub(crate) fn advertise(&self, available: usize, rwnd: usize) {
        let mut adv = self.advertisement.lock();
        adv.available = available;
        adv.rwnd = rwnd;
        // A normal SACK can satisfy a pending update. Conversely, a reader
        // may have drained after the SACK's unread snapshot but before here.
        self.refresh_pending(&mut adv);
        self.sacks.fetch_add(1, Ordering::Relaxed);
    }

    fn charge(&self, n: usize) {
        self.unread.fetch_add(n, Ordering::SeqCst);
    }

    fn release(&self, n: usize) {
        let _ = self
            .unread
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |u| {
                Some(u.saturating_sub(n))
            });
        self.refresh_pending(&mut self.advertisement.lock());
    }

    /// Reopen a closed window immediately, subject to the 10 ms rate bound.
    /// Updates to an open window wait up to 10 ms for an ordinary DATA SACK.
    pub(crate) fn take_update(&self, now: Instant) -> bool {
        let mut adv = self.advertisement.lock();
        let Some(deadline) = adv.update_at else {
            return false;
        };
        if !self.needs_update(&adv) {
            adv.update_at = None;
            return false;
        }
        if now < deadline {
            return false;
        }
        adv.update_at = None;
        adv.last_update = Some(now);
        self.updates.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub(crate) fn update_timeout(&self, now: Instant) -> Duration {
        match self.advertisement.lock().update_at {
            Some(deadline) if deadline > now => deadline - now,
            Some(_) => Duration::from_millis(1),
            None => Duration::from_secs(3600),
        }
    }
}

fn message_charge(event: &DataChannelEvent) -> usize {
    match event {
        DataChannelEvent::Message(m) => m.len() + MESSAGE_OVERHEAD,
        _ => 0,
    }
}

impl DataChannel {
    pub fn new(id: u16, config: DataChannelConfig) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            id,
            label: config.label,
            protocol: config.protocol,
            ordered: config.ordered,
            max_retransmits: config.max_retransmits,
            max_packet_life_time: config.max_packet_life_time,
            max_payload_size: config.max_payload_size.unwrap_or(1200),
            negotiated: config.negotiated.is_some(),
            state: AtomicUsize::new(DataChannelState::Connecting as usize),
            next_ssn: AtomicU16::new(0),
            tx: Mutex::new(Some(tx)),
            rx: TokioMutex::new(rx),
            reassembly_buffer: Mutex::new(BytesMut::new()),
            send_lock: TokioMutex::new(()),
            buffered_amount: AtomicUsize::new(0),
            buffered_amount_low_threshold: AtomicUsize::new(config.buffered_amount_low_threshold),
            above_low_threshold: AtomicBool::new(false),
            unread_bytes: AtomicUsize::new(0),
            receive_window: Mutex::new(None),
        }
    }

    /// Number of bytes queued for this channel that the SCTP layer has not
    /// acknowledged yet (W3C `bufferedAmount`).
    pub fn buffered_amount(&self) -> usize {
        self.buffered_amount.load(Ordering::SeqCst)
    }

    /// Set the low-water threshold for `BufferedAmountLow` events
    /// (W3C `bufferedAmountLowThreshold`).
    pub fn set_buffered_amount_low_threshold(&self, threshold: usize) {
        self.buffered_amount_low_threshold
            .store(threshold, Ordering::SeqCst);
    }

    pub fn buffered_amount_low_threshold(&self) -> usize {
        self.buffered_amount_low_threshold.load(Ordering::SeqCst)
    }

    /// Called by the SCTP transport whenever this channel's buffered amount
    /// changes. Emits `BufferedAmountLow` on the above → at-or-below edge.
    pub(crate) fn on_buffered_amount(&self, amount: usize) {
        let prev = self.buffered_amount.swap(amount, Ordering::SeqCst);
        if prev == amount {
            return;
        }
        let threshold = self.buffered_amount_low_threshold.load(Ordering::SeqCst);
        if threshold == 0 {
            return;
        }
        if amount > threshold {
            self.above_low_threshold.store(true, Ordering::SeqCst);
        } else if self.above_low_threshold.swap(false, Ordering::SeqCst) {
            self.send_event(DataChannelEvent::BufferedAmountLow(amount));
        }
    }

    pub async fn recv(&self) -> Option<DataChannelEvent> {
        let mut rx = self.rx.lock().await;
        let event = rx.recv().await;
        if let Some(event) = &event {
            let charge = message_charge(event);
            if charge > 0 {
                self.unread_bytes.fetch_sub(charge, Ordering::SeqCst);
                if let Some(window) = self.receive_window.lock().as_ref() {
                    window.release(charge);
                }
            }
        }
        event
    }

    /// What the delivered messages not yet taken by `recv` hold:
    /// their bytes plus `MESSAGE_OVERHEAD` each.
    pub fn unread_bytes(&self) -> usize {
        self.unread_bytes.load(Ordering::SeqCst)
    }

    pub(crate) fn send_event(&self, event: DataChannelEvent) {
        if let Some(tx) = &*self.tx.lock() {
            let charge = message_charge(&event);
            // Counted before the send, so `recv` never subtracts first.
            self.unread_bytes.fetch_add(charge, Ordering::SeqCst);
            let window = self.receive_window.lock().clone();
            if let Some(window) = &window {
                window.charge(charge);
            }
            if tx.send(event).is_err() {
                self.unread_bytes.fetch_sub(charge, Ordering::SeqCst);
                if let Some(window) = &window {
                    window.release(charge);
                }
            }
        }
    }

    /// `Connecting` to `Open`, with the `Open` event, once only: a
    /// negotiated channel is opened both when the association comes up and
    /// when it is made on an association already up (vendored change).
    pub(crate) fn mark_open(&self) {
        if self
            .state
            .compare_exchange(
                DataChannelState::Connecting as usize,
                DataChannelState::Open as usize,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            self.send_event(DataChannelEvent::Open);
        }
    }

    pub(crate) fn close_channel(&self) {
        *self.tx.lock() = None;
    }
}

impl Drop for DataChannel {
    /// What nobody read any more stops counting against the association's
    /// window (vendored change).
    fn drop(&mut self) {
        let unread = self.unread_bytes.load(Ordering::SeqCst);
        if let Some(window) = self.receive_window.lock().as_ref() {
            window.release(unread);
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod receive_window_tests {
    use super::*;
    use crate::platform::time::Instant;
    use core::time::Duration;

    #[test]
    fn update_requires_one_pmtu_or_half_the_buffer() {
        // Expected thresholds from RFC 1122 section 4.2.3.3, as referenced by
        // RFC 9260 section 6.2, including a buffer smaller than two PMTUs.
        for (capacity, pmtu, step) in [(128 * 1024, 1200, 1200), (1024, 1200, 512)] {
            let w = ReceiveWindow::new(capacity, pmtu, Arc::new(Notify::new()));
            w.charge(capacity);
            w.advertise(capacity, 0);
            w.release(step - 1);
            assert!(!w.take_update(Instant::now()));
            w.release(1);
            assert!(w.take_update(Instant::now()));
            assert_eq!(w.updates.load(Ordering::Relaxed), 1);
        }
    }

    #[test]
    fn overshoot_and_reassembly_must_be_freed_before_advancing() {
        let w = ReceiveWindow::new(128 * 1024, 1200, Arc::new(Notify::new()));
        let used = 2000;
        w.charge(128 * 1024 - used + 512);
        w.advertise(128 * 1024 - used, 0);
        w.release(1200);
        assert!(!w.take_update(Instant::now()), "only 688 bytes available");
        w.release(512);
        assert!(w.take_update(Instant::now()));
    }

    #[test]
    fn a_read_racing_with_the_sack_snapshot_still_wakes_the_transport() {
        let w = ReceiveWindow::new(128 * 1024, 1200, Arc::new(Notify::new()));
        w.charge(128 * 1024);
        // SCTP snapshots a zero window here, then the reader runs before that
        // snapshot is recorded. The previous advertised window was still full.
        w.release(1200);
        assert!(!w.take_update(Instant::now()));
        w.advertise(128 * 1024, 0);
        assert!(w.take_update(Instant::now()));
    }

    #[test]
    fn ordinary_sacks_and_new_data_cancel_redundant_updates() {
        let w = ReceiveWindow::new(128 * 1024, 1200, Arc::new(Notify::new()));
        w.charge(128 * 1024);
        w.advertise(128 * 1024, 0);
        w.release(1200);
        w.advertise(128 * 1024, 1200);
        assert!(!w.take_update(Instant::now()));
        w.release(1200);
        w.charge(1200);
        assert!(!w.take_update(Instant::now()));
        assert_eq!(w.updates.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_reassembly_chunk_limit_does_not_schedule_reader_updates() {
        let w = ReceiveWindow::new(128 * 1024, 1200, Arc::new(Notify::new()));
        w.charge(1200);
        // SCTP has withheld the whole window because its chunk queue is full.
        w.advertise(0, 0);
        w.release(1200);
        assert!(!w.take_update(Instant::now()));
        assert_eq!(w.update_timeout(Instant::now()), Duration::from_secs(3600));
        // Receiving the missing DATA frees reassembly and sends a normal SACK.
        w.advertise(128 * 1024, 128 * 1024);
        assert!(!w.take_update(Instant::now()));
        assert_eq!(w.updates.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn an_open_window_waits_for_a_piggyback_before_its_first_update() {
        let capacity = 128 * 1024;
        let w = ReceiveWindow::new(capacity, 1200, Arc::new(Notify::new()));
        let now = Instant::now();
        w.charge(2400);
        w.advertise(capacity, capacity - 2400);
        w.release(1200);
        assert!(!w.take_update(now));
        let deadline = w.advertisement.lock().update_at.unwrap();
        assert!(deadline >= now + WINDOW_UPDATE_INTERVAL);
        assert!(!w.take_update(deadline - Duration::from_millis(1)));
        assert!(w.take_update(deadline));
        assert_eq!(w.updates.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn repeated_reads_coalesce_but_keep_a_timer_without_more_data() {
        let w = ReceiveWindow::new(128 * 1024, 1200, Arc::new(Notify::new()));
        let now = Instant::now();
        w.charge(128 * 1024);
        w.advertise(128 * 1024, 0);
        w.release(1200);
        assert!(w.take_update(now));
        w.advertise(128 * 1024, 1200);
        for _ in 0..20 {
            w.release(1200);
            assert!(!w.take_update(now));
        }
        assert_eq!(w.update_timeout(now), WINDOW_UPDATE_INTERVAL);
        assert!(!w.take_update(now + WINDOW_UPDATE_INTERVAL - Duration::from_millis(1)));
        assert!(w.take_update(now + WINDOW_UPDATE_INTERVAL));
        assert_eq!(w.updates.load(Ordering::Relaxed), 2);
        w.advertise(128 * 1024, 21 * 1200);
        assert!(!w.take_update(now + WINDOW_UPDATE_INTERVAL * 2));
    }
}
