use std::{
    collections::{BTreeMap, VecDeque},
    net::SocketAddr,
    time::Duration,
};

use rand::Rng;
use redux::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{Limit, P2pLimits, P2pTimeouts, PeerId};

/// When we may dial a libp2p peer, an OCaml node, again.
///
/// OCaml never redials a peer that disconnects: the helper only reports it
/// (`app.go`), OCaml ignores it (`on_peer_disconnected:ignore`), and new
/// connections come from its discovery loop, only while it has fewer than its
/// low-water 20 connections. go-libp2p backs off failed dials per peer and
/// address (5 s, then `5 s + 1 s * tries^2`, at most 5 min) and clears that
/// on any successful connection. We redial on every disconnection, so a peer
/// that accepts us and trims us 15 to 25 s later (its connection manager's
/// grace period and 10 s tick) was redialled 47 to 102 times in 30 minutes.
///
/// So the record here backs off both failed dials and connections that end
/// before they were healthy, per peer id and per address, and outlives the
/// peer's entry in the peer table:
///
/// - After the `n`th failure in a row the next dial waits `base * 2^(n-1)`,
///   at most [`P2pDialBackoffConfig::max`], each delay jittered by up to
///   [`Self::JITTER`] either way. `base` is the reconnect timeout after a
///   disconnection and the outgoing error timeout after a failed dial (30 s
///   each by default).
/// - A connection that stayed ready for [`P2pDialBackoffConfig::healthy_after`]
///   resets the count: when it ends, the next dial waits one `base`.
/// - A peer id whose address answers the handshake with another peer id is
///   stale there: OCaml makes a new libp2p key at each start. We don't dial
///   it at that address again for [`P2pDialBackoffConfig::stale_for`].
/// - Dials to a peer id or an address dialled before are redials, at most
///   [`P2pDialBackoffConfig::max_redials_per_minute`] in any minute. First
///   dials to new peers are discovery and don't count against it, but all
///   dials together are held to [`P2pDialBackoffConfig::max_dials_per_minute`].
///
/// Only libp2p peers are kept here; WebRTC peers keep the plain timeouts.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/app/libp2p_helper/src/libp2p_helper/app.go#L51-L54>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/gossip_net/libp2p.ml#L258>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/app/libp2p_helper/src/libp2p_helper/config_msg.go#L145-L171>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/gossip_net/libp2p.ml#L263-L274>
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct P2pDialBackoff {
    by_peer: BTreeMap<PeerId, P2pDialBackoffEntry>,
    by_addr: BTreeMap<SocketAddr, P2pDialBackoffEntry>,
    stale: BTreeMap<PeerId, P2pStalePeerId>,
    redials: VecDeque<Timestamp>,
    #[serde(default)]
    last_recovery: Option<Timestamp>,
    /// Every dial in the last minute, first dials and redials.
    #[serde(default)]
    dials: VecDeque<Timestamp>,
}

/// What we remember about dials to one peer id or one address.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2pDialBackoffEntry {
    /// Failed dials and unhealthy connections in a row.
    pub failures: u32,
    /// No dial before this time.
    pub retry_at: Timestamp,
    /// When the current connection became ready, while it is.
    pub ready_since: Option<Timestamp>,
    /// For a peer id: the address we dialled it at last, while we dial or are
    /// connected through it.
    pub addr: Option<SocketAddr>,
    /// The last dial, connection or failure: the least recent are forgotten
    /// first.
    pub last_event: Timestamp,
}

impl P2pDialBackoffEntry {
    fn new(now: Timestamp) -> Self {
        Self {
            failures: 0,
            retry_at: now,
            ready_since: None,
            addr: None,
            last_event: now,
        }
    }
}

/// A peer id that another peer id answered for at `addr`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2pStalePeerId {
    pub addr: SocketAddr,
    pub answered_by: PeerId,
    pub time: Timestamp,
}

/// How a dial or a connection ended, for [`P2pDialBackoff`].
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2pDialEnd {
    /// The dial failed before the peer was ready.
    Failed,
    /// The connection ended before it was healthy.
    Unhealthy,
    /// The connection had been ready for long enough: the count is reset.
    Healthy,
}

