//! OCaml's inbound RPC limits. These records outlive connections.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/rate_limiter.ml#L44-L150>

use super::{CatchupRequest, P2pRpcRequest};
use crate::PeerId;
use mina_p2p_messages::v2::{
    MerkleAddressBinableArgStableV1, MinaLedgerSyncLedgerQueryStableV1 as Query,
};
use redux::Timestamp;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    net::IpAddr,
    time::Duration,
};

pub const MAX_SERVED_ACCOUNT_SUBTREE_HEIGHT: usize = 6;
pub const MAX_SERVED_CHILD_HASHES_DEPTH: usize = 8;
/// Defense in depth above the maximum 64 bounded account records. Does not
/// change the limits on responses to our own outgoing sync requests.
pub const MAX_SERVED_LEDGER_ANSWER_BYTES: usize = 1024 * 1024;
pub const CONTENTS_TOO_LARGE: &str = "Requested too big of a subtree at once";
pub const CONTENTS_EMPTY: &str = "Requested empty subtree";
pub const INVALID_CHILD_DEPTH: &str = "Invalid depth requested in What_child_hashes request";
pub const INVALID_CHILD_ADDRESS: &str = "Invalid address in What_child_hashes request";

/// Validate before converting the wire address (whose length is a u64) or
/// scheduling ledger work. OCaml bounds contents at 2^6 and hashes at 2^8.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_ledger/sync_ledger.ml#L25-L57>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/syncable_ledger/syncable_ledger.ml#L321-L430>
pub fn validate_ledger_query(query: &Query, ledger_depth: usize) -> Result<(), &'static str> {
    let (addr, contents) = match query {
        Query::NumAccounts => return Ok(()),
        Query::WhatContents(addr) => (addr, true),
        Query::WhatChildHashes(addr, depth) => {
            if depth.as_u64() == 0 || depth.as_u64() > (1u64 << 62) - 1 {
                return Err(INVALID_CHILD_DEPTH);
            }
            (addr, false)
        }
    };
    let MerkleAddressBinableArgStableV1(length, bytes) = addr;
    let length = usize::try_from(length.as_u64()).map_err(|_| INVALID_CHILD_ADDRESS)?;
    let height = ledger_depth
        .checked_sub(length)
        .ok_or(INVALID_CHILD_ADDRESS)?;
    if bytes.len() != length.div_ceil(8) {
        return Err(INVALID_CHILD_ADDRESS);
    }
    if contents && height > MAX_SERVED_ACCOUNT_SUBTREE_HEIGHT {
        return Err(CONTENTS_TOO_LARGE);
    }
    if let Query::WhatChildHashes(_, depth) = query {
        // OCaml clamps to max_subtree_depth, then Addr.extend_exn rejects
        // addresses past the leaves; it does not clamp to remaining height.
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/merkle_address/merkle_address.ml#L136-L146>
        if depth.as_u64().min(MAX_SERVED_CHILD_HASHES_DEPTH as u64) > height as u64 {
            return Err(INVALID_CHILD_ADDRESS);
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ServedRpc {
    InitialPeers,
    BestTip,
    Ancestry,
    LedgerQuery,
    StagedLedger,
    CompletedSnarks,
    TransitionKnowledge,
    TransitionChain,
    TransitionChainProof,
}
impl ServedRpc {
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/rpcs.ml#L200-L1175>
    pub fn capacity(self) -> u32 {
        match self {
            Self::InitialPeers | Self::TransitionKnowledge => 5,
            Self::BestTip | Self::TransitionChainProof => 15,
            Self::Ancestry => 25,
            Self::LedgerQuery => 655_360,
            Self::StagedLedger => 20,
            // OCaml rpcs.ml:838: one request per minute.
            Self::CompletedSnarks => 5,
            Self::TransitionChain => 300,
        }
    }
    pub fn for_request(request: &P2pRpcRequest) -> Option<(Self, u32)> {
        Some(match request {
            P2pRpcRequest::BestTipWithProof => (Self::BestTip, 1),
            P2pRpcRequest::LedgerQuery(..) => (Self::LedgerQuery, 1),
            P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(_) => (Self::StagedLedger, 1),
            P2pRpcRequest::Block(_) => (Self::TransitionChain, 1),
            P2pRpcRequest::InitialPeers => (Self::InitialPeers, 1),
            P2pRpcRequest::Catchup(req) => match req {
                CatchupRequest::Ancestry(_) => (Self::Ancestry, 1),
                CatchupRequest::TransitionChain(hashes) => (
                    Self::TransitionChain,
                    u32::try_from(hashes.len()).unwrap_or(u32::MAX).max(1),
                ),
                CatchupRequest::TransitionChainProof(_) => (Self::TransitionChainProof, 1),
                CatchupRequest::TransitionKnowledge => (Self::TransitionKnowledge, 1),
            },
            // Native object fetches have no OCaml RPC counterpart.
            P2pRpcRequest::Snark(_) | P2pRpcRequest::Transaction(_) => return None,
            P2pRpcRequest::CompletedSnarks => (Self::CompletedSnarks, 1),
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ServingLimits {
    methods: BTreeMap<ServedRpc, MethodBudget>,
}
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
struct MethodBudget {
    peers: BudgetTable<PeerId>,
    ips: BudgetTable<IpAddr>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
struct BudgetTable<K: Ord> {
    records: BTreeMap<K, Record>,
    lru: VecDeque<K>,
}
impl<K: Ord> Default for BudgetTable<K> {
    fn default() -> Self {
        Self {
            records: BTreeMap::new(),
            lru: VecDeque::new(),
        }
    }
}
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
struct Record {
    used: u32,
    entries: VecDeque<(Timestamp, u32)>,
}
const WINDOW: Duration = Duration::from_secs(300);
const MAX_KEYS: usize = 2048;
impl Record {
    fn allows(&mut self, now: Timestamp, cost: u32, capacity: u32) -> bool {
        while self
            .entries
            .front()
            .is_some_and(|(time, _)| now.checked_sub(*time).unwrap_or_default() > WINDOW)
        {
            let (_, cost) = self.entries.pop_front().expect("checked above");
            self.used -= cost;
        }
        self.used
            .checked_add(cost)
            .is_some_and(|total| total <= capacity)
    }
    fn charge(&mut self, now: Timestamp, cost: u32) {
        self.used += cost; // checked by allows, for both attributes, first
        if let Some((time, previous)) = self.entries.back_mut().filter(|(time, _)| *time == now) {
            let _ = time;
            *previous += cost;
        } else {
            self.entries.push_back((now, cost));
        }
    }
}
impl<K: Ord + Copy> BudgetTable<K> {
    fn allows(&mut self, key: K, now: Timestamp, cost: u32, capacity: u32) -> bool {
        let Some(record) = self.records.get_mut(&key) else {
            return cost <= capacity;
        };
        self.lru.retain(|k| *k != key);
        self.lru.push_back(key);
        record.allows(now, cost, capacity)
    }
    fn charge(&mut self, key: K, now: Timestamp, cost: u32) {
        if let Some(record) = self.records.get_mut(&key) {
            record.charge(now, cost);
            return;
        }
        if self.records.len() == MAX_KEYS {
            if let Some(old) = self.lru.pop_front() {
                self.records.remove(&old);
            }
        }
        self.lru.push_back(key);
        let mut record = Record::default();
        record.charge(now, cost);
        self.records.insert(key, record);
    }
}
impl ServingLimits {
    /// Admit a libp2p wire query using its observed socket IP. WebRTC must not
    /// call this: its request cadence and pending-request caps are independent.
    pub fn admit(
        &mut self,
        method: ServedRpc,
        peer: PeerId,
        ip: IpAddr,
        now: Timestamp,
        cost: u32,
    ) -> bool {
        let budget = self.methods.entry(method).or_default();
        let capacity = method.capacity();
        if !budget.ips.allows(ip, now, cost, capacity)
            || !budget.peers.allows(peer, now, cost, capacity)
        {
            return false;
        }
        budget.ips.charge(ip, now, cost);
        budget.peers.charge(peer, now, cost);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn peer(n: u8) -> PeerId {
        PeerId::from_bytes([n; 32])
    }
    fn ip(n: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, n])
    }
    fn at(s: u64) -> Timestamp {
        Timestamp::new(1_000_000_000_000 + s * 1_000_000_000)
    }
    #[test]
    fn all_ocaml_budgets_at_over_and_strict_window_expiry() {
        // rpcs.ml budgets scaled onto rate_limiter.ml's five-minute window.
        for (method, capacity) in [
            (ServedRpc::InitialPeers, 5),
            (ServedRpc::BestTip, 15),
            (ServedRpc::Ancestry, 25),
            (ServedRpc::LedgerQuery, 655_360),
            (ServedRpc::StagedLedger, 20),
            (ServedRpc::CompletedSnarks, 5),
            (ServedRpc::TransitionKnowledge, 5),
            (ServedRpc::TransitionChain, 300),
            (ServedRpc::TransitionChainProof, 15),
        ] {
            assert_eq!(method.capacity(), capacity);
            let mut limits = ServingLimits::default();
            assert!(limits.admit(method, peer(1), ip(1), at(0), 1));
            assert!(limits.admit(method, peer(1), ip(1), at(0), capacity - 1));
            assert!(!limits.admit(method, peer(1), ip(1), at(1), 1));
            assert!(!limits.admit(method, peer(1), ip(1), at(300), 1)); // strictly >300
            assert!(limits.admit(method, peer(1), ip(1), at(301), capacity - 1));
            assert!(limits.admit(method, peer(1), ip(1), at(301), 1));
            assert!(!limits.admit(method, peer(1), ip(1), at(302), 1));
        }
    }
    #[test]
    fn peer_and_ip_are_independent_and_rejection_does_not_charge() {
        let mut limits = ServingLimits::default();
        let m = ServedRpc::BestTip;
        assert!(limits.admit(m, peer(1), ip(1), at(0), 1));
        assert!(limits.admit(m, peer(1), ip(1), at(0), 14));
        assert!(!limits.admit(m, peer(2), ip(1), at(0), 1)); // fresh identity, same IP
        assert!(!limits.admit(m, peer(1), ip(2), at(0), 1)); // same identity, fresh IP
        assert!(limits.admit(m, peer(2), ip(2), at(0), 1)); // both fresh; previous denial charged neither
        assert!(limits.admit(m, peer(2), ip(2), at(0), 14));
        assert!(!limits.admit(m, peer(2), ip(2), at(0), 1));
        assert!(limits.admit(ServedRpc::Ancestry, peer(1), ip(1), at(0), 1));
        // Serde/reconnect cannot reset a still-live budget.
        let json = serde_json::to_string(&limits).unwrap();
        let mut limits: ServingLimits = serde_json::from_str(&json).unwrap();
        assert!(!limits.admit(m, peer(1), ip(1), at(1), 1));
    }
    #[test]
    fn chain_cost_and_new_attribute_are_charged() {
        let mut limits = ServingLimits::default();
        let m = ServedRpc::TransitionChain;
        assert!(!limits.admit(m, peer(1), ip(1), at(0), u32::MAX));
        assert!(!limits.admit(m, peer(1), ip(1), at(0), u32::MAX));
        for _ in 0..15 {
            assert!(limits.admit(m, peer(1), ip(1), at(0), 20));
        }
        assert!(!limits.admit(m, peer(1), ip(1), at(0), 1));
        let empty = P2pRpcRequest::Catchup(CatchupRequest::TransitionChain(Default::default()));
        assert_eq!(ServedRpc::for_request(&empty), Some((m, 1)));
    }
    #[test]
    fn tables_are_lru_bounded() {
        let mut table: BudgetTable<u32> = Default::default();
        for n in 0..2048 {
            assert!(table.allows(n, at(0), 1, 5));
            table.charge(n, at(0), 1);
        }
        assert!(table.allows(0, at(0), 1, 5));
        table.charge(2048, at(0), 1);
        assert_eq!(table.records.len(), 2048);
        assert!(table.records.contains_key(&0));
        assert!(!table.records.contains_key(&1));
        assert_eq!(table.lru.len(), 2048);
    }
    #[test]
    fn contents_depth_and_malformed_address_boundaries() {
        let addr = |length: u64| {
            MerkleAddressBinableArgStableV1(
                length.into(),
                vec![0; length.div_ceil(8) as usize].into(),
            )
        };
        assert_eq!(
            validate_ledger_query(&Query::WhatContents(addr(29)), 35),
            Ok(())
        ); // 6 levels
        for length in [28, 3, 0] {
            // 7 levels, old 2_u32.pow(32) overflow, all 35 levels
            assert_eq!(
                validate_ledger_query(&Query::WhatContents(addr(length)), 35),
                Err(CONTENTS_TOO_LARGE)
            );
        }
        assert!(validate_ledger_query(&Query::WhatContents(addr(36)), 35).is_err());
        assert!(validate_ledger_query(
            &Query::WhatContents(MerkleAddressBinableArgStableV1(
                u64::MAX.into(),
                vec![0].into()
            )),
            35
        )
        .is_err());
        assert_eq!(
            validate_ledger_query(&Query::WhatChildHashes(addr(0), 0.into()), 35),
            Err(INVALID_CHILD_DEPTH)
        );
        assert_eq!(
            validate_ledger_query(&Query::WhatChildHashes(addr(35), 1.into()), 35),
            Err(INVALID_CHILD_ADDRESS)
        );
        assert_eq!(
            validate_ledger_query(&Query::WhatChildHashes(addr(0), u64::MAX.into()), 35),
            Err(INVALID_CHILD_DEPTH)
        );
        for (length, depth, expected) in [
            (34, 1u64, Ok(())),
            (34, 8, Err(INVALID_CHILD_ADDRESS)),
            (27, 1000, Ok(())),
            (28, 1000, Err(INVALID_CHILD_ADDRESS)),
        ] {
            assert_eq!(
                validate_ledger_query(&Query::WhatChildHashes(addr(length), depth.into()), 35),
                expected
            );
        }
        assert!(
            validate_ledger_query(&Query::WhatChildHashes(addr(0), 1000u64.into()), 35).is_ok()
        );
    }
}
