use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use mina_core::{
    block::{ArcBlockWithHash, BlockWithHash},
    impl_substate_access,
    requests::RpcId,
    snark::{Snark, SnarkInfo, SnarkJobCommitment},
    transaction::{TransactionInfo, TransactionWithHash},
    ChainId, SubstateAccess,
};

use malloc_size_of_derive::MallocSizeOf;
use redux::{Callback, Timestamp};
use serde::{Deserialize, Serialize};

use crate::{
    bootstrap::P2pNetworkKadBootstrapState,
    channels::{
        rpc::{P2pBestTipRequestBudget, P2pRpcId, P2pRpcRequest, P2pRpcResponse},
        streaming_rpc::{P2pStreamingRpcId, P2pStreamingRpcResponseFull},
        ChannelId, P2pChannelsState,
    },
    connection::{
        incoming::P2pConnectionIncomingState,
        outgoing::{
            P2pConnectionOutgoingError, P2pConnectionOutgoingInitOpts, P2pConnectionOutgoingState,
            P2pDialBackoff, P2pDialBackoffConfig,
        },
        P2pConnectionResponse, P2pConnectionState,
    },
    is_time_passed,
    network::{
        identify::{P2pNetworkIdentify, P2pNetworkIdentifyState},
        P2pNetworkState,
    },
    Limit, P2pConfig, P2pLimits, P2pNetworkKadState, P2pNetworkPubsubMessageCacheId,
    P2pNetworkPubsubState, P2pNetworkSchedulerState, P2pTimeouts, PeerId,
};
use mina_p2p_messages::v2;

/// An address is proven only after an outgoing connection reaches Ready.
/// Rank is used among unproven addresses, never to evict a proven one.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pDialAddress {
    pub opts: P2pConnectionOutgoingInitOpts,
    #[serde(default)]
    pub rank: (u8, u8),
    pub last_success: Option<Timestamp>,
    pub failures: u32,
    pub retry_at: Timestamp,
}

impl P2pDialAddress {
    const MAX_PER_PEER: usize = 8;

    fn new(opts: P2pConnectionOutgoingInitOpts, observed: Option<std::net::IpAddr>) -> Self {
        let rank = match &opts {
            P2pConnectionOutgoingInitOpts::LibP2P(opts) => opts.dial_rank(observed),
            _ => (0, 0),
        };
        Self {
            opts,
            rank,
            last_success: None,
            failures: 0,
            retry_at: Timestamp::ZERO,
        }
    }