/// The backoff's parameters, from [`P2pTimeouts`] and [`P2pLimits`].
#[derive(Debug, Clone, Copy)]
pub struct P2pDialBackoffConfig {
    /// The first wait after a connection ends.
    pub after_disconnect: Duration,
    /// The first wait after a failed dial.
    pub after_error: Duration,
    /// The longest wait, before jitter.
    pub max: Duration,
    /// How long a connection must stay ready to reset the backoff.
    pub healthy_after: Duration,
    /// How long a stale peer id isn't dialled at its old address.
    pub stale_for: Duration,
    /// Redials in any minute, at most.
    pub max_redials_per_minute: Limit<usize>,
    /// Dials of any kind in any minute, at most.
    pub max_dials_per_minute: Limit<usize>,
}

impl P2pDialBackoffConfig {
    pub const DEFAULT_BASE: Duration = Duration::from_secs(30);
    pub const DEFAULT_MAX: Duration = Duration::from_secs(10 * 60);
    pub const DEFAULT_HEALTHY_AFTER: Duration = Duration::from_secs(60);
    pub const DEFAULT_STALE_FOR: Duration = Duration::from_secs(60 * 60);

    pub fn new(timeouts: &P2pTimeouts, limits: &P2pLimits) -> Self {
        Self {
            after_disconnect: timeouts.reconnect_timeout.unwrap_or(Self::DEFAULT_BASE),
            after_error: timeouts
                .outgoing_error_reconnect_timeout
                .unwrap_or(Self::DEFAULT_BASE),
            max: timeouts.reconnect_backoff_max.unwrap_or(Self::DEFAULT_MAX),
            healthy_after: timeouts
                .healthy_connection
                .unwrap_or(Self::DEFAULT_HEALTHY_AFTER),
            stale_for: timeouts.stale_peer_id.unwrap_or(Self::DEFAULT_STALE_FOR),
            max_redials_per_minute: limits.max_redials_per_minute(),
            max_dials_per_minute: limits.max_dials_per_minute(),
        }
    }
}

impl P2pDialBackoff {
    /// Each delay is multiplied by a factor from `1 - JITTER` to `1 + JITTER`,
    /// so that peers dropped together are not dialled again together.
    pub const JITTER: f64 = 0.2;
    /// The window of the redial cap.
    pub const REDIAL_WINDOW: Duration = Duration::from_secs(60);
    /// A small devnet or a correlated outage must not wait ten minutes to probe.
    /// Below three ready libp2p peers, allow one early retry every 15 seconds.
    pub const RECOVERY_PEER_FLOOR: usize = 3;
    pub const RECOVERY_INTERVAL: Duration = Duration::from_secs(15);
    /// Peer ids, addresses and stale ids remembered at most, each.
    const MAX_KEYS: usize = 1024;

    /// The entry for `peer_id`, if we have dialled it or it connected.
    pub fn peer(&self, peer_id: &PeerId) -> Option<&P2pDialBackoffEntry> {
        self.by_peer.get(peer_id)
    }

    /// The entry for `addr`, if we have dialled it.
    pub fn addr(&self, addr: &SocketAddr) -> Option<&P2pDialBackoffEntry> {
        self.by_addr.get(addr)
    }

    /// The stale record for `peer_id`, if any.
    pub fn stale(&self, peer_id: &PeerId) -> Option<&P2pStalePeerId> {
        self.stale.get(peer_id)
    }

    /// Whether `peer_id` is stale at `addr` at `now`: that address answered
    /// as another peer id, less than `stale_for` ago.
    pub fn is_stale(
        &self,
        peer_id: &PeerId,
        addr: Option<SocketAddr>,
        now: Timestamp,
        config: &P2pDialBackoffConfig,
    ) -> bool {
        self.stale.get(peer_id).is_some_and(|stale| {
            Some(stale.addr) == addr
                && now.checked_sub(stale.time).unwrap_or_default() < config.stale_for
        })
    }

    /// The earliest time we may dial `peer_id` at `addr` again, if we must
    /// wait at all.
    pub fn retry_at(&self, peer_id: &PeerId, addr: Option<SocketAddr>) -> Option<Timestamp> {
        let by_peer = self.by_peer.get(peer_id).map(|e| e.retry_at);
        let by_addr = addr.and_then(|addr| self.by_addr.get(&addr).map(|e| e.retry_at));
        by_peer.max(by_addr)
    }

    /// Whether a dial to `peer_id` at `addr` would be a redial: we dialled
    /// that peer id or that address before, or the peer connected to us.
    pub fn is_redial(&self, peer_id: &PeerId, addr: Option<SocketAddr>) -> bool {
        self.by_peer.contains_key(peer_id)
            || addr.is_some_and(|addr| self.by_addr.contains_key(&addr))
    }

