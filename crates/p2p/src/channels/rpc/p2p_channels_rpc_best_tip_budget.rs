use std::{
    collections::{BTreeMap, VecDeque},
    net::IpAddr,
    time::Duration,
};

use redux::Timestamp;
use serde::{Deserialize, Serialize};

use crate::PeerId;

/// How often we ask libp2p peers, OCaml nodes, for their best tip.
///
/// OCaml rate-limits `get_best_tip` by the sender's peer id and by its IP: 3 a
/// minute, kept as a budget of 15 over a sliding 5 minutes. It answers a
/// request over either budget with an exception, which we used to answer by
/// disconnecting, and every reconnection asked again. All nodes behind one IP
/// share that IP's budget.
///
/// Both of our request sites go through this budget: the request on each new
/// connection and the request for a candidate's chain proof. A request is
/// allowed if our last one to the same peer id, and our last one to the same
/// IP, were at least `min_interval` ago (20 s by default, so at most 3 a
/// minute), and if fewer than [`Self::MAX_PER_WINDOW`] went to either in the
/// last 5 minutes. That is below OCaml's 15, to leave room for differences in
/// latency between requests and for other nodes on our IP. The record is kept
/// here rather than with the peer, so that it outlives the connection, and
/// the peer's entry in the peer table.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/rpcs.ml#L1173>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/rate_limiter.ml#L9-L10>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/rate_limiter.ml#L127-L150>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/gossip_net/libp2p.ml#L159-L165>
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct P2pBestTipRequestBudget {
    by_peer: BTreeMap<PeerId, VecDeque<Timestamp>>,
    by_ip: BTreeMap<IpAddr, VecDeque<Timestamp>>,
}

impl P2pBestTipRequestBudget {
    /// OCaml's rate limiting window.
    pub const WINDOW: Duration = Duration::from_secs(5 * 60);
    /// Requests to one peer id, or to one IP, in any [`Self::WINDOW`].
    pub const MAX_PER_WINDOW: usize = 12;
    /// Peer ids and IPs remembered at most, each; those asked longest ago are
    /// forgotten first. OCaml's own tables keep 2048.
    const MAX_KEYS: usize = 1024;

    /// Whether a best tip request to `peer_id`, at `ip` if known, is within
    /// the budget at `now`.
    pub fn allows(
        &self,
        peer_id: &PeerId,
        ip: Option<IpAddr>,
        now: Timestamp,
        min_interval: Duration,
    ) -> bool {
        let within = |times: Option<&VecDeque<Timestamp>>| {
            let Some(times) = times else {
                return true;
            };
            let since_last_ok = times
                .back()
                .is_none_or(|last| now.checked_sub(*last).unwrap_or_default() >= min_interval);
            let in_window = times
                .iter()
                .filter(|time| now.checked_sub(**time).unwrap_or_default() < Self::WINDOW)
                .count();
            since_last_ok && in_window < Self::MAX_PER_WINDOW
        };
        within(self.by_peer.get(peer_id)) && ip.is_none_or(|ip| within(self.by_ip.get(&ip)))
    }

    /// Records a best tip request to `peer_id`, at `ip` if known, sent at
    /// `now`.
    pub fn record(&mut self, peer_id: PeerId, ip: Option<IpAddr>, now: Timestamp) {
        Self::record_in(&mut self.by_peer, peer_id, now);
        if let Some(ip) = ip {
            Self::record_in(&mut self.by_ip, ip, now);
        }
    }

    /// When we last asked `peer_id`, if we remember.
    pub fn last_request(&self, peer_id: &PeerId) -> Option<Timestamp> {
        self.by_peer.get(peer_id)?.back().copied()
    }

    /// Requests recorded for `peer_id` in the window that ends at `now`.
    pub fn peer_requests_in_window(&self, peer_id: &PeerId, now: Timestamp) -> usize {
        self.by_peer.get(peer_id).map_or(0, |times| {
            times
                .iter()
                .filter(|time| now.checked_sub(**time).unwrap_or_default() < Self::WINDOW)
                .count()
        })
    }

    fn record_in<K: Ord + Copy>(
        map: &mut BTreeMap<K, VecDeque<Timestamp>>,
        key: K,
        now: Timestamp,
    ) {
        let times = map.entry(key).or_default();
        while times
            .front()
            .is_some_and(|time| now.checked_sub(*time).unwrap_or_default() >= Self::WINDOW)
        {
            times.pop_front();
        }
        times.push_back(now);

        if map.len() > Self::MAX_KEYS {
            // Forget the keys asked longest ago, and any whose requests are
            // all outside the window.
            map.retain(|_, times| {
                times
                    .back()
                    .is_some_and(|last| now.checked_sub(*last).unwrap_or_default() < Self::WINDOW)
            });
            while map.len() > Self::MAX_KEYS {
                let Some(oldest) = map
                    .iter()
                    .min_by_key(|(_, times)| times.back().copied())
                    .map(|(key, _)| *key)
                else {
                    break;
                };
                map.remove(&oldest);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    const MIN_INTERVAL: Duration = Duration::from_secs(20);

    fn peer(n: u8) -> PeerId {
        PeerId::from_bytes([n; 32])
    }

    fn at(secs: u64) -> Timestamp {
        // Any time well after 0, so that `checked_sub` never saturates.
        Timestamp::new(1_000_000_000_000 + secs * 1_000_000_000)
    }

    fn ip(n: u8) -> Option<IpAddr> {
        Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, n)))
    }