    fn failed(&mut self, now: Timestamp, base: Duration) {
        self.failures = self.failures.saturating_add(1);
        let multiplier = 1_u32 << self.failures.saturating_sub(1).min(5);
        self.retry_at = now
            + base
                .saturating_mul(multiplier)
                .min(Duration::from_secs(600));
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pState {
    pub chain_id: ChainId,
    pub config: P2pConfig,
    pub network: P2pNetworkState,
    pub peers: BTreeMap<PeerId, P2pPeerState>,
    /// Advertised, discovered and previously successful libp2p dial targets.
    #[serde(default)]
    pub dial_addresses: BTreeMap<PeerId, Vec<P2pDialAddress>>,

    pub last_random_disconnection_try: redux::Timestamp,

    /// Our best tip requests to libp2p peers, kept within OCaml's rate limit.
    #[serde(default)]
    pub best_tip_requests: P2pBestTipRequestBudget,
    #[serde(default)]
    pub completed_snarks_requests: crate::channels::rpc::CompletedSnarksBudget,

    /// When we may dial each libp2p peer and address again.
    #[serde(default)]
    pub dial_backoff: P2pDialBackoff,

    pub callbacks: P2pCallbacks,
}

impl P2pState {
    pub fn new(config: P2pConfig, callbacks: P2pCallbacks, chain_id: &ChainId) -> Self {
        let addrs = if cfg!(feature = "p2p-libp2p") {
            config
                .libp2p_port
                .map(|port| multiaddr::multiaddr!(Ip4([127, 0, 0, 1]), Tcp((port))))
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };

        let my_id = config.identity_pub_key.peer_id();

        #[cfg(not(target_arch = "wasm32"))]
        {
            let peer_id_str = my_id.to_libp2p_string();

            mina_core::log::info!(
                mina_core::log::system_time();
                kind = "P2pState new",
                summary = format!("Current node's id: {my_id}"),
                peer_id_str = peer_id_str,
            );
        }

        let known_peers = if cfg!(feature = "p2p-libp2p") {
            config
                .initial_peers
                .iter()
                .filter_map(|peer| {
                    if let P2pConnectionOutgoingInitOpts::LibP2P(peer) = peer {
                        Some(peer.into())
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        let network = P2pNetworkState::new(
            config.identity_pub_key.clone(),
            addrs,
            known_peers,
            chain_id,
            config.peer_discovery,
        );
        Self {
            chain_id: chain_id.clone(),
            config,
            network,
            peers: Default::default(),
            dial_addresses: Default::default(),

            last_random_disconnection_try: redux::Timestamp::ZERO,

            best_tip_requests: Default::default(),
            completed_snarks_requests: Default::default(),

            dial_backoff: Default::default(),

            callbacks,
        }
    }

    pub fn my_id(&self) -> PeerId {
        self.config.identity_pub_key.peer_id()
    }

    pub fn peer_connection_rpc_id(&self, peer_id: &PeerId) -> Option<RpcId> {
        self.peers.get(peer_id)?.connection_rpc_id()
    }

    /// Get peer in ready state. `None` if peer isn't in `Ready` state,
    /// or if peer doesn't exist.
    pub fn get_ready_peer(&self, peer_id: &PeerId) -> Option<&P2pPeerStatusReady> {
        self.peers.get(peer_id)?.status.as_ready()
    }

    /// Get peer in ready state. `None` if peer isn't in `Ready` state,
    /// or if peer doesn't exist.
    pub fn get_ready_peer_mut(&mut self, peer_id: &PeerId) -> Option<&mut P2pPeerStatusReady> {
        self.peers.get_mut(peer_id)?.status.as_ready_mut()
    }

    pub fn any_ready_peers(&self) -> bool {
        self.peers
            .iter()
            .any(|(_, p)| p.status.as_ready().is_some())
    }

    pub fn dial_backoff_config(&self) -> P2pDialBackoffConfig {
        P2pDialBackoffConfig::new(&self.config.timeouts, &self.config.limits)
    }

    /// Whether the dial backoff allows dialling `opts` at `now`: always for
    /// WebRTC peers, and for libp2p peers unless the peer id or the address
    /// is backing off, the peer id is stale at that address, or a redial
    /// would go over the cap ([`P2pDialBackoff`]).
    pub fn may_dial(&self, opts: &P2pConnectionOutgoingInitOpts, now: Timestamp) -> bool {
        !opts.is_libp2p()
            || self.dial_backoff.allows(
                opts.peer_id(),
                opts.libp2p_socket_addr(),
                now,
                &self.dial_backoff_config(),
            )
    }

    /// Whether `peer_id` is one of our seeds, the initial peers.
    pub fn is_seed(&self, peer_id: &PeerId) -> bool {
        self.config
            .initial_peers
            .iter()
            .any(|opts| opts.peer_id() == peer_id)
    }

    /// Whether we dial seeds again for a connection, and not only through
    /// discovery: while fewer than [`P2pLimits::seed_reconnect_below`] ready
    /// libp2p peers are not seeds. OCaml dials its seeds at start
    /// (`BeginAdvertising`) and later only when its DHT routing table is
    /// empty (go-libp2p-kad-dht's `fixLowPeers`); they are not protected
    /// from its connection manager's trimming.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/app/libp2p_helper/src/libp2p_helper/config_msg.go#L59-L66>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/app/libp2p_helper/src/codanet.go#L738-L743>
    pub fn wants_seed_connections(&self) -> bool {
        let non_seeds = self
            .peers
            .iter()
            .filter(|(id, p)| p.is_libp2p() && p.status.as_ready().is_some() && !self.is_seed(id))
            .count();
        non_seeds < self.config.limits.seed_reconnect_below()
    }

    /// Peers we may redial now: disconnected or failed, with dial options,
    /// past their reconnect timeout and their backoff, and not seeds unless
    /// we want seed connections ([`Self::wants_seed_connections`]).
    pub fn reconnect_candidates(
        &self,
        now: Timestamp,
    ) -> impl '_ + Iterator<Item = &P2pConnectionOutgoingInitOpts> {
        let timeouts = &self.config.timeouts;
        let wants_seeds = self.wants_seed_connections();
        self.peers.iter().filter_map(move |(peer_id, peer)| {
            let opts = self.select_dial_address(peer_id, peer, now)?;
            (peer.can_reconnect(now, timeouts)
                && (!opts.is_libp2p() || wants_seeds || !self.is_seed(peer_id)))
            .then_some(opts)
        })
    }

    /// Add an address without replacing a proven one. The bound also applies
    /// when Identify advertises many addresses in one message.
    pub fn add_dial_address(
        &mut self,
        peer_id: PeerId,
        opts: P2pConnectionOutgoingInitOpts,
        observed: Option<std::net::IpAddr>,
    ) -> bool {
        let P2pConnectionOutgoingInitOpts::LibP2P(candidate) = &opts else {
            return false;
        };
        let rank = candidate.dial_rank(observed);
        let existing = self.peers.get(&peer_id).and_then(|peer| {
            let opts = peer.dial_opts.clone()?;
            let success = peer
                .status
                .as_ready()
                .and_then(|ready| (!ready.is_incoming).then_some(ready.connected_since));
            Some((opts, success))
        });
        let addresses = self.dial_addresses.entry(peer_id).or_default();
        if addresses.is_empty() {
            if let Some((existing_opts, last_success)) =
                existing.filter(|(opts, _)| opts.is_libp2p())
            {
                let mut entry = P2pDialAddress::new(existing_opts, observed);
                entry.last_success = last_success;
                addresses.push(entry);
            }
        }
        if let Some(entry) = addresses.iter_mut().find(|entry| entry.opts == opts) {
            entry.rank = entry.rank.max(rank);
            return false;
        }
        if addresses.len() >= P2pDialAddress::MAX_PER_PEER {
            let Some((index, worst)) = addresses
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.last_success.is_none())
                .max_by_key(|(_, entry)| (entry.failures, std::cmp::Reverse(entry.rank)))
            else {
                return false;
            };
            let worst_rank = worst.rank;
            if worst.failures == 0 && rank <= worst_rank {
                return false;
            }
            addresses.remove(index);
        }
        addresses.push(P2pDialAddress::new(opts.clone(), observed));
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            let has_working = addresses.iter().any(|entry| entry.last_success.is_some());
            if peer.dial_opts.is_none()
                || (!has_working
                    && peer
                        .dial_opts
                        .as_ref()
                        .is_some_and(|current| opts.is_better_than(current, observed)))
            {
                peer.dial_opts = Some(opts);
            }
        }
        true
    }

    fn select_dial_address<'a>(
        &'a self,
        peer_id: &PeerId,
        peer: &'a P2pPeerState,
        now: Timestamp,
    ) -> Option<&'a P2pConnectionOutgoingInitOpts> {
        self.select_dial_address_with_backoff(peer_id, peer, now, true)
    }

    fn select_dial_address_with_backoff<'a>(
        &'a self,
        peer_id: &PeerId,
        peer: &'a P2pPeerState,
        now: Timestamp,
        respect_backoff: bool,
    ) -> Option<&'a P2pConnectionOutgoingInitOpts> {
        let addresses = self.dial_addresses.get(peer_id);
        if let Some(addresses) = addresses.filter(|addresses| !addresses.is_empty()) {
            return addresses
                .iter()
                .filter(|entry| {
                    entry.retry_at <= now && (!respect_backoff || self.may_dial(&entry.opts, now))
                })
                .max_by_key(|entry| {
                    (
                        u8::from(entry.last_success.is_some()) * 2
                            + u8::from(entry.failures == 0 && entry.last_success.is_none()),
                        entry.last_success.unwrap_or(Timestamp::ZERO),
                        std::cmp::Reverse(entry.failures),
                        if entry.last_success.is_none() && entry.failures == 0 {
                            entry.rank
                        } else {
                            (0, 0)
                        },
                    )
                })
                .map(|entry| &entry.opts);
        }
        peer.dial_opts
            .as_ref()
            .filter(|opts| !respect_backoff || self.may_dial(opts, now))
    }

    pub fn dial_address_known(
        &self,
        peer_id: &PeerId,
        opts: &P2pConnectionOutgoingInitOpts,
    ) -> bool {
        self.dial_addresses
            .get(peer_id)
            .is_some_and(|addresses| addresses.iter().any(|entry| &entry.opts == opts))
    }

    pub fn record_dial_address_success(&mut self, peer_id: PeerId, now: Timestamp) {
        let Some(opts) = self
            .peers
            .get(&peer_id)
            .and_then(|peer| peer.dial_opts.clone())
        else {
            return;
        };
        self.add_dial_address(peer_id, opts.clone(), None);
        if let Some(entry) = self
            .dial_addresses
            .get_mut(&peer_id)
            .and_then(|addresses| addresses.iter_mut().find(|entry| entry.opts == opts))
        {
            entry.last_success = Some(now);
            entry.failures = 0;
            entry.retry_at = Timestamp::ZERO;
        }
    }

    fn record_dial_address_failure(&mut self, peer_id: PeerId, now: Timestamp) {
        let Some(opts) = self
            .peers
            .get(&peer_id)
            .and_then(|peer| peer.dial_opts.clone())
        else {
            return;
        };
        self.add_dial_address(peer_id, opts.clone(), None);
        if let Some(entry) = self
            .dial_addresses
            .get_mut(&peer_id)
            .and_then(|addresses| addresses.iter_mut().find(|entry| entry.opts == opts))
        {
            entry.failed(
                now,
                self.config
                    .timeouts
                    .outgoing_error_reconnect_timeout
                    .unwrap_or(Duration::from_secs(30)),
            );
        }
    }

    /// Libp2p peers that are ready.
    pub fn libp2p_ready_peers_count(&self) -> usize {
        self.peers
            .values()
            .filter(|p| p.is_libp2p() && p.status.as_ready().is_some())
            .count()
    }

    /// Libp2p peers that are ready or that we are connecting to (not failed).
    pub fn libp2p_connected_or_connecting_count(&self) -> usize {
        self.peers
            .values()
            .filter(|p| p.is_libp2p() && p.status.is_connected_or_connecting())
            .count()
    }

    /// Whether we have fewer ready libp2p peers than we keep
    /// ([`P2pLimits::libp2p_target_peers`]), and so look for more.
    pub fn wants_more_libp2p_peers(&self) -> bool {
        self.libp2p_ready_peers_count() < self.config.limits.libp2p_target_peers()
    }

    /// How many more libp2p peers we may dial for now: the target
    /// ([`P2pLimits::libp2p_target_peers`]) less the libp2p peers that are
    /// ready or connecting, so dials in flight count too. OCaml's helper dials
    /// what it discovers only while it has fewer connections than its
    /// low-water mark.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/app/libp2p_helper/src/libp2p_helper/config_msg.go#L86-L93>
    pub fn libp2p_dial_deficit(&self) -> usize {
        match self.config.limits.libp2p_target_peers() {
            Limit::Some(target) => {
                target.saturating_sub(self.libp2p_connected_or_connecting_count())
            }
            Limit::Unlimited => usize::MAX,
        }
    }

    /// The peers to dial now, in order. WebRTC peers are all the
    /// [`Self::reconnect_candidates`], as before. Libp2p peers only fill the
    /// [`Self::libp2p_dial_deficit`], best first ([`Self::dial_priority`]):
    /// newly discovered peers we never tried, then peers whose backoff has
    /// expired, the fewest failures in a row and the least recently tried
    /// first. Until I18b every candidate was dialled at once, in peer id
    /// order, and none was new after the first Kademlia lookup.
    pub fn dial_candidates(&self, now: Timestamp) -> Vec<&P2pConnectionOutgoingInitOpts> {
        let (mut libp2p, mut candidates): (Vec<_>, Vec<_>) = self
            .reconnect_candidates(now)
            .partition(|opts| opts.is_libp2p());
        libp2p.sort_by_cached_key(|opts| self.dial_priority(opts));
        libp2p.truncate(self.libp2p_dial_deficit());
        candidates.extend(libp2p);
        candidates
    }

    /// The order of libp2p dial candidates, lowest first: whether we dialled
    /// the peer id or its address before, failures in a row (of the peer or
    /// the address, whichever has more), the last dial or connection, and the
    /// peer id to break ties.
    pub fn dial_priority(
        &self,
        opts: &P2pConnectionOutgoingInitOpts,
    ) -> (bool, u32, Timestamp, PeerId) {
        let peer_id = *opts.peer_id();
        let addr = opts.libp2p_socket_addr();
        let by_peer = self.dial_backoff.peer(&peer_id);
        let by_addr = addr.and_then(|addr| self.dial_backoff.addr(&addr));
        let failures = by_peer
            .map_or(0, |e| e.failures)
            .max(by_addr.map_or(0, |e| e.failures));
        let last_event = by_peer.map_or(Timestamp::ZERO, |e| e.last_event);
        (
            self.dial_backoff.is_redial(&peer_id, addr),
            failures,
            last_event,
            peer_id,
        )
    }

    /// The libp2p peer to forget to make room in the peer table: a peer we
    /// dialled that failed or disconnected unhealthily, at least a minute ago,
    /// is not a seed, and has the most failures in a row, then the longest
    /// wait. Its backoff record outlives it, so if it is discovered again it
    /// still waits. Peers we never tried, healthy peers and seeds are kept.
    pub fn peer_to_evict(&self, now: Timestamp) -> Option<PeerId> {
        self.peers
            .iter()
            .filter(|(id, p)| {
                p.is_libp2p()
                    && !self.is_seed(id)
                    && (matches!(p.status, P2pPeerStatus::Disconnected { .. })
                        || p.status.is_error())
            })
            .filter_map(|(id, _)| {
                let entry = self.dial_backoff.peer(id)?;
                (entry.failures > 0
                    && now.checked_sub(entry.last_event).unwrap_or_default()
                        >= P2pDialBackoff::REDIAL_WINDOW)
                    .then_some((entry.failures, entry.retry_at, *id))
            })
            .max()
            .map(|(_, _, id)| id)
    }

    /// Whether `peer_id` at `dial_opts` is newly discovered: not in the peer
    /// table, and neither it nor its address was ever dialled.
    pub fn is_new_libp2p_peer(
        &self,
        peer_id: &PeerId,
        dial_opts: Option<&P2pConnectionOutgoingInitOpts>,
    ) -> bool {
        !self.peers.contains_key(peer_id)
            && !self
                .dial_backoff
                .is_redial(peer_id, dial_opts.and_then(|o| o.libp2p_socket_addr()))
    }

    /// Restore configured seeds after quarantine, including retained error
    /// entries that the initial-peer loop otherwise skips. Do not overwrite
    /// a valid alternate address discovered while the seed was quarantined.
    pub fn restore_seed_dial_opts(&mut self, now: Timestamp) {
        let config = self.dial_backoff_config();
        for opts in &self.config.initial_peers {
            if !opts.is_libp2p()
                || self.dial_backoff.is_stale(
                    opts.peer_id(),
                    opts.libp2p_socket_addr(),
                    now,
                    &config,
                )
            {
                continue;
            }
            if let Some(peer) = self.peers.get_mut(opts.peer_id()) {
                if peer.dial_opts.is_none() && !peer.status.is_connected_or_connecting() {
                    peer.dial_opts = Some(opts.clone());
                }
            }
        }
    }

    /// Oldest attempted eligible peer first, so one repeatedly failing peer
    /// cannot monopolize recovery. Normal dials keep their original backoff.
    pub fn recovery_candidate(&self, now: Timestamp) -> Option<&P2pConnectionOutgoingInitOpts> {
        if self.already_has_min_peers()
            || self
                .peers
                .values()
                .filter(|p| p.is_libp2p() && p.status.as_ready().is_some())
                .count()
                >= P2pDialBackoff::RECOVERY_PEER_FLOOR
        {
            return None;
        }
        let config = self.dial_backoff_config();
        self.peers
            .iter()
            .filter_map(|(id, p)| self.select_dial_address_with_backoff(id, p, now, false))
            .chain(self.config.initial_peers.iter())
            .filter(|opts| {
                opts.is_libp2p()
                    && *opts.peer_id() != self.my_id()
                    && self.peers.get(opts.peer_id()).is_none_or(|p| {
                        matches!(p.status, P2pPeerStatus::Disconnected { .. })
                            || p.status.as_connecting().is_some_and(|s| s.is_error())
                    })
                    && self.dial_backoff.allows_recovery(
                        opts.peer_id(),
                        opts.libp2p_socket_addr(),
                        now,
                        &config,
                    )
            })
            .min_by_key(|opts| {
                (
                    self.dial_backoff.peer(opts.peer_id()).map(|e| e.last_event),
                    *opts.peer_id(),
                )
            })
    }

    /// Records a dial to a libp2p peer in the backoff, and logs it.
    pub fn record_libp2p_dial(&mut self, opts: &P2pConnectionOutgoingInitOpts, now: Timestamp) {
        if !opts.is_libp2p() {
            return;
        }
        let peer_id = *opts.peer_id();
        self.add_dial_address(peer_id, opts.clone(), None);
        let addr = opts.libp2p_socket_addr();
        let redial = self.dial_backoff.is_redial(&peer_id, addr);
        let failures = self.dial_backoff.peer(&peer_id).map_or(0, |e| e.failures);
        self.dial_backoff.on_dial(peer_id, addr, now);
        mina_core::debug!(now;
            kind = "P2pDial",
            summary = "dialling a libp2p peer",
            peer_id = display(peer_id),
            addr = debug(addr),
            redial = redial,
            failures = failures,
            seed = self.is_seed(&peer_id),
            redials_in_minute = self.dial_backoff.redials_in_window(now),
            dials_in_minute = self.dial_backoff.dials_in_window(now),
            ready_libp2p_peers = self.libp2p_ready_peers_count(),
        );
    }

    /// Records in the backoff that a dial to the libp2p peer `peer_id` failed
    /// (`was_connected` false) or that its connection ended, and logs the
    /// wait before the next dial.
    pub fn record_libp2p_dial_end(&mut self, peer_id: PeerId, was_connected: bool, now: Timestamp) {
        if !was_connected {
            self.record_dial_address_failure(peer_id, now);
        }
        let config = self.dial_backoff_config();
        let addr = self.dial_backoff.peer(&peer_id).and_then(|e| e.addr);
        let (end, wait) = if was_connected {
            self.dial_backoff.on_disconnected(peer_id, now, &config)
        } else {
            self.dial_backoff.on_dial_failed(peer_id, now, &config)
        };
        let failures = self.dial_backoff.peer(&peer_id).map_or(0, |e| e.failures);
        mina_core::debug!(now;
            kind = "P2pDialBackoff",
            summary = "libp2p peer backs off",
            peer_id = display(peer_id),
            addr = debug(addr),
            end = debug(end),
            failures = failures,
            retry_in_ms = wait.as_millis() as u64,
        );
    }

    /// Drops the dial options of `peer_id` if its address answered as
    /// another peer id ([`crate::P2pPeerAction::Stale`]), so that it isn't
    /// redialled there and may be discovered at another address.
    pub fn forget_stale_dial_opts(&mut self, peer_id: &PeerId, now: Timestamp) {
        let config = self.dial_backoff_config();
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return;
        };
        let addr = peer.dial_opts.as_ref().and_then(|o| o.libp2p_socket_addr());
        if addr.is_some() && self.dial_backoff.is_stale(peer_id, addr, now, &config) {
            peer.dial_opts = None;
            if let Some(addresses) = self.dial_addresses.get_mut(peer_id) {
                addresses.retain(|entry| entry.opts.libp2p_socket_addr() != addr);
            }
        }
    }

    pub fn disconnected_peers(&self) -> impl '_ + Iterator<Item = P2pConnectionOutgoingInitOpts> {
        self.peers.iter().filter_map(|(_, state)| {
            if let P2pPeerState {
                status: P2pPeerStatus::Disconnected { .. },
                dial_opts: Some(opts),
                ..
            } = state
            {
                Some(opts.clone())
            } else {
                None
            }
        })
    }

    pub fn ready_peers_iter(&self) -> impl Iterator<Item = (&PeerId, &P2pPeerStatusReady)> {
        self.peers
            .iter()
            .filter_map(|(id, p)| Some((id, p.status.as_ready()?)))
    }

    pub fn ready_rpc_peers_iter(
        &self,
    ) -> impl '_ + Iterator<Item = (&PeerId, &P2pPeerStatusReady)> {
        self.ready_peers_iter()
            .filter(|(_, p)| p.channels.rpc.can_send_request())
    }

    pub fn ready_peers(&self) -> Vec<PeerId> {
        self.peers
            .iter()
            .filter(|(_, p)| p.status.as_ready().is_some())
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn connected_or_connecting_peers_count(&self) -> usize {
        self.peers
            .iter()
            .filter(|(_, p)| p.status.is_connected_or_connecting())
            .count()
    }

    pub fn is_peer_connecting(&self, peer_id: &PeerId) -> bool {
        self.peers
            .get(peer_id)
            .and_then(|p| p.status.as_connecting())
            .is_some_and(|p| !p.is_error())
    }

    pub fn is_peer_connected_or_connecting(&self, peer_id: &PeerId) -> bool {
        self.peers
            .get(peer_id)
            .is_some_and(|p| p.status.is_connected_or_connecting())
    }

    pub fn is_libp2p_peer(&self, peer_id: &PeerId) -> bool {
        self.peers.get(peer_id).is_some_and(|p| p.is_libp2p())
    }

    /// The IP of our libp2p connection to `peer_id`, if there is one.
    pub fn libp2p_peer_ip(&self, peer_id: &PeerId) -> Option<std::net::IpAddr> {
        self.network
            .scheduler
            .find_peer(peer_id)
            .map(|(addr, _)| addr.sock_addr.ip())
    }

    /// Whether a best tip request to `peer_id` at `now` stays within OCaml's
    /// rate limit ([`P2pBestTipRequestBudget`]). Only libp2p peers are OCaml
    /// nodes, so WebRTC peers are always within it.
    pub fn is_best_tip_request_within_budget(&self, peer_id: &PeerId, now: Timestamp) -> bool {
        if !self.is_libp2p_peer(peer_id) {
            return true;
        }
        let min_interval = self
            .config
            .timeouts
            .best_tip_request_interval
            .unwrap_or_default();
        self.best_tip_requests
            .allows(peer_id, self.libp2p_peer_ip(peer_id), now, min_interval)
    }

    pub fn is_completed_snarks_request_within_budget(
        &self,
        peer_id: &PeerId,
        now: Timestamp,
    ) -> bool {
        self.is_libp2p_peer(peer_id)
            && self
                .completed_snarks_requests
                .allows(peer_id, self.libp2p_peer_ip(peer_id), now)
    }

    pub fn is_peer_rpc_timed_out(
        &self,
        peer_id: &PeerId,
        rpc_id: P2pRpcId,
        now: redux::Timestamp,
    ) -> bool {
        self.get_ready_peer(peer_id).is_some_and(|p| {
            p.channels
                .rpc
                .is_timed_out(rpc_id, now, &self.config.timeouts)
        })
    }

    pub fn is_peer_streaming_rpc_timed_out(
        &self,
        peer_id: &PeerId,
        rpc_id: P2pStreamingRpcId,
        now: redux::Timestamp,
    ) -> bool {
        self.get_ready_peer(peer_id).is_some_and(|p| {
            p.channels
                .streaming_rpc
                .is_timed_out(rpc_id, now, &self.config.timeouts)
        })
    }

    pub fn peer_rpc_timeouts(&self, now: redux::Timestamp) -> Vec<(PeerId, P2pRpcId, bool)> {
        let config = &self.config.timeouts;
        self.ready_peers_iter()
            .flat_map(|(peer_id, s)| {
                let pending_rpc = s.channels.rpc.unanswered_local_rpc().map(|(id, _)| id);
                let pending_streaming_rpc = s.channels.streaming_rpc.pending_local_rpc_id();
                IntoIterator::into_iter([
                    pending_rpc
                        .filter(|id| s.channels.rpc.is_timed_out(*id, now, config))
                        .map(|id| (*peer_id, id, false)),
                    pending_streaming_rpc
                        .filter(|id| s.channels.streaming_rpc.is_timed_out(*id, now, config))
                        .map(|id| (*peer_id, id, true)),
                ])
                .flatten()
            })
            .collect()
    }

    pub fn peer_streaming_rpc_timeouts(
        &self,
        now: redux::Timestamp,
    ) -> Vec<(PeerId, P2pStreamingRpcId)> {
        self.ready_peers_iter()
            .filter_map(|(peer_id, s)| {
                let rpc_id = s.channels.streaming_rpc.pending_local_rpc_id()?;
                if !s
                    .channels
                    .streaming_rpc
                    .is_timed_out(rpc_id, now, &self.config.timeouts)
                {
                    return None;
                }

                Some((*peer_id, rpc_id))
            })
            .collect()
    }

    pub fn already_has_min_peers(&self) -> bool {
        self.connected_or_connecting_peers_count() >= self.config.limits.min_peers()
    }

    pub fn already_has_max_peers(&self) -> bool {
        self.connected_or_connecting_peers_count() >= self.config.limits.max_peers()
    }

    /// The peers capacity is exceeded.
    pub fn already_has_max_ready_peers(&self) -> bool {
        self.ready_peers_iter().count() >= self.config.limits.max_peers()
    }

    /// Minimal number of peers that the node should connect
    pub fn min_peers(&self) -> Limit<usize> {
        self.config.limits.min_peers()
    }

    /// Peer with libp2p connection identified by `conn_id`.
    #[cfg(feature = "p2p-libp2p")]
    pub fn peer_with_connection(
        &self,
        conn_id: crate::ConnectionAddr,
    ) -> Option<(PeerId, &P2pPeerState)> {
        let result = if let crate::ConnectionAddr {
            sock_addr,
            incoming: false,
        } = conn_id
        {
            self.peers
                .iter()
                .find(|(_, peer_state)| match &peer_state.dial_opts {
                    Some(P2pConnectionOutgoingInitOpts::LibP2P(libp2p_opts)) => {
                        libp2p_opts.matches_socket_addr(sock_addr)
                    }
                    _ => false,
                })
        } else {
            None
        };

        result
            .or_else(|| {
                self.network
                    .scheduler
                    .connections
                    .get(&conn_id)
                    .and_then(|state| {
                        state
                            .peer_id()
                            .and_then(|peer_id| self.peers.iter().find(|(id, _)| *id == peer_id))
                    })
            })
            .map(|(peer_id, peer_state)| (*peer_id, peer_state))
    }

    pub fn incoming_peer_connection_mut(
        &mut self,
        peer_id: &PeerId,
    ) -> Option<&mut P2pConnectionIncomingState> {
        let peer_state = self.peers.get_mut(peer_id)?;

        match &mut peer_state.status {
            P2pPeerStatus::Connecting(P2pConnectionState::Incoming(state)) => Some(state),
            _ => None,
        }
    }

    pub fn outgoing_peer_connection_mut(
        &mut self,
        peer_id: &PeerId,
    ) -> Option<&mut P2pConnectionOutgoingState> {
        let peer_state = self.peers.get_mut(peer_id)?;

        match &mut peer_state.status {
            P2pPeerStatus::Connecting(P2pConnectionState::Outgoing(state)) => Some(state),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
pub struct P2pPeerState {
    pub is_libp2p: bool,
    pub dial_opts: Option<P2pConnectionOutgoingInitOpts>,
    pub status: P2pPeerStatus,
    pub identify: Option<P2pNetworkIdentify>,
}

impl P2pPeerState {
    pub fn is_libp2p(&self) -> bool {
        self.is_libp2p
    }

    pub fn connection_rpc_id(&self) -> Option<RpcId> {
        match &self.status {
            P2pPeerStatus::Connecting(v) => v.rpc_id(),
            _ => None,
        }
    }

    /// Returns true if the peer can be reconnected, that is:
    /// - it is never been connected yet or enough time is passed since its connection failure or disconnection.
    pub fn can_reconnect(&self, now: Timestamp, timeouts: &P2pTimeouts) -> bool {
        match &self.status {
            P2pPeerStatus::Connecting(P2pConnectionState::Incoming(
                P2pConnectionIncomingState::Error { time, .. },
            )) => is_time_passed(now, *time, timeouts.incoming_error_reconnect_timeout),
            P2pPeerStatus::Connecting(P2pConnectionState::Outgoing(
                P2pConnectionOutgoingState::Error { time, .. },
            )) => is_time_passed(now, *time, timeouts.outgoing_error_reconnect_timeout),
            P2pPeerStatus::Disconnected { time } => {
                *time == Timestamp::ZERO || is_time_passed(now, *time, timeouts.reconnect_timeout)
            }
            P2pPeerStatus::Disconnecting { time } => {
                if !is_time_passed(now, *time, timeouts.reconnect_timeout) {
                    false
                } else {
                    #[cfg(not(test))]
                    mina_core::bug_condition!(
                        "peer stuck in `P2pPeerStatus::Disconnecting` state?"
                    );
                    #[cfg(test)]
                    // in test, since time is virtual, timeout can pass before service finishes cleanup.
                    // Hence it shouldn't be a bug condition in such case.
                    mina_core::warn!(*time; "peer stuck in `P2pPeerStatus::Disconnecting` state?");
                    true
                }
            }
            _ => false,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
#[serde(tag = "state")]
pub enum P2pPeerStatus {
    Connecting(P2pConnectionState),
    Disconnecting {
        #[ignore_malloc_size_of = "doesn't allocate"]
        time: redux::Timestamp,
    },
    Disconnected {
        #[ignore_malloc_size_of = "doesn't allocate"]
        time: redux::Timestamp,
    },

    Ready(Box<P2pPeerStatusReady>),
}

impl P2pPeerStatus {
    /// Checks if the peer is in `Connecting` state and we have finished
    /// connecting to the peer.
    pub fn is_connecting_success(&self) -> bool {
        match self {
            Self::Connecting(v) => v.is_success(),
            _ => false,
        }
    }

    pub fn is_connected_or_connecting(&self) -> bool {
        match self {
            Self::Connecting(s) => !s.is_error(),
            Self::Ready(_) => true,
            Self::Disconnecting { .. } => false,
            Self::Disconnected { .. } => false,
        }
    }

    pub fn is_disconnected_or_disconnecting(&self) -> bool {
        self.disconnected_or_disconnecting_time().is_some()
    }

    pub fn disconnected_or_disconnecting_time(&self) -> Option<redux::Timestamp> {
        match self {
            Self::Disconnecting { time } | Self::Disconnected { time } => Some(*time),
            _ => None,
        }
    }

    pub fn as_connecting(&self) -> Option<&P2pConnectionState> {
        match self {
            Self::Connecting(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_ready(&self) -> Option<&P2pPeerStatusReady> {
        match self {
            Self::Ready(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_ready_mut(&mut self) -> Option<&mut P2pPeerStatusReady> {
        match self {
            Self::Ready(v) => Some(v),
            _ => None,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, P2pPeerStatus::Connecting(s) if s.is_error())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pPeerStatusReady {
    pub is_incoming: bool,
    pub connected_since: redux::Timestamp,
    pub channels: P2pChannelsState,
    pub best_tip: Option<ArcBlockWithHash>,
}

impl P2pPeerStatusReady {
    pub fn new(
        is_incoming: bool,
        time: redux::Timestamp,
        enabled_channels: &BTreeSet<ChannelId>,
    ) -> Self {
        Self {
            is_incoming,
            connected_since: time,
            channels: P2pChannelsState::new(enabled_channels),
            best_tip: None,
        }
    }

    pub fn connected_for(&self, now: redux::Timestamp) -> Duration {
        now.checked_sub(self.connected_since).unwrap_or_default()
    }
}

impl SubstateAccess<P2pState> for P2pState {
    fn substate(&self) -> mina_core::SubstateResult<&P2pState> {
        Ok(self)
    }

    fn substate_mut(&mut self) -> mina_core::SubstateResult<&mut P2pState> {
        Ok(self)
    }
}

type OptionalCallback<T> = Option<Callback<T>>;

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct P2pCallbacks {
    /// Callback for [`crate::channels::transaction::P2pChannelsTransactionAction::Received`]
    pub on_p2p_channels_transaction_received: OptionalCallback<(PeerId, Box<TransactionInfo>)>,
    /// Callback for [`crate::channels::transaction::P2pChannelsTransactionAction::Libp2pReceived`]
    pub on_p2p_channels_transactions_libp2p_received: OptionalCallback<(
        PeerId,
        Vec<TransactionWithHash>,
        P2pNetworkPubsubMessageCacheId,
    )>,
    /// Callback for [`crate::channels::snark_job_commitment::P2pChannelsSnarkJobCommitmentAction::Received`]
    pub on_p2p_channels_snark_job_commitment_received:
        OptionalCallback<(PeerId, Box<SnarkJobCommitment>)>,

    /// Callback for [`crate::channels::snark::P2pChannelsSnarkAction::Received`]
    pub on_p2p_channels_snark_received: OptionalCallback<(PeerId, Box<SnarkInfo>)>,
    /// Callback for [`crate::channels::snark::P2pChannelsSnarkAction::Libp2pReceived`]
    pub on_p2p_channels_snark_libp2p_received: OptionalCallback<(PeerId, Box<Snark>)>,

    /// Callback for [`crate::channels::best_tip::P2pChannelsBestTipAction::RequestReceived`]
    pub on_p2p_channels_best_tip_request_received: OptionalCallback<PeerId>,

    /// Callback for [`crate::disconnection::P2pDisconnectionAction::Finish`]
    pub on_p2p_disconnection_finish: OptionalCallback<PeerId>,

    /// TODO: these 2 should be set by `P2pConnectionOutgoingAction::Init`
    /// Callback for [`crate::connection::outgoing::P2pConnectionOutgoingAction::Error`]
    pub on_p2p_connection_outgoing_error: OptionalCallback<(RpcId, P2pConnectionOutgoingError)>,
    /// Callback for [`crate::connection::outgoing::P2pConnectionOutgoingAction::Success`]
    pub on_p2p_connection_outgoing_success: OptionalCallback<RpcId>,

    /// TODO: these 3 should be set by `P2pConnectionIncomingAction::Init`
    /// Callback for [`crate::connection::incoming::P2pConnectionIncomingAction::Error`]
    pub on_p2p_connection_incoming_error: OptionalCallback<(RpcId, String)>,
    /// Callback for [`crate::connection::incoming::P2pConnectionIncomingAction::Success`]
    pub on_p2p_connection_incoming_success: OptionalCallback<RpcId>,
    /// Callback for [`crate::connection::incoming::P2pConnectionIncomingAction::AnswerReady`]
    pub on_p2p_connection_incoming_answer_ready:
        OptionalCallback<(RpcId, PeerId, P2pConnectionResponse)>,

    /// Callback for [`crate::peer::P2pPeerAction::BestTipUpdate`]
    pub on_p2p_peer_best_tip_update:
        OptionalCallback<BlockWithHash<Arc<v2::MinaBlockBlockStableV2>>>,

    /// Callback for [`crate::channels::rpc::P2pChannelsRpcAction::Ready`]
    pub on_p2p_channels_rpc_ready: OptionalCallback<PeerId>,
    /// Callback for [`crate::channels::rpc::P2pChannelsRpcAction::Timeout`]
    pub on_p2p_channels_rpc_timeout: OptionalCallback<(PeerId, P2pRpcId)>,
    /// Callback for [`crate::channels::rpc::P2pChannelsRpcAction::ResponseReceived`]
    pub on_p2p_channels_rpc_response_received:
        OptionalCallback<(PeerId, P2pRpcId, Option<Box<P2pRpcResponse>>)>,
    /// Callback for [`crate::channels::rpc::P2pChannelsRpcAction::RequestReceived`]
    pub on_p2p_channels_rpc_request_received:
        OptionalCallback<(PeerId, P2pRpcId, Box<P2pRpcRequest>)>,

    /// Callback for [`crate::channels::streaming_rpc::P2pChannelsStreamingRpcAction::Ready`]
    pub on_p2p_channels_streaming_rpc_ready: OptionalCallback<()>,
    /// Callback for [`crate::channels::streaming_rpc::P2pChannelsStreamingRpcAction::Timeout`]
    pub on_p2p_channels_streaming_rpc_timeout: OptionalCallback<(PeerId, P2pRpcId)>,
    /// Callback for [`crate::channels::streaming_rpc::P2pChannelsStreamingRpcAction::ResponseReceived`]
    pub on_p2p_channels_streaming_rpc_response_received:
        OptionalCallback<(PeerId, P2pRpcId, Option<P2pStreamingRpcResponseFull>)>,

    /// Callback for received pubsub message
    pub on_p2p_pubsub_message_received: OptionalCallback<P2pNetworkPubsubMessageCacheId>,
}

impl_substate_access!(P2pState, P2pNetworkState, network);
impl_substate_access!(P2pState, P2pNetworkSchedulerState, network.scheduler);
impl_substate_access!(P2pState, P2pLimits, config.limits);

impl SubstateAccess<P2pNetworkKadState> for P2pState {
    fn substate(&self) -> mina_core::SubstateResult<&P2pNetworkKadState> {
        self.network
            .scheduler
            .discovery_state()
            .ok_or_else(|| "kademlia state is unavailable".to_owned())
    }

    fn substate_mut(&mut self) -> mina_core::SubstateResult<&mut P2pNetworkKadState> {
        self.network
            .scheduler
            .discovery_state
            .as_mut()
            .ok_or_else(|| "kademlia state is unavailable".to_owned())
    }
}

impl SubstateAccess<P2pNetworkKadBootstrapState> for P2pState {
    fn substate(&self) -> mina_core::SubstateResult<&P2pNetworkKadBootstrapState> {
        let kad_state: &P2pNetworkKadState = self.substate()?;
        kad_state
            .bootstrap_state()
            .ok_or_else(|| "bootstrap state is unavailable".to_owned())
    }

    fn substate_mut(&mut self) -> mina_core::SubstateResult<&mut P2pNetworkKadBootstrapState> {
        let kad_state: &mut P2pNetworkKadState = self.substate_mut()?;
        kad_state
            .bootstrap_state_mut()
            .ok_or_else(|| "bootstrap state is unavailable".to_owned())
    }
}

impl_substate_access!(
    P2pState,
    P2pNetworkIdentifyState,
    network.scheduler.identify_state
);
impl_substate_access!(
    P2pState,
    P2pNetworkPubsubState,
    network.scheduler.broadcast_state
);
impl_substate_access!(P2pState, P2pConfig, config);

mod measurement {
    use malloc_size_of::{MallocSizeOf, MallocSizeOfOps};

    use super::*;

    impl MallocSizeOf for P2pState {
        fn size_of(&self, ops: &mut MallocSizeOfOps) -> usize {
            self.peers.values().map(|v| v.size_of(ops)).sum::<usize>()
                + self
                    .dial_addresses
                    .values()
                    .flat_map(|addresses| addresses.iter())
                    .map(|entry| entry.opts.size_of(ops))
                    .sum::<usize>()
                + self.network.scheduler.size_of(ops)
        }
    }

    impl MallocSizeOf for P2pPeerStatusReady {
        fn size_of(&self, ops: &mut MallocSizeOfOps) -> usize {
            self.best_tip
                .as_ref()
                .map(|v| {
                    usize::from(!ops.have_seen_ptr(Arc::as_ptr(&v.block)))
                        * (size_of::<v2::MinaBlockBlockStableV2>() + v.block.size_of(ops))
                })
                .unwrap_or_default()
            // TODO(vlad): `channels`
        }
    }
}

/// The dial backoff in the state machine (I18): what the enabling conditions
/// and the reconnect loops let through.
#[cfg(test)]
mod dial_backoff_tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use mina_core::{requests::RpcId, MAINNET_CHAIN_ID};
    use redux::EnablingCondition;

    use super::*;
    use crate::{
        channels::ChannelId,
        connection::outgoing::{P2pConnectionOutgoingAction, P2pDialEnd},
        identity::SecretKey,
        P2pMeshsubConfig, P2pPeerAction,
    };

    fn peer(n: u8) -> PeerId {
        SecretKey::from_bytes([n; 32]).public_key().peer_id()
    }

    fn addr(n: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, n)), 8302)
    }

    fn opts(n: u8) -> P2pConnectionOutgoingInitOpts {
        P2pConnectionOutgoingInitOpts::LibP2P((peer(n), addr(n)).into())
    }

    fn at(secs: u64) -> Timestamp {
        Timestamp::new(1_000_000_000_000 + secs * 1_000_000_000)
    }

    fn state(seeds: &[u8]) -> P2pState {
        let config = P2pConfig {
            libp2p_port: None,
            listen_port: None,
            identity_pub_key: SecretKey::from_bytes([200; 32]).public_key(),
            initial_peers: seeds.iter().map(|n| opts(*n)).collect(),
            external_addrs: vec![],
            enabled_channels: ChannelId::iter_all().collect(),
            timeouts: Default::default(),
            limits: Default::default(),
            peer_discovery: false,
            meshsub: P2pMeshsubConfig::default(),
            catchup_rpc: Default::default(),
        };
        P2pState::new(config, P2pCallbacks::default(), &MAINNET_CHAIN_ID)
    }

    fn add_peer(state: &mut P2pState, n: u8, status: P2pPeerStatus) {
        state.peers.insert(
            peer(n),
            P2pPeerState {
                is_libp2p: true,
                dial_opts: Some(opts(n)),
                status,
                identify: None,
            },
        );
    }

    fn ready(since: Timestamp) -> P2pPeerStatus {
        P2pPeerStatus::Ready(Box::new(P2pPeerStatusReady::new(
            false,
            since,
            &BTreeSet::new(),
        )))
    }

    fn init(n: u8, rpc_id: Option<RpcId>) -> P2pConnectionOutgoingAction {
        P2pConnectionOutgoingAction::Init {
            opts: opts(n),
            rpc_id,
            on_success: None,
        }
    }

    /// Dials peer `n`, has it ready, then closed by the peer `lifetime`
    /// seconds later, as the reducers record it.
    fn connect_and_close(state: &mut P2pState, n: u8, from: u64, lifetime: u64) {
        state.record_libp2p_dial(&opts(n), at(from));
        state.dial_backoff.on_ready(peer(n), false, at(from));
        state.record_libp2p_dial_end(peer(n), true, at(from + lifetime));
    }

    /// Init, which Kademlia, the initial peers and discovery use, used to
    /// dial any peer that wasn't connecting, whatever its history.
    #[test]
    fn init_waits_for_the_backoff() {
        let mut state = state(&[]);
        connect_and_close(&mut state, 1, 0, 10);
        connect_and_close(&mut state, 1, 40, 10);
        let retry = state
            .dial_backoff
            .retry_at(&peer(1), Some(addr(1)))
            .unwrap();
        // Two short connections: about 60 s after the second ended.
        assert!(retry >= at(50 + 48) && retry <= at(50 + 72), "{retry:?}");

        assert!(!init(1, None).is_enabled(&state, at(60)));
        assert!(init(1, None).is_enabled(&state, retry));
        // The operator's dial, through the RPC, is not held back.
        let rpc_id = RpcId::new_unchecked(0, 1);
        assert!(init(1, Some(rpc_id)).is_enabled(&state, at(60)));
        // Another peer is not affected.
        assert!(init(2, None).is_enabled(&state, at(60)));
    }

    #[test]
    fn reconnect_waits_for_the_backoff() {
        let mut state = state(&[]);
        for i in 0..4 {
            connect_and_close(&mut state, 1, i * 1000, 10);
        }
        add_peer(
            &mut state,
            1,
            P2pPeerStatus::Disconnected { time: at(3010) },
        );
        let retry = state
            .dial_backoff
            .retry_at(&peer(1), Some(addr(1)))
            .unwrap();
        assert!(retry >= at(3010 + 192), "{retry:?}");

        let reconnect = P2pConnectionOutgoingAction::Reconnect {
            opts: opts(1),
            rpc_id: None,
        };
        // Past the 30 s reconnect timeout, not past the backoff.
        assert!(!reconnect.is_enabled(&state, at(3050)));
        assert_eq!(state.reconnect_candidates(at(3050)).count(), 0);
        assert!(!P2pConnectionOutgoingAction::RandomInit.is_enabled(&state, at(3050)));
        assert!(reconnect.is_enabled(&state, retry));
        assert_eq!(
            state
                .reconnect_candidates(retry)
                .cloned()
                .collect::<Vec<_>>(),
            vec![opts(1)]
        );
        assert!(P2pConnectionOutgoingAction::RandomInit.is_enabled(&state, retry));
    }

    #[test]
    fn a_connection_that_lasted_resets_the_backoff() {
        let mut state = state(&[]);
        for i in 0..5 {
            connect_and_close(&mut state, 1, i * 2000, 10);
        }
        assert_eq!(state.dial_backoff.peer(&peer(1)).unwrap().failures, 5);
        state.record_libp2p_dial(&opts(1), at(20_000));
        state.dial_backoff.on_ready(peer(1), false, at(20_000));
        state.record_libp2p_dial_end(peer(1), true, at(20_000 + 3600));
        let entry = state.dial_backoff.peer(&peer(1)).unwrap();
        assert_eq!(entry.failures, 0);
        assert!(entry.retry_at <= at(23_600 + 36));
    }

    /// Seeds are dialled for connections while we have few other peers, and
    /// otherwise left to discovery, as OCaml does.
    #[test]
    fn seeds_are_redialled_only_while_other_peers_are_few() {
        let mut state = state(&[1, 2]);
        add_peer(&mut state, 1, P2pPeerStatus::Disconnected { time: at(0) });
        add_peer(&mut state, 3, P2pPeerStatus::Disconnected { time: at(0) });
        add_peer(&mut state, 10, ready(at(0)));
        add_peer(&mut state, 11, ready(at(0)));
        assert!(state.is_seed(&peer(1)) && !state.is_seed(&peer(3)));
        assert!(state.wants_seed_connections());
        let candidates = |state: &P2pState| {
            state
                .reconnect_candidates(at(100))
                .map(|o| *o.peer_id())
                .collect::<BTreeSet<_>>()
        };
        assert_eq!(candidates(&state), BTreeSet::from([peer(1), peer(3)]));

        // A third non-seed peer is ready: seed 1 is left to discovery.
        add_peer(&mut state, 12, ready(at(0)));
        assert!(!state.wants_seed_connections());
        assert_eq!(candidates(&state), BTreeSet::from([peer(3)]));
        // Kademlia may still dial a seed (Init is not limited to seeds).
        assert!(init(2, None).is_enabled(&state, at(100)));
        // A ready seed doesn't count as another peer.
        add_peer(&mut state, 2, ready(at(0)));
        assert!(!state.wants_seed_connections());
    }

    #[test]
    fn a_stale_peer_id_is_not_dialled_or_rediscovered_at_its_address() {
        let mut state = state(&[]);
        state.record_libp2p_dial(&opts(1), at(0));
        state
            .dial_backoff
            .on_stale(peer(1), addr(1), peer(9), at(0));
        state.record_libp2p_dial_end(peer(1), false, at(0));
        assert_eq!(
            state.dial_backoff.peer(&peer(1)).map(|e| e.failures),
            Some(1)
        );

        let discovered = |a: SocketAddr| P2pPeerAction::Discovered {
            peer_id: peer(1),
            dial_opts: Some(P2pConnectionOutgoingInitOpts::LibP2P((peer(1), a).into())),
        };
        // Much later than any backoff, still not at that address.
        assert!(!init(1, None).is_enabled(&state, at(1800)));
        assert!(!discovered(addr(1)).is_enabled(&state, at(1800)));
        // At another address it is a new record.
        assert!(discovered(addr(2)).is_enabled(&state, at(1800)));
        // The peer that answered is not stale.
        assert!(!state.dial_backoff.is_stale(
            &peer(9),
            Some(addr(1)),
            at(1800),
            &state.dial_backoff_config()
        ));
        // After an hour it may be tried once more.
        assert!(init(1, None).is_enabled(&state, at(3601)));
    }

    #[test]
    fn working_private_address_survives_public_advertisement_and_public_failure() {
        let mut state = state(&[]);
        add_peer(&mut state, 1, P2pPeerStatus::Disconnected { time: at(0) });
        state.add_dial_address(peer(1), opts(1), None);
        state.record_dial_address_success(peer(1), at(0));
        let public = P2pConnectionOutgoingInitOpts::LibP2P(
            (peer(1), "203.0.113.7:8302".parse().unwrap()).into(),
        );
        let discovered = P2pPeerAction::Discovered {
            peer_id: peer(1),
            dial_opts: Some(public.clone()),
        };
        assert!(discovered.is_enabled(&state, at(1)));
        assert!(state.add_dial_address(peer(1), public.clone(), None));
        assert_eq!(state.peers[&peer(1)].dial_opts, Some(opts(1)));
        assert_eq!(
            state.select_dial_address(&peer(1), &state.peers[&peer(1)], at(1)),
            Some(&opts(1))
        );

        // Once the working target fails, the advertised one is tried. Its
        // failed attempt puts it in backoff; the proven private target remains.
        state.record_dial_address_failure(peer(1), at(2));
        assert_eq!(
            state.select_dial_address(&peer(1), &state.peers[&peer(1)], at(3)),
            Some(&public)
        );
        state.peers.get_mut(&peer(1)).unwrap().dial_opts = Some(public.clone());
        state.record_dial_address_failure(peer(1), at(4));
        assert_eq!(
            state.select_dial_address(&peer(1), &state.peers[&peer(1)], at(33)),
            Some(&opts(1))
        );
    }

    #[test]
    fn discovered_addresses_are_bounded_and_dns_is_untried() {
        let mut state = state(&[]);
        add_peer(&mut state, 1, P2pPeerStatus::Disconnected { time: at(0) });
        state.add_dial_address(peer(1), opts(1), None);
        state.record_dial_address_success(peer(1), at(0));
        let dns = P2pConnectionOutgoingInitOpts::LibP2P(
            crate::connection::outgoing::P2pConnectionOutgoingInitLibp2pOpts::from_listen_addr(
                peer(1),
                &"/dns4/example.org/tcp/8302".parse().unwrap(),
            )
            .unwrap(),
        );
        assert!(state.add_dial_address(peer(1), dns.clone(), None));
        assert_eq!(state.peers[&peer(1)].dial_opts, Some(opts(1)));
        assert!(state.dial_addresses[&peer(1)]
            .iter()
            .any(|entry| entry.opts == dns && entry.last_success.is_none()));
        state.record_dial_address_failure(peer(1), at(1));
        assert_eq!(
            state.select_dial_address(&peer(1), &state.peers[&peer(1)], at(2)),
            Some(&dns)
        );
        for n in 1..=30 {
            let address = format!("198.51.100.{n}:8302").parse().unwrap();
            let candidate = P2pConnectionOutgoingInitOpts::LibP2P((peer(1), address).into());
            state.add_dial_address(peer(1), candidate, None);
        }
        assert!(state.dial_addresses[&peer(1)].len() <= P2pDialAddress::MAX_PER_PEER);
        assert!(state.dial_addresses[&peer(1)]
            .iter()
            .any(|entry| entry.opts == opts(1) && entry.last_success.is_some()));
    }

    #[test]
    fn redials_are_capped_per_minute_new_peers_are_not() {
        let mut state = state(&[]);
        for n in 0..25 {
            state.record_libp2p_dial(&opts(n), at(0));
            state.record_libp2p_dial_end(peer(n), false, at(0));
        }
        let mut allowed = 0;
        for n in 0..25 {
            if init(n, None).is_enabled(&state, at(100)) {
                state.record_libp2p_dial(&opts(n), at(100));
                allowed += 1;
            }
        }
        assert_eq!(allowed, 20);
        assert_eq!(state.dial_backoff.redials_in_window(at(100)), 20);
        assert!(init(100, None).is_enabled(&state, at(100)));
        assert!(init(24, None).is_enabled(&state, at(160)));
    }

    /// Records `failures` failed dials of peer `n`, the last one at `last`.
    fn fail(state: &mut P2pState, n: u8, failures: u64, last: u64) {
        for i in (0..failures).rev() {
            let t = last.saturating_sub(i * 100);
            state.record_libp2p_dial(&opts(n), at(t));
            state.record_libp2p_dial_end(peer(n), false, at(t));
        }
    }

    fn candidates(state: &P2pState, now: u64) -> Vec<PeerId> {
        state
            .dial_candidates(at(now))
            .into_iter()
            .map(|o| *o.peer_id())
            .collect()
    }

    /// I18b: newly discovered peers we never tried come first, then peers
    /// whose backoff has expired, the fewest failures first; peers still
    /// backing off are not candidates at all. The reconnect loop used to
    /// take the candidates in peer id order.
    #[test]
    fn dial_candidates_prefer_new_peers_and_skip_backed_off_ones() {
        let mut state = state(&[]);
        // 1: three failures, the last at 2000 s, due again (120 s at most
        // 144 s later). 2: one failure at 0, due. 3: four failures, the last
        // at 2400 s, waiting 192 to 288 s. 4 and 5: discovered, never tried.
        fail(&mut state, 1, 3, 2000);
        fail(&mut state, 2, 1, 0);
        fail(&mut state, 3, 4, 2400);
        for (n, time) in [(1, 2000), (2, 0), (3, 2400), (4, 0), (5, 0)] {
            add_peer(
                &mut state,
                n,
                P2pPeerStatus::Disconnected { time: at(time) },
            );
        }
        let order = candidates(&state, 2510);
        assert_eq!(order.len(), 4, "{order:?}");
        assert_eq!(
            order[..2].iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([peer(4), peer(5)])
        );
        assert_eq!(order[2..], [peer(2), peer(1)]);
        // Once its backoff expires, peer 3 comes last: the most failures.
        let retry = state
            .dial_backoff
            .retry_at(&peer(3), Some(addr(3)))
            .unwrap();
        let later = (u64::from(retry) - u64::from(at(0))) / 1_000_000_000 + 1;
        assert_eq!(candidates(&state, later).last(), Some(&peer(3)));
    }

    /// I18b: libp2p dials fill only what the target lacks, counting dials in
    /// flight, and stop at it; the operator's RPC dial is not held back.
    #[test]
    fn dial_candidates_fill_only_what_the_target_lacks() {
        let mut state = state(&[]);
        for n in 100..118 {
            add_peer(&mut state, n, ready(at(0)));
        }
        for n in 1..=5 {
            add_peer(
                &mut state,
                n,
                P2pPeerStatus::Disconnected {
                    time: Timestamp::ZERO,
                },
            );
        }
        assert_eq!(state.libp2p_ready_peers_count(), 18);
        assert_eq!(state.libp2p_dial_deficit(), 2);
        assert!(state.wants_more_libp2p_peers());
        assert_eq!(candidates(&state, 100).len(), 2);
        let reconnect = |n: u8, rpc_id: Option<RpcId>| P2pConnectionOutgoingAction::Reconnect {
            opts: opts(n),
            rpc_id,
        };
        assert!(reconnect(1, None).is_enabled(&state, at(100)));

        // A dial in flight counts: one more slot.
        add_peer(
            &mut state,
            118,
            P2pPeerStatus::Connecting(P2pConnectionState::outgoing_init(&opts(118))),
        );
        assert_eq!(state.libp2p_dial_deficit(), 1);
        assert_eq!(candidates(&state, 100).len(), 1);

        // At the target: no candidate, no Reconnect or RandomInit, but the
        // operator's dial.
        add_peer(&mut state, 119, ready(at(0)));
        assert_eq!(state.libp2p_dial_deficit(), 0);
        assert!(candidates(&state, 100).is_empty());
        assert!(!reconnect(1, None).is_enabled(&state, at(100)));
        assert!(!P2pConnectionOutgoingAction::RandomInit.is_enabled(&state, at(100)));
        assert!(reconnect(1, Some(RpcId::new_unchecked(0, 1))).is_enabled(&state, at(100)));
        // 19 ready and one connecting: still looking for peers until 20 are
        // ready.
        assert!(state.wants_more_libp2p_peers());
        state.peers.get_mut(&peer(118)).unwrap().status = ready(at(0));
        assert!(!state.wants_more_libp2p_peers());
    }

    /// I18b: a full peer table forgets its worst failing peer for a newly
    /// discovered one, never a seed, a ready peer, a peer we never tried, or
    /// one that failed in the last minute; a peer we dialled before is not
    /// new and makes no room.
    #[test]
    fn a_full_peer_table_makes_room_for_new_peers_only() {
        let mut state = state(&[1]);
        // The seed has the most failures, then peer 61.
        fail(&mut state, 1, 8, 1000);
        add_peer(
            &mut state,
            1,
            P2pPeerStatus::Disconnected { time: at(1000) },
        );
        for n in 2..=60 {
            fail(&mut state, n, 1 + u64::from(n % 2), 1000);
            add_peer(
                &mut state,
                n,
                P2pPeerStatus::Disconnected { time: at(1000) },
            );
        }
        fail(&mut state, 61, 6, 1000);
        add_peer(
            &mut state,
            61,
            P2pPeerStatus::Disconnected { time: at(1000) },
        );
        for n in 62..=80 {
            add_peer(&mut state, n, ready(at(0)));
        }
        for n in 81..=100 {
            add_peer(
                &mut state,
                n,
                P2pPeerStatus::Disconnected {
                    time: Timestamp::ZERO,
                },
            );
        }
        assert_eq!(state.peers.len(), 100);
        assert_eq!(state.peer_to_evict(at(2000)), Some(peer(61)));
        // Not within a minute of its last failure.
        assert_eq!(state.peer_to_evict(at(1030)), None);

        let discovered = |n: u8, a: SocketAddr| P2pPeerAction::Discovered {
            peer_id: peer(n),
            dial_opts: Some(P2pConnectionOutgoingInitOpts::LibP2P((peer(n), a).into())),
        };
        // A new peer at a new address: room is made.
        assert!(discovered(150, addr(150)).is_enabled(&state, at(2000)));
        assert!(!discovered(150, addr(150)).is_enabled(&state, at(1030)));
        // A peer we dialled before and forgot: no room for it.
        fail(&mut state, 151, 1, 1000);
        assert!(!discovered(151, addr(151)).is_enabled(&state, at(2000)));
        // A new peer id at an address we dialled: no room either.
        assert!(!discovered(152, addr(2)).is_enabled(&state, at(2000)));

        // Only failing, non-seed, libp2p peers go: none left, no room.
        for n in 2..=61 {
            state.peers.get_mut(&peer(n)).unwrap().status = ready(at(1500));
        }
        assert_eq!(state.peer_to_evict(at(2000)), None);
        assert!(!discovered(150, addr(150)).is_enabled(&state, at(2000)));
    }

    #[test]
    fn a_dial_that_fails_backs_off_from_the_error_timeout() {
        let mut state = state(&[]);
        state.record_libp2p_dial(&opts(1), at(0));
        state.record_libp2p_dial_end(peer(1), false, at(15));
        let entry = *state.dial_backoff.peer(&peer(1)).unwrap();
        assert_eq!(entry.failures, 1);
        assert!(entry.retry_at >= at(15 + 24) && entry.retry_at <= at(15 + 36));
        let (end, _) = state.dial_backoff.on_dial_failed(
            peer(1),
            entry.retry_at,
            &state.dial_backoff_config(),
        );
        assert_eq!(end, P2pDialEnd::Failed);
    }
}