    /// Redials in the minute that ends at `now`.
    pub fn redials_in_window(&self, now: Timestamp) -> usize {
        Self::in_window(&self.redials, now)
    }

    /// Dials of any kind in the minute that ends at `now`.
    pub fn dials_in_window(&self, now: Timestamp) -> usize {
        Self::in_window(&self.dials, now)
    }

    fn in_window(times: &VecDeque<Timestamp>, now: Timestamp) -> usize {
        times
            .iter()
            .filter(|t| now.checked_sub(**t).unwrap_or_default() < Self::REDIAL_WINDOW)
            .count()
    }

    /// Whether the overall cap allows another dial at `now`.
    fn dial_rate_allows(&self, now: Timestamp, config: &P2pDialBackoffConfig) -> bool {
        self.dials_in_window(now) < config.max_dials_per_minute
    }

    /// Whether we may dial `peer_id` at `addr` at `now`: it is not stale there,
    /// neither the peer id nor the address is backing off, the overall dial
    /// cap allows it and, for a redial, the redial cap does too.
    pub fn allows(
        &self,
        peer_id: &PeerId,
        addr: Option<SocketAddr>,
        now: Timestamp,
        config: &P2pDialBackoffConfig,
    ) -> bool {
        if self.is_stale(peer_id, addr, now, config) {
            return false;
        }
        if self.retry_at(peer_id, addr).is_some_and(|t| now < t) {
            return false;
        }
        if !self.dial_rate_allows(now, config) {
            return false;
        }
        !self.is_redial(peer_id, addr)
            || self.redials_in_window(now) < config.max_redials_per_minute
    }

    /// An early retry only bypasses the exponential delay, never quarantine
    /// or the shared redial cap. Also wait 15 seconds after this peer's last
    /// event so ordinary short backoffs retain their behavior.
    pub fn allows_recovery(
        &self,
        peer_id: &PeerId,
        addr: Option<SocketAddr>,
        now: Timestamp,
        config: &P2pDialBackoffConfig,
    ) -> bool {
        self.retry_at(peer_id, addr).is_some_and(|t| now < t)
            && self
                .last_recovery
                .is_none_or(|t| now.checked_sub(t).unwrap_or_default() >= Self::RECOVERY_INTERVAL)
            && self.peer(peer_id).is_some_and(|entry| {
                now.checked_sub(entry.last_event).unwrap_or_default() >= Self::RECOVERY_INTERVAL
            })
            && !self.is_stale(peer_id, addr, now, config)
            && self.redials_in_window(now) < config.max_redials_per_minute
            && self.dial_rate_allows(now, config)
    }

    pub fn on_recovery(&mut self, now: Timestamp) {
        self.last_recovery = Some(now);
    }

    /// Records a dial to `peer_id` at `addr` at `now`.
    pub fn on_dial(&mut self, peer_id: PeerId, addr: Option<SocketAddr>, now: Timestamp) {
        if self.is_redial(&peer_id, addr) {
            Self::push_to_window(&mut self.redials, now);
        }
        Self::push_to_window(&mut self.dials, now);
        let entry = self
            .by_peer
            .entry(peer_id)
            .or_insert_with(|| P2pDialBackoffEntry::new(now));
        entry.addr = addr;
        entry.ready_since = None;
        entry.last_event = now;
        if let Some(addr) = addr {
            let entry = self
                .by_addr
                .entry(addr)
                .or_insert_with(|| P2pDialBackoffEntry::new(now));
            entry.ready_since = None;
            entry.last_event = now;
        }
        self.forget_least_recent(now);
    }

    fn push_to_window(times: &mut VecDeque<Timestamp>, now: Timestamp) {
        while times
            .front()
            .is_some_and(|t| now.checked_sub(*t).unwrap_or_default() >= Self::REDIAL_WINDOW)
        {
            times.pop_front();
        }
        times.push_back(now);
        // With no cap, keep the window bounded anyway.
        while times.len() > Self::MAX_KEYS {
            times.pop_front();
        }
    }

    /// Records that the connection with `peer_id` became ready at `now`, on
    /// an address we dialled unless `incoming`.
    pub fn on_ready(&mut self, peer_id: PeerId, incoming: bool, now: Timestamp) {
        let entry = self
            .by_peer
            .entry(peer_id)
            .or_insert_with(|| P2pDialBackoffEntry::new(now));
        if incoming {
            entry.addr = None;
        }
        entry.ready_since = Some(now);
        entry.last_event = now;
        if let Some(addr) = entry.addr {
            let entry = self
                .by_addr
                .entry(addr)
                .or_insert_with(|| P2pDialBackoffEntry::new(now));
            entry.ready_since = Some(now);
            entry.last_event = now;
        }
        self.forget_least_recent(now);
    }