    /// Sends a request whenever the budget allows, every second from `from`
    /// to `to`, and returns the times sent.
    fn ask_whenever_allowed(
        budget: &mut P2pBestTipRequestBudget,
        peer_id: PeerId,
        ip: Option<IpAddr>,
        from: u64,
        to: u64,
    ) -> Vec<u64> {
        (from..to)
            .filter(|t| {
                let allowed = budget.allows(&peer_id, ip, at(*t), MIN_INTERVAL);
                if allowed {
                    budget.record(peer_id, ip, at(*t));
                }
                allowed
            })
            .collect()
    }

    #[test]
    fn at_most_three_a_minute_and_twelve_in_five_minutes() {
        let mut budget = P2pBestTipRequestBudget::default();
        let sent = ask_whenever_allowed(&mut budget, peer(1), ip(1), 0, 3600);

        for window_start in 0..3600 {
            let in_minute = sent
                .iter()
                .filter(|t| (window_start..window_start + 60).contains(*t))
                .count();
            assert!(
                in_minute <= 3,
                "{in_minute} in the minute from {window_start}"
            );
            let in_window = sent
                .iter()
                .filter(|t| (window_start..window_start + 300).contains(*t))
                .count();
            assert!(
                in_window <= P2pBestTipRequestBudget::MAX_PER_WINDOW,
                "{in_window} in the 5 minutes from {window_start}"
            );
        }
        // It still asks steadily, about 12 every 5 minutes.
        assert!(sent.len() >= 12 * 11, "only {} in an hour", sent.len());
        assert_eq!(&sent[..3], &[0, 20, 40]);
    }

    /// The record is by peer id, not by connection: a peer that reconnects
    /// every few seconds is asked no more often.
    #[test]
    fn reconnects_do_not_reset_the_budget() {
        let mut budget = P2pBestTipRequestBudget::default();
        let mut sent = Vec::new();
        // A new connection every 5 s, each of which would ask once.
        for t in (0..600).step_by(5) {
            if budget.allows(&peer(1), ip(1), at(t), MIN_INTERVAL) {
                budget.record(peer(1), ip(1), at(t));
                sent.push(t);
            }
        }
        assert_eq!(&sent[..4], &[0, 20, 40, 60]);
        assert!(sent.len() <= 2 * P2pBestTipRequestBudget::MAX_PER_WINDOW);
    }

    /// A node that comes back with a new peer id on the same IP shares the
    /// IP's budget.
    #[test]
    fn peers_on_one_ip_share_its_budget() {
        let mut budget = P2pBestTipRequestBudget::default();
        budget.record(peer(1), ip(1), at(0));
        assert!(!budget.allows(&peer(2), ip(1), at(10), MIN_INTERVAL));
        assert!(budget.allows(&peer(2), ip(2), at(10), MIN_INTERVAL));
        assert!(budget.allows(&peer(2), None, at(10), MIN_INTERVAL));
        assert!(budget.allows(&peer(2), ip(1), at(20), MIN_INTERVAL));
        // The same peer id on another IP doesn't get a fresh budget.
        assert!(!budget.allows(&peer(1), ip(3), at(10), MIN_INTERVAL));
    }

    #[test]
    fn requests_age_out_of_the_window() {
        let mut budget = P2pBestTipRequestBudget::default();
        let sent = ask_whenever_allowed(&mut budget, peer(1), None, 0, 300);
        assert_eq!(sent.len(), P2pBestTipRequestBudget::MAX_PER_WINDOW);
        assert_eq!(budget.peer_requests_in_window(&peer(1), at(299)), 12);
        assert!(!budget.allows(&peer(1), None, at(299), MIN_INTERVAL));
        assert!(budget.allows(&peer(1), None, at(300), MIN_INTERVAL));
        assert_eq!(budget.peer_requests_in_window(&peer(1), at(300)), 11);
    }

    #[test]
    fn remembers_a_bounded_number_of_peers() {
        let mut budget = P2pBestTipRequestBudget::default();
        let max = P2pBestTipRequestBudget::MAX_KEYS;
        for n in 0..(max as u64 + 100) {
            let mut id = [0u8; 32];
            id[..8].copy_from_slice(&n.to_le_bytes());
            budget.record(PeerId::from_bytes(id), None, at(n / 10));
        }
        assert_eq!(budget.by_peer.len(), max);
        // The most recent are kept.
        let mut last = [0u8; 32];
        last[..8].copy_from_slice(&(max as u64 + 99).to_le_bytes());
        assert!(!budget.allows(&PeerId::from_bytes(last), None, at(115), MIN_INTERVAL));
    }
}
