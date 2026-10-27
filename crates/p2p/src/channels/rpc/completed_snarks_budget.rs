use std::{collections::BTreeMap, net::IpAddr, time::Duration};

use redux::Timestamp;
use serde::{Deserialize, Serialize};

use super::P2pBestTipRequestBudget;
use crate::PeerId;

/// OCaml allows 5 requests per rolling 5 minutes, independently by sender peer
/// and IP. Leave room for the other nodes on this machine: one per destination
/// peer/IP per 5 minutes, at most two destination peers per window, and 75 s
/// between requests globally. Like I17, this survives peer removal/reconnects.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/rpcs.ml#L836-L838>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/rate_limiter.ml#L127-L150>
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CompletedSnarksBudget {
    history: P2pBestTipRequestBudget,
    recent_peers: BTreeMap<PeerId, Timestamp>,
    last: Option<Timestamp>,
    pub sent: u64,
}

impl CompletedSnarksBudget {
    pub const PEER_INTERVAL: Duration = Duration::from_secs(300);
    pub const GLOBAL_INTERVAL: Duration = Duration::from_secs(75);
    pub const MAX_PEERS: usize = 2;

    pub fn allows(&self, peer: &PeerId, ip: Option<IpAddr>, now: Timestamp) -> bool {
        self.last
            .is_none_or(|last| now.checked_sub(last).unwrap_or_default() >= Self::GLOBAL_INTERVAL)
            && self.history.allows(peer, ip, now, Self::PEER_INTERVAL)
            && self
                .recent_peers
                .values()
                .filter(|last| now.checked_sub(**last).unwrap_or_default() < Self::PEER_INTERVAL)
                .count()
                < Self::MAX_PEERS
    }

    pub fn record(&mut self, peer: PeerId, ip: Option<IpAddr>, now: Timestamp) {
        self.history.record(peer, ip, now);
        self.recent_peers
            .retain(|_, last| now.checked_sub(*last).unwrap_or_default() < Self::PEER_INTERVAL);
        self.recent_peers.insert(peer, now);
        self.last = Some(now);
        self.sent = self.sent.saturating_add(1);
    }

    pub fn last_request(&self, peer: &PeerId) -> Option<Timestamp> {
        self.history.last_request(peer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(s: u64) -> Timestamp {
        Timestamp::new((s + 1000) * 1_000_000_000)
    }
    #[test]
    fn global_peer_ip_and_fanout_limits_survive_reconnects() {
        let mut budget = CompletedSnarksBudget::default();
        let a = PeerId::from_bytes([1; 32]);
        let b = PeerId::from_bytes([2; 32]);
        let c = PeerId::from_bytes([3; 32]);
        let ip = Some("192.0.2.1".parse().unwrap());
        assert!(budget.allows(&a, ip, at(0)));
        budget.record(a, ip, at(0));
        assert!(!budget.allows(&b, None, at(74)));
        assert!(!budget.allows(&b, ip, at(75)));
        assert!(!budget.allows(&a, None, at(75)));
        assert!(budget.allows(&b, None, at(75)));
        budget.record(b, None, at(75));
        assert!(!budget.allows(&c, None, at(150)));
        assert!(!budget.allows(&a, ip, at(299)));
        // No connection or peer-state lifetime can reset this state.
        let restored: CompletedSnarksBudget =
            serde_json::from_str(&serde_json::to_string(&budget).unwrap()).unwrap();
        assert!(!restored.allows(&a, ip, at(299)));
        assert!(restored.allows(&a, ip, at(300)));
        assert!(!restored.allows(&a, ip, at(0)));
    }
}