    /// Records that our dial to `peer_id` failed at `now`. Returns the end and
    /// how long we wait before dialling it again.
    pub fn on_dial_failed(
        &mut self,
        peer_id: PeerId,
        now: Timestamp,
        config: &P2pDialBackoffConfig,
    ) -> (P2pDialEnd, Duration) {
        self.end(peer_id, now, config, false)
    }

    /// Records that the connection with `peer_id` ended at `now`. Returns
    /// whether it was healthy and how long we wait before dialling it again.
    pub fn on_disconnected(
        &mut self,
        peer_id: PeerId,
        now: Timestamp,
        config: &P2pDialBackoffConfig,
    ) -> (P2pDialEnd, Duration) {
        self.end(peer_id, now, config, true)
    }

    fn end(
        &mut self,
        peer_id: PeerId,
        now: Timestamp,
        config: &P2pDialBackoffConfig,
        was_connected: bool,
    ) -> (P2pDialEnd, Duration) {
        let peer = self
            .by_peer
            .entry(peer_id)
            .or_insert_with(|| P2pDialBackoffEntry::new(now));
        let end = match peer.ready_since {
            Some(since)
                if was_connected
                    && now.checked_sub(since).unwrap_or_default() >= config.healthy_after =>
            {
                P2pDialEnd::Healthy
            }
            _ if was_connected => P2pDialEnd::Unhealthy,
            _ => P2pDialEnd::Failed,
        };
        let base = match end {
            P2pDialEnd::Failed => config.after_error,
            P2pDialEnd::Unhealthy | P2pDialEnd::Healthy => config.after_disconnect,
        };
        let jitter = jitter_factor(now);
        let addr = peer.addr;
        let delay = Self::apply_end(peer, end, base, jitter, now, config);
        if let Some(addr) = addr {
            let entry = self
                .by_addr
                .entry(addr)
                .or_insert_with(|| P2pDialBackoffEntry::new(now));
            Self::apply_end(entry, end, base, jitter, now, config);
        }
        self.forget_least_recent(now);
        (end, delay)
    }

    fn apply_end(
        entry: &mut P2pDialBackoffEntry,
        end: P2pDialEnd,
        base: Duration,
        jitter: f64,
        now: Timestamp,
        config: &P2pDialBackoffConfig,
    ) -> Duration {
        entry.failures = match end {
            P2pDialEnd::Healthy => 0,
            P2pDialEnd::Failed | P2pDialEnd::Unhealthy => entry.failures.saturating_add(1),
        };
        let delay = Self::delay(base, entry.failures, config.max).mul_f64(jitter);
        entry.retry_at = now + delay;
        entry.ready_since = None;
        entry.last_event = now;
        delay
    }

    /// The wait after `failures` failures in a row, before jitter: one `base`
    /// after none or one, then doubling, at most `max`.
    pub fn delay(base: Duration, failures: u32, max: Duration) -> Duration {
        let doublings = failures.saturating_sub(1).min(20);
        base.saturating_mul(1 << doublings).min(max)
    }

    /// Records that `addr` answered as `answered_by` when we dialled
    /// `peer_id` at `now`.
    pub fn on_stale(
        &mut self,
        peer_id: PeerId,
        addr: SocketAddr,
        answered_by: PeerId,
        now: Timestamp,
    ) {
        self.stale.insert(
            peer_id,
            P2pStalePeerId {
                addr,
                answered_by,
                time: now,
            },
        );
        if self.stale.len() > Self::MAX_KEYS {
            if let Some(oldest) = self
                .stale
                .iter()
                .min_by_key(|(_, stale)| stale.time)
                .map(|(peer_id, _)| *peer_id)
            {
                self.stale.remove(&oldest);
            }
        }
    }

    /// Forgets the least recently used entries beyond [`Self::MAX_KEYS`]:
    /// first those no longer backing off, then any but those of connections
    /// that are ready.
    fn forget_least_recent(&mut self, now: Timestamp) {
        fn forget<K: Ord + Copy>(map: &mut BTreeMap<K, P2pDialBackoffEntry>, now: Timestamp) {
            while map.len() > P2pDialBackoff::MAX_KEYS {
                let least_recent = |due_only: bool| {
                    map.iter()
                        .filter(|(_, e)| {
                            e.ready_since.is_none() && (!due_only || e.retry_at <= now)
                        })
                        .min_by_key(|(_, e)| e.last_event)
                        .map(|(key, _)| *key)
                };
                let Some(oldest) = least_recent(true).or_else(|| least_recent(false)) else {
                    break;
                };
                map.remove(&oldest);
            }
        }
        forget(&mut self.by_peer, now);
        forget(&mut self.by_addr, now);
    }
}

/// A factor from `1 - JITTER` to `1 + JITTER`, derived from the time, so that
/// the reducer stays deterministic.
fn jitter_factor(now: Timestamp) -> f64 {
    let mut rng = mina_core::pseudo_rng(now);
    rng.gen_range((1.0 - P2pDialBackoff::JITTER)..=(1.0 + P2pDialBackoff::JITTER))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    fn peer(n: u8) -> PeerId {
        PeerId::from_bytes([n; 32])
    }

    fn addr(n: u8) -> Option<SocketAddr> {
        Some(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, n)),
            8302,
        ))
    }

    fn at(secs: f64) -> Timestamp {
        // Any time well after 0, so that `checked_sub` never saturates.
        Timestamp::new(1_000_000_000_000 + (secs * 1e9) as u64)
    }

    fn secs(t: Timestamp) -> f64 {
        (u64::from(t) - 1_000_000_000_000) as f64 / 1e9
    }

    fn config() -> P2pDialBackoffConfig {
        P2pDialBackoffConfig::new(&P2pTimeouts::default(), &P2pLimits::default())
    }

    fn base_config() -> P2pDialBackoffConfig {
        P2pDialBackoffConfig {
            after_disconnect: Duration::from_secs(30),
            after_error: Duration::from_secs(30),
            max: Duration::from_secs(600),
            healthy_after: Duration::from_secs(60),
            stale_for: Duration::from_secs(3600),
            max_redials_per_minute: Limit::Some(20),
            max_dials_per_minute: Limit::Unlimited,
        }
    }

    #[test]
    fn defaults() {
        let config = config();
        assert_eq!(config.after_disconnect, Duration::from_secs(30));
        assert_eq!(config.after_error, Duration::from_secs(30));
        assert_eq!(config.max, Duration::from_secs(600));
        assert_eq!(config.healthy_after, Duration::from_secs(60));
        assert_eq!(config.stale_for, Duration::from_secs(3600));
        assert!(matches!(config.max_redials_per_minute, Limit::Some(20)));
        assert!(matches!(config.max_dials_per_minute, Limit::Some(100)));
    }

    #[test]
    fn delays_double_up_to_the_cap() {
        let base = Duration::from_secs(30);
        let max = Duration::from_secs(600);
        let delays = (0..=8)
            .map(|n| P2pDialBackoff::delay(base, n, max).as_secs())
            .collect::<Vec<_>>();
        assert_eq!(delays, [30, 30, 60, 120, 240, 480, 600, 600, 600]);
        assert_eq!(P2pDialBackoff::delay(base, u32::MAX, max), max);
    }

    #[test]
    fn jitter_stays_within_bounds_and_varies() {
        let factors = (0..1000)
            .map(|n| jitter_factor(at(n as f64 * 0.37)))
            .collect::<Vec<_>>();
        assert!(factors.iter().all(|f| (0.8..=1.2).contains(f)));
        let min = factors.iter().cloned().fold(f64::MAX, f64::min);
        let max = factors.iter().cloned().fold(f64::MIN, f64::max);
        assert!(min < 0.85 && max > 1.15, "{min} to {max}");
        // Deterministic: the same time gives the same factor.
        assert_eq!(jitter_factor(at(5.0)), jitter_factor(at(5.0)));
    }

    /// A peer that accepts us and closes 10 s later, as OCaml's connection
    /// manager does once its grace period is over, is dialled with doubling
    /// waits: 6 to 8 times in 30 minutes, where we used to dial it every 20 s
    /// or so (102 times in the m6 follow).
    #[test]
    fn a_peer_that_keeps_closing_is_dialled_less_and_less() {
        let config = base_config();
        let mut backoff = P2pDialBackoff::default();
        let (p, a) = (peer(1), addr(1));
        let mut t = 0.0;
        let mut dials = Vec::new();
        let mut waits = Vec::new();
        while t < 1800.0 {
            if !backoff.allows(&p, a, at(t), &config) {
                t += 1.0;
                continue;
            }
            dials.push(t);
            backoff.on_dial(p, a, at(t));
            backoff.on_ready(p, false, at(t + 0.5));
            let (end, wait) = backoff.on_disconnected(p, at(t + 10.0), &config);
            assert_eq!(end, P2pDialEnd::Unhealthy);
            waits.push(wait.as_secs_f64());
            t += 10.0;
        }
        // 0, 40, 110, 240, 490, 980 and 1590 s without jitter; 6 to 8 with.
        assert!((6..=8).contains(&dials.len()), "dials at {dials:?}");
        // Each wait is the doubling delay within the jitter.
        for (n, wait) in waits.iter().enumerate() {
            let expected = P2pDialBackoff::delay(config.after_disconnect, n as u32 + 1, config.max)
                .as_secs_f64();
            assert!(
                (expected * 0.8..=expected * 1.2).contains(wait),
                "wait {n} is {wait}, expected about {expected}"
            );
        }
        assert_eq!(backoff.peer(&p).unwrap().failures, dials.len() as u32);
        assert_eq!(
            backoff.addr(&a.unwrap()).unwrap().failures,
            dials.len() as u32
        );
    }

    #[test]
    fn failed_dials_back_off_from_the_error_timeout() {
        let config = P2pDialBackoffConfig {
            after_error: Duration::from_secs(10),
            ..base_config()
        };
        let mut backoff = P2pDialBackoff::default();
        let (p, a) = (peer(1), addr(1));
        assert!(backoff.allows(&p, a, at(0.0), &config));
        backoff.on_dial(p, a, at(0.0));
        let (end, wait) = backoff.on_dial_failed(p, at(5.0), &config);
        assert_eq!(end, P2pDialEnd::Failed);
        assert!((8.0..=12.0).contains(&wait.as_secs_f64()), "{wait:?}");
        assert!(!backoff.allows(&p, a, at(5.0 + 7.9), &config));
        let retry = backoff.retry_at(&p, a).unwrap();
        assert!(backoff.allows(&p, a, retry, &config));

        backoff.on_dial(p, a, retry);
        let (_, wait) = backoff.on_dial_failed(p, retry, &config);
        assert!((16.0..=24.0).contains(&wait.as_secs_f64()), "{wait:?}");
    }

    /// A connection that stays ready for a minute resets the count: after it
    /// ends we wait one base delay again.
    #[test]
    fn a_healthy_connection_resets_the_backoff() {
        let config = base_config();
        let mut backoff = P2pDialBackoff::default();
        let (p, a) = (peer(1), addr(1));
        let mut t = 0.0;
        for _ in 0..5 {
            t = secs(backoff.retry_at(&p, a).unwrap_or(at(t))).max(t);
            backoff.on_dial(p, a, at(t));
            backoff.on_ready(p, false, at(t));
            backoff.on_disconnected(p, at(t + 5.0), &config);
            t += 5.0;
        }
        assert_eq!(backoff.peer(&p).unwrap().failures, 5);

        t = secs(backoff.retry_at(&p, a).unwrap());
        backoff.on_dial(p, a, at(t));
        backoff.on_ready(p, false, at(t + 1.0));
        let (end, wait) = backoff.on_disconnected(p, at(t + 61.0), &config);
        assert_eq!(end, P2pDialEnd::Healthy);
        assert!((24.0..=36.0).contains(&wait.as_secs_f64()), "{wait:?}");
        assert_eq!(backoff.peer(&p).unwrap().failures, 0);
        assert_eq!(backoff.addr(&a.unwrap()).unwrap().failures, 0);

        // The next short connection starts the doubling again from one base.
        t += 61.0 + wait.as_secs_f64();
        backoff.on_dial(p, a, at(t));
        backoff.on_ready(p, false, at(t));
        let (end, wait) = backoff.on_disconnected(p, at(t + 5.0), &config);
        assert_eq!(end, P2pDialEnd::Unhealthy);
        assert!((24.0..=36.0).contains(&wait.as_secs_f64()), "{wait:?}");
        assert_eq!(backoff.peer(&p).unwrap().failures, 1);
    }

    /// An address that answers with a new peer id at every dial (a node that
    /// restarts, or several behind one address) backs off by address, even
    /// though each peer id is new.
    #[test]
    fn an_address_backs_off_whatever_the_peer_id() {
        let config = base_config();
        let mut backoff = P2pDialBackoff::default();
        let a = addr(1);
        backoff.on_dial(peer(1), a, at(0.0));
        backoff.on_dial_failed(peer(1), at(1.0), &config);
        backoff.on_dial(peer(2), a, at(40.0));
        backoff.on_dial_failed(peer(2), at(41.0), &config);
        assert_eq!(backoff.addr(&a.unwrap()).unwrap().failures, 2);
        // A third peer id at that address waits for the address.
        assert!(!backoff.allows(&peer(3), a, at(80.0), &config));
        assert!(backoff.allows(&peer(3), addr(2), at(80.0), &config));
        let retry = backoff.retry_at(&peer(3), a).unwrap();
        assert!(secs(retry) >= 41.0 + 48.0, "{}", secs(retry));
    }

    #[test]
    fn a_stale_peer_id_is_not_dialled_at_its_old_address() {
        let config = base_config();
        let mut backoff = P2pDialBackoff::default();
        let a = addr(1);
        backoff.on_dial(peer(1), a, at(0.0));
        backoff.on_stale(peer(1), a.unwrap(), peer(2), at(0.5));
        backoff.on_dial_failed(peer(1), at(0.5), &config);

        let stale = backoff.stale(&peer(1)).unwrap();
        assert_eq!(stale.answered_by, peer(2));
        assert!(backoff.is_stale(&peer(1), a, at(10.0), &config));
        // Long after the address's backoff, still not at that address...
        assert!(!backoff.allows(&peer(1), a, at(3000.0), &config));
        // ...but at another one, and after `stale_for`, it may be tried.
        assert!(!backoff.is_stale(&peer(1), addr(2), at(10.0), &config));
        assert!(backoff.allows(&peer(1), addr(2), at(3000.0), &config));
        assert!(backoff.allows(&peer(1), a, at(3601.0), &config));
        // The id that answered isn't stale.
        assert!(!backoff.is_stale(&peer(2), a, at(10.0), &config));
    }

    #[test]
    fn redials_are_capped_per_minute() {
        let config = base_config();
        let mut backoff = P2pDialBackoff::default();
        // 30 peers we dialled once, each failed, all due again.
        for n in 0..30 {
            backoff.on_dial(peer(n), addr(n), at(0.0));
            backoff.on_dial_failed(peer(n), at(1.0), &config);
        }
        let mut redialled = Vec::new();
        for second in 100..400 {
            for n in 0..30 {
                let t = at(second as f64);
                if backoff.allows(&peer(n), addr(n), t, &config) {
                    backoff.on_dial(peer(n), addr(n), t);
                    backoff.on_dial_failed(peer(n), t, &config);
                    redialled.push(second);
                }
            }
        }
        for start in 100..400 {
            let in_minute = redialled
                .iter()
                .filter(|s| (start..start + 60).contains(*s))
                .count();
            assert!(
                in_minute <= 20,
                "{in_minute} redials in the minute from {start}"
            );
        }
        assert_eq!(redialled.iter().filter(|s| **s < 160).count(), 20);
    }

    /// The cap holds back redials only: a peer we never dialled is discovery.
    #[test]
    fn the_cap_does_not_hold_back_new_peers() {
        let config = base_config();
        let mut backoff = P2pDialBackoff::default();
        for n in 0..21 {
            backoff.on_dial(peer(n), addr(n), at(0.0));
            backoff.on_dial_failed(peer(n), at(0.0), &config);
        }
        let t = at(100.0);
        for n in 0..20 {
            assert!(backoff.allows(&peer(n), addr(n), t, &config));
            backoff.on_dial(peer(n), addr(n), t);
        }
        assert_eq!(backoff.redials_in_window(t), 20);
        assert!(!backoff.allows(&peer(20), addr(20), t, &config));
        assert!(backoff.allows(&peer(100), addr(100), t, &config));
        // The same peer id at a new address is still a redial.
        assert!(!backoff.allows(&peer(20), addr(120), t, &config));
        assert!(backoff.allows(&peer(20), addr(20), at(160.0), &config));
    }

    /// First dials to new peers don't count against the redial cap, but all
    /// dials together are held to the overall cap (I18b).
    #[test]
    fn all_dials_are_capped_per_minute_first_dials_too() {
        let config = P2pDialBackoffConfig {
            max_dials_per_minute: Limit::Some(30),
            ..base_config()
        };
        let mut backoff = P2pDialBackoff::default();
        let t = at(0.0);
        // 25 first dials: none of them is a redial.
        for n in 0..25 {
            assert!(backoff.allows(&peer(n), addr(n), t, &config));
            backoff.on_dial(peer(n), addr(n), t);
        }
        assert_eq!(backoff.redials_in_window(t), 0);
        assert_eq!(backoff.dials_in_window(t), 25);
        // Five more, then the overall cap holds back a new peer as well.
        for n in 25..30 {
            assert!(backoff.allows(&peer(n), addr(n), t, &config));
            backoff.on_dial(peer(n), addr(n), t);
        }
        assert!(!backoff.allows(&peer(100), addr(100), t, &config));
        assert!(!backoff.allows(&peer(100), addr(100), at(59.9), &config));
        assert!(backoff.allows(&peer(100), addr(100), at(60.0), &config));
        // A recovery probe is held to it too.
        backoff.on_dial_failed(peer(1), at(1.0), &config);
        assert!(!backoff.allows_recovery(&peer(1), addr(1), at(20.0), &config));
    }

    #[test]
    fn the_default_overall_cap_does_not_hold_back_m8s_busiest_minute() {
        let config = config();
        let mut backoff = P2pDialBackoff::default();
        // m8's bootstrap: 74 first dials in its first 16 s, then 20 redials.
        for n in 0..74 {
            let t = at(f64::from(n) * 0.2);
            assert!(backoff.allows(&peer(n), addr(n), t, &config));
            backoff.on_dial(peer(n), addr(n), t);
            backoff.on_dial_failed(peer(n), t, &config);
        }
        let mut redials = 0;
        for n in 0..74 {
            let t = at(45.0);
            if backoff.allows(&peer(n), addr(n), t, &config) {
                backoff.on_dial(peer(n), addr(n), t);
                redials += 1;
            }
        }
        assert_eq!(redials, 20);
        assert_eq!(backoff.dials_in_window(at(45.0)), 94);
        // Six more new peers fit in that minute, the seventh doesn't.
        for n in 100..106 {
            assert!(backoff.allows(&peer(n), addr(n), at(45.0), &config));
            backoff.on_dial(peer(n), addr(n), at(45.0));
        }
        assert!(!backoff.allows(&peer(106), addr(106), at(45.0), &config));
    }

    #[test]
    fn an_unlimited_cap_allows_every_due_redial() {
        let config = P2pDialBackoffConfig {
            max_redials_per_minute: Limit::Unlimited,
            ..base_config()
        };
        let mut backoff = P2pDialBackoff::default();
        for n in 0..50 {
            backoff.on_dial(peer(n), addr(n), at(0.0));
            backoff.on_dial_failed(peer(n), at(0.0), &config);
        }
        let t = at(100.0);
        for n in 0..50 {
            assert!(backoff.allows(&peer(n), addr(n), t, &config));
            backoff.on_dial(peer(n), addr(n), t);
        }
        assert_eq!(backoff.redials_in_window(t), 50);
    }

    /// An incoming connection doesn't tie the peer to an address we dialled.
    #[test]
    fn an_incoming_connection_backs_off_the_peer_only() {
        let config = base_config();
        let mut backoff = P2pDialBackoff::default();
        let (p, a) = (peer(1), addr(1));
        backoff.on_dial(p, a, at(0.0));
        backoff.on_dial_failed(p, at(1.0), &config);
        backoff.on_ready(p, true, at(50.0));
        backoff.on_disconnected(p, at(55.0), &config);
        assert_eq!(backoff.peer(&p).unwrap().failures, 2);
        assert_eq!(backoff.addr(&a.unwrap()).unwrap().failures, 1);
    }

    #[test]
    fn remembers_a_bounded_number_of_peers_and_addresses() {
        let config = base_config();
        let mut backoff = P2pDialBackoff::default();
        let max = P2pDialBackoff::MAX_KEYS;
        for n in 0..(max as u32 + 100) {
            let mut id = [0u8; 32];
            id[..4].copy_from_slice(&n.to_be_bytes());
            let a = SocketAddr::new(IpAddr::V4(Ipv4Addr::from(n)), 8302);
            let t = at(n as f64);
            backoff.on_dial(PeerId::from_bytes(id), Some(a), t);
            backoff.on_dial_failed(PeerId::from_bytes(id), t, &config);
            backoff.on_stale(PeerId::from_bytes(id), a, peer(0), t);
        }
        assert!(backoff.by_peer.len() <= max);
        assert!(backoff.by_addr.len() <= max);
        assert!(backoff.stale.len() <= max);
        assert!(backoff.redials.len() <= max);
        assert!(backoff.dials.len() <= max);
    }
}
