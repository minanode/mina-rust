//! Browser-to-browser sharing (MINAWEB-OPT-IN-SHARING). Off by default.
//!
//! A web node that opts in ([`P2pSharing::Peer`]) still dials only its seeds,
//! the relays. It uses the signalling discovery and exchange channels with
//! them alone: it asks them for an introduction while it has fewer than
//! `wanted_peers` browser peers, and accepts the browsers they introduce up to
//! `max_peers`. It introduces no one. It hands its WebRTC stack only the
//! candidates of an introduced browser's description that [`sdp`] allows.
//!
//! It serves bootstraps (snarked ledger queries, staged ledger, blocks) to at
//! most `max_bootstraps` browsers at once, counted from the requests it
//! serves: a browser it served such a request within [`SERVING_WINDOW`], or
//! is serving now, holds a slot, and another browser's request gets an empty
//! answer, which its node takes as "unavailable here" and asks elsewhere. A
//! peer's advertised best tip can't buy a slot: it can repeat a tip it never
//! verified. The tips only help pick whom to admit.
//!
//! A relay that opts in ([`P2pSharing::Relay`]) introduces such browsers to
//! each other and uses the channels for nothing else. Only a peer that opened
//! both channels and asked is ever introduced or offered as a target; a page
//! that did not opt in never opens them. On top of that the relay counts
//! introductions over a window, per peer key, per address (IPv4, or IPv6 /64)
//! and in total, never repeats a pair within the window, and hands each
//! requester the target it introduced least. libp2p peers never have these
//! channels.
//!
//! The relay also avoids pairing two pages that are both still syncing: a
//! page is syncing if its advertised tip is behind, or if it asked the relay
//! for bootstrap data within [`RELAY_SYNCING_WINDOW`]. That is best effort,
//! not a security boundary: a page can claim a tip it doesn't hold, and a
//! modified page can pose as synced to be paired with newcomers. The limits
//! above are what bound what such a page gets.

use std::{
    collections::{BTreeMap, VecDeque},
    net::IpAddr,
    time::Duration,
};

use redux::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    channels::{
        rpc::{P2pChannelsRpcState, P2pRpcRequest},
        streaming_rpc::{P2pChannelsStreamingRpcState, P2pStreamingRpcRemoteState},
    },
    connection::{incoming::IncomingSignalingMethod, outgoing::P2pConnectionOutgoingInitOpts},
    P2pState, PeerId,
};

pub mod sdp;

/// A sharing browser keeps serving a browser's bootstrap this long after
/// its last bootstrap request: a ledger sync asks many times a second.
pub const SERVING_WINDOW: Duration = Duration::from_secs(30);

/// A sharing relay counts a page as syncing this long after it last asked
/// the relay for bootstrap data: a ledger sync asks many times a second, and
/// a staged ledger download in flight counts by itself.
pub const RELAY_SYNCING_WINDOW: Duration = SERVING_WINDOW;

/// Most addresses a sharing relay keeps: connected peers' only. Beyond, a new
/// peer's address isn't kept and it isn't introduced.
pub const MAX_ADDRESSES: usize = 1024;

/// How often a sharing relay's timer lets waiting requests try again.
pub const RETRY_EVERY: Duration = Duration::from_secs(10);

/// The requests a bootstrap is made of, which cost the server most of its
/// upload: the snarked ledger, the staged ledger and the blocks.
pub fn is_bootstrap_request(request: &P2pRpcRequest) -> bool {
    matches!(
        request,
        P2pRpcRequest::LedgerQuery(..)
            | P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(_)
            | P2pRpcRequest::Block(_)
            | P2pRpcRequest::Catchup(_)
    )
}

/// How many blocks below our best tip a peer's best tip may be for the peer
/// to count as synced. A tab that is still syncing reports its root, hundreds
/// of blocks back.
pub const SHARING_SYNCED_WITHIN: u32 = 5;

/// Who shares what with browsers.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum P2pSharing {
    /// No sharing. The signalling channels are what `enabled_channels` says:
    /// closed by default, open to everyone with the web node's
    /// `peerDiscovery`.
    #[default]
    Off,
    /// A seed-only web node that shares with other browsers.
    Peer(P2pSharingPeerLimits),
    /// A relay that introduces sharing browsers to each other.
    Relay(P2pSharingRelayLimits),
}

impl P2pSharing {
    pub fn peer() -> Self {
        Self::Peer(Default::default())
    }

    pub fn relay() -> Self {
        Self::Relay(Default::default())
    }

    pub fn is_peer(&self) -> bool {
        matches!(self, Self::Peer(_))
    }

    pub fn is_relay(&self) -> bool {
        matches!(self, Self::Relay(_))
    }
}

/// A sharing browser's limits. WEB-P2P-TEST measured each steady browser link
/// at 25 to 35 MiB an hour in each direction, and each bootstrap served at up
/// to 130 MiB of upload, with no change in the serving tab's memory.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2pSharingPeerLimits {
    /// Most browser peers, in or out, besides the relays.
    pub max_peers: usize,
    /// Ask for introductions only while it has fewer browser peers.
    pub wanted_peers: usize,
    /// Most browser peers connecting or still syncing at once: bootstraps
    /// it may be serving.
    pub max_bootstraps: usize,
}

impl Default for P2pSharingPeerLimits {
    fn default() -> Self {
        Self {
            max_peers: 4,
            wanted_peers: 2,
            max_bootstraps: 2,
        }
    }
}

/// A sharing relay's introduction limits, over a sliding window. Each
/// introduction counts once for each side: its peer key and its address.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2pSharingRelayLimits {
    pub window: Duration,
    /// Introductions involving one peer key.
    pub per_peer: usize,
    /// Introductions involving one address: an IPv4 address or an IPv6 /64.
    pub per_address: usize,
    /// Introductions in all.
    pub total: usize,
}

impl Default for P2pSharingRelayLimits {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(60 * 60),
            per_peer: 6,
            per_address: 12,
            total: 120,
        }
    }
}

/// An address as the relay's limits count it: an IPv4 address, or an IPv6
/// /64, which one device or home usually holds whole.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum P2pAddressGroup {
    V4([u8; 4]),
    V6([u8; 8]),
}

impl From<IpAddr> for P2pAddressGroup {
    fn from(ip: IpAddr) -> Self {
        let ip = ip.to_canonical();
        match ip {
            IpAddr::V4(ip) => Self::V4(ip.octets()),
            IpAddr::V6(ip) => {
                let mut prefix = [0; 8];
                prefix.copy_from_slice(&ip.octets()[..8]);
                Self::V6(prefix)
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct P2pIntroduction {
    pub time: Timestamp,
    pub requester: PeerId,
    pub target: PeerId,
    pub requester_address: P2pAddressGroup,
    pub target_address: P2pAddressGroup,
}

impl P2pIntroduction {
    fn involves(&self, peer_id: &PeerId) -> bool {
        self.requester == *peer_id || self.target == *peer_id
    }

    fn involves_address(&self, address: &P2pAddressGroup) -> usize {
        usize::from(self.requester_address == *address)
            + usize::from(self.target_address == *address)
    }

    fn pairs(&self, a: &PeerId, b: &PeerId) -> bool {
        (self.requester == *a && self.target == *b) || (self.requester == *b && self.target == *a)
    }
}

/// What a sharing relay remembers.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct P2pSharingRelayState {
    /// Each incoming WebRTC peer's address, from its HTTP signalling request.
    pub addresses: BTreeMap<PeerId, P2pAddressGroup>,
    /// Introductions within the window, oldest first.
    pub recent: VecDeque<P2pIntroduction>,
    pub introduced_total: u64,
    /// Requests left waiting because the requester was at a limit.
    pub refused_total: u64,
    /// When the timer last let waiting requests try again, and whether this
    /// tick does.
    #[serde(default)]
    pub last_retry: Option<Timestamp>,
    #[serde(default)]
    pub retry_due: bool,
}

impl P2pSharingRelayState {
    fn recent_since(
        &self,
        now: Timestamp,
        window: Duration,
    ) -> impl Iterator<Item = &P2pIntroduction> {
        self.recent
            .iter()
            .filter(move |i| now.checked_sub(i.time).is_none_or(|age| age < window))
    }
}

/// Why a sharing relay doesn't introduce two peers now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum P2pIntroductionRefusal {
    #[display(fmt = "not a sharing relay")]
    NotRelay,
    #[display(fmt = "a peer did not come through HTTP signalling")]
    UnknownAddress,
    #[display(fmt = "the relay's total limit")]
    Total,
    #[display(fmt = "the peer's limit")]
    Peer,
    #[display(fmt = "the address's limit")]
    Address,
    #[display(fmt = "introduced to each other already")]
    Repeated,
    #[display(fmt = "both are still syncing")]
    BothSyncing,
}

impl P2pState {
    /// Whether the signalling channels may be used with `peer_id`, once
    /// `enabled_channels` has them. A sharing browser uses them with its
    /// seeds only.
    pub fn signaling_allowed_with(&self, peer_id: &PeerId) -> bool {
        match self.config.sharing {
            P2pSharing::Peer(_) => self.is_seed(peer_id),
            P2pSharing::Off | P2pSharing::Relay(_) => true,
        }
    }

    /// Our best tip's height, as we last sent it to a peer.
    pub fn own_best_tip_height(&self) -> Option<u32> {
        self.ready_peers_iter()
            .filter_map(|(_, p)| p.channels.best_tip.last_sent())
            .map(|tip| tip.height())
            .max()
    }

    /// A ready peer whose best tip is within [`SHARING_SYNCED_WITHIN`] blocks
    /// of `own`.
    pub fn peer_is_synced(&self, peer_id: &PeerId, own: Option<u32>) -> bool {
        let (Some(own), Some(peer)) = (own, self.get_ready_peer(peer_id)) else {
            return false;
        };
        peer.best_tip
            .as_ref()
            .is_some_and(|tip| tip.height().saturating_add(SHARING_SYNCED_WITHIN) >= own)
    }

    /// A sharing browser's browser peers (WebRTC, not seeds) connected or
    /// connecting, and how many of them look like bootstraps: connecting,
    /// behind by their advertised tip, or being served one now. This picks
    /// whom to admit; [`Self::sharing_serves`] enforces the cap.
    pub fn sharing_browser_peers(&self) -> (usize, usize) {
        let own = self.own_best_tip_height();
        self.peers
            .iter()
            .filter(|(id, p)| {
                !p.is_libp2p() && !self.is_seed(id) && p.status.is_connected_or_connecting()
            })
            .fold((0, 0), |(peers, bootstraps), (id, p)| {
                let bootstrap = p.status.as_ready().is_none()
                    || !self.peer_is_synced(id, own)
                    || self.bootstrap_in_flight(id);
                (peers + 1, bootstraps + usize::from(bootstrap))
            })
    }

    /// We are answering a bootstrap request from this peer now.
    fn bootstrap_in_flight(&self, peer_id: &PeerId) -> bool {
        let Some(peer) = self.get_ready_peer(peer_id) else {
            return false;
        };
        let streaming = matches!(
            peer.channels.streaming_rpc,
            P2pChannelsStreamingRpcState::Ready {
                remote: P2pStreamingRpcRemoteState::Requested { .. },
                ..
            }
        );
        let rpc = match &peer.channels.rpc {
            P2pChannelsRpcState::Ready { remote, .. } => remote
                .pending_requests
                .iter()
                .any(|r| is_bootstrap_request(&r.request)),
            _ => false,
        };
        streaming || rpc
    }

    /// We served this peer a bootstrap request within `window`, or are now.
    fn bootstrap_served_within(&self, peer_id: &PeerId, now: Timestamp, window: Duration) -> bool {
        self.bootstrap_in_flight(peer_id)
            || self
                .sharing_served
                .get(peer_id)
                .is_some_and(|at| now.checked_sub(*at).is_none_or(|age| age < window))
    }

    /// Whether a sharing browser answers a bootstrap request from `peer_id`
    /// now: always a seed's; a browser's while it is one of the at most
    /// `max_bootstraps` it serves. Anything but a sharing browser serves all.
    /// Call it before the request is recorded as pending.
    pub fn sharing_serves(&self, peer_id: &PeerId, now: Timestamp) -> bool {
        let P2pSharing::Peer(limits) = self.config.sharing else {
            return true;
        };
        if self.is_seed(peer_id) || self.bootstrap_served_within(peer_id, now, SERVING_WINDOW) {
            return true;
        }
        let served = self
            .peers
            .iter()
            .filter(|(id, p)| {
                *id != peer_id
                    && !p.is_libp2p()
                    && !self.is_seed(id)
                    && self.bootstrap_served_within(id, now, SERVING_WINDOW)
            })
            .count();
        served < limits.max_bootstraps
    }

    /// Records a bootstrap request we serve a WebRTC peer, sharing or as a
    /// sharing relay; nothing otherwise.
    pub fn sharing_record_served(&mut self, peer_id: PeerId, now: Timestamp) {
        let keep = match self.config.sharing {
            P2pSharing::Off => false,
            P2pSharing::Peer(_) => !self.is_seed(&peer_id),
            P2pSharing::Relay(_) => true,
        };
        if keep && !self.is_libp2p_peer(&peer_id) {
            self.sharing_served.insert(peer_id, now);
        }
    }

    /// Forgets the addresses and serving times of peers that left, and
    /// serving times past any window. Run from the timer.
    pub fn sharing_prune(&mut self, now: Timestamp) {
        if matches!(self.config.sharing, P2pSharing::Off) {
            return;
        }
        let peers = &self.peers;
        let here = |id: &PeerId| {
            peers
                .get(id)
                .is_some_and(|p| p.status.is_connected_or_connecting())
        };
        self.sharing_relay.addresses.retain(|id, _| here(id));
        self.sharing_served.retain(|id, at| {
            here(id)
                && now
                    .checked_sub(*at)
                    .is_none_or(|age| age < RELAY_SYNCING_WINDOW)
        });
        let relay = &mut self.sharing_relay;
        relay.retry_due = relay
            .last_retry
            .is_none_or(|at| now.checked_sub(at).is_none_or(|since| since >= RETRY_EVERY));
        if relay.retry_due {
            relay.last_retry = Some(now);
        }
    }

    /// A sharing relay's best-effort view: the page's advertised tip is
    /// behind, or it asked us for bootstrap data lately.
    fn relay_sees_syncing(&self, peer_id: &PeerId, own: Option<u32>, now: Timestamp) -> bool {
        !self.peer_is_synced(peer_id, own)
            || self.bootstrap_served_within(peer_id, now, RELAY_SYNCING_WINDOW)
    }

    /// A sharing relay with a request waiting and a target available: its
    /// timer runs the responder again every [`RETRY_EVERY`], for a request
    /// that waited at a limit or for a page to sync.
    pub fn sharing_relay_has_waiting_request(&self) -> bool {
        self.config.sharing.is_relay()
            && self.sharing_relay.retry_due
            && self
                .ready_peers_iter()
                .any(|(_, p)| p.channels.signaling.is_looking_for_peer())
            && self
                .ready_peers_iter()
                .any(|(_, p)| p.channels.signaling.is_looking_for_incoming_peer())
    }

    /// A sharing browser has room for one more browser peer.
    pub fn sharing_has_room(&self) -> bool {
        let P2pSharing::Peer(limits) = self.config.sharing else {
            return false;
        };
        let (peers, bootstraps) = self.sharing_browser_peers();
        peers < limits.max_peers && bootstraps < limits.max_bootstraps
    }

    /// Whether to ask a peer for an introduction (`Discover`).
    pub fn wants_introduction(&self) -> bool {
        match self.config.sharing {
            P2pSharing::Off => !self.already_has_min_peers(),
            P2pSharing::Peer(limits) => {
                self.sharing_browser_peers().0 < limits.wanted_peers && self.sharing_has_room()
            }
            P2pSharing::Relay(_) => false,
        }
    }

    /// Whether to tell a peer we take introduced offers (exchange `GetNext`).
    pub fn takes_introduced_offers(&self) -> bool {
        match self.config.sharing {
            P2pSharing::Off => !self.already_has_max_peers(),
            P2pSharing::Peer(_) => !self.already_has_max_peers() && self.sharing_has_room(),
            P2pSharing::Relay(_) => false,
        }
    }

    /// Whether we take a request to introduce a peer (discovery `GetNext`).
    pub fn introduces(&self) -> bool {
        !self.config.sharing.is_peer()
    }

    /// An incoming connection that a seed-only node accepts: a sharing
    /// browser's, introduced by a seed it is connected to, while it has room,
    /// with a description the [`sdp`] policy takes.
    pub fn sharing_accepts_incoming(&self, signaling: &IncomingSignalingMethod, sdp: &str) -> bool {
        match signaling {
            IncomingSignalingMethod::P2p { relay_peer_id } => {
                self.config.sharing.is_peer()
                    && self.is_seed(relay_peer_id)
                    && self.get_ready_peer(relay_peer_id).is_some()
                    && self.sharing_has_room()
                    && sdp::filter_sdp(sdp).is_ok()
            }
            IncomingSignalingMethod::Http => false,
        }
    }

    /// The remote description a sharing browser hands its WebRTC stack for a
    /// browser its seeds introduced (`relayed`): the [`sdp`] policy's copy,
    /// or why it is refused. `None`: hand it over as it came. The state keeps
    /// the description as it came, which the connection's authentication
    /// hashes.
    pub fn sharing_stack_sdp(
        &self,
        relayed: bool,
        sdp: &str,
    ) -> Result<Option<String>, sdp::SdpRefusal> {
        if self.config.sharing.is_peer() && relayed {
            sdp::filter_sdp(sdp).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Whether the sharing limits allow this dial now. They count only a
    /// sharing browser's dials to the browsers its seeds introduce; the
    /// configuration's allowlist ([`crate::P2pConfig::allows_dial`]) decides
    /// the rest.
    pub fn sharing_allows_dial(&self, opts: &P2pConnectionOutgoingInitOpts) -> bool {
        if !self.config.seed_only
            || !self.config.sharing.is_peer()
            || self.config.initial_peers.contains(opts)
        {
            return true;
        }
        opts.webrtc_p2p_relay_peer_id()
            .is_some_and(|relay| self.get_ready_peer(&relay).is_some())
            && self.sharing_has_room()
    }

    /// Keeps an incoming WebRTC peer's address, for a sharing relay's limits,
    /// and forgets those of peers that left.
    pub fn sharing_record_address(&mut self, peer_id: PeerId, ip: IpAddr) {
        if !self.config.sharing.is_relay() {
            return;
        }
        let peers = &self.peers;
        self.sharing_relay.addresses.retain(|id, _| {
            peers
                .get(id)
                .is_some_and(|p| p.status.is_connected_or_connecting())
        });
        if self.sharing_relay.addresses.len() >= MAX_ADDRESSES
            && !self.sharing_relay.addresses.contains_key(&peer_id)
        {
            return;
        }
        self.sharing_relay.addresses.insert(peer_id, ip.into());
    }

    /// The limits that concern `requester` alone: the relay's total, and the
    /// requester's key and address.
    pub fn sharing_requester_check(
        &self,
        requester: &PeerId,
        now: Timestamp,
    ) -> Result<(), P2pIntroductionRefusal> {
        let P2pSharing::Relay(limits) = self.config.sharing else {
            return Err(P2pIntroductionRefusal::NotRelay);
        };
        let relay = &self.sharing_relay;
        let address = relay
            .addresses
            .get(requester)
            .ok_or(P2pIntroductionRefusal::UnknownAddress)?;
        let recent = || relay.recent_since(now, limits.window);
        if recent().count() >= limits.total {
            return Err(P2pIntroductionRefusal::Total);
        }
        if recent().filter(|i| i.involves(requester)).count() >= limits.per_peer {
            return Err(P2pIntroductionRefusal::Peer);
        }
        if recent().map(|i| i.involves_address(address)).sum::<usize>() >= limits.per_address {
            return Err(P2pIntroductionRefusal::Address);
        }
        Ok(())
    }

    /// Whether a sharing relay may introduce `requester` to `target` now.
    pub fn sharing_introduction_check(
        &self,
        requester: &PeerId,
        target: &PeerId,
        now: Timestamp,
    ) -> Result<(), P2pIntroductionRefusal> {
        let P2pSharing::Relay(limits) = self.config.sharing else {
            return Err(P2pIntroductionRefusal::NotRelay);
        };
        self.sharing_requester_check(requester, now)?;
        let relay = &self.sharing_relay;
        let requester_address = relay.addresses.get(requester);
        let target_address = relay
            .addresses
            .get(target)
            .ok_or(P2pIntroductionRefusal::UnknownAddress)?;
        let recent = || relay.recent_since(now, limits.window);
        if recent().filter(|i| i.involves(target)).count() >= limits.per_peer {
            return Err(P2pIntroductionRefusal::Peer);
        }
        // Both sides' counts, with this introduction added.
        let same_address = usize::from(requester_address == Some(target_address));
        if recent()
            .map(|i| i.involves_address(target_address))
            .sum::<usize>()
            + same_address
            >= limits.per_address
        {
            return Err(P2pIntroductionRefusal::Address);
        }
        if recent().any(|i| i.pairs(requester, target)) {
            return Err(P2pIntroductionRefusal::Repeated);
        }
        let own = self.own_best_tip_height();
        if self.relay_sees_syncing(requester, own, now) && self.relay_sees_syncing(target, own, now)
        {
            return Err(P2pIntroductionRefusal::BothSyncing);
        }
        Ok(())
    }

    /// The target a sharing relay introduces `requester` to, among
    /// `candidates` (shuffled): one it may introduce, introduced least
    /// within the window.
    pub fn sharing_pick_target(
        &self,
        requester: &PeerId,
        candidates: impl IntoIterator<Item = PeerId>,
        now: Timestamp,
    ) -> Option<PeerId> {
        let P2pSharing::Relay(limits) = self.config.sharing else {
            return None;
        };
        candidates
            .into_iter()
            .filter(|target| {
                target != requester
                    && self
                        .sharing_introduction_check(requester, target, now)
                        .is_ok()
            })
            .min_by_key(|target| {
                self.sharing_relay
                    .recent_since(now, limits.window)
                    .filter(|i| i.involves(target))
                    .count()
            })
    }

    /// Records an introduction a sharing relay sends, and logs it.
    pub fn sharing_record_introduction(
        &mut self,
        requester: PeerId,
        target: PeerId,
        now: Timestamp,
    ) {
        let P2pSharing::Relay(limits) = self.config.sharing else {
            return;
        };
        let relay = &mut self.sharing_relay;
        let (Some(&requester_address), Some(&target_address)) = (
            relay.addresses.get(&requester),
            relay.addresses.get(&target),
        ) else {
            return;
        };
        while relay.recent.front().is_some_and(|i| {
            now.checked_sub(i.time)
                .is_some_and(|age| age >= limits.window)
        }) {
            relay.recent.pop_front();
        }
        relay.recent.push_back(P2pIntroduction {
            time: now,
            requester,
            target,
            requester_address,
            target_address,
        });
        relay.introduced_total = relay.introduced_total.saturating_add(1);
        mina_core::info!(now;
            kind = "P2pSharingIntroduction",
            summary = "introduced a sharing browser to another",
            requester = display(requester),
            target = display(target),
            in_window = relay.recent.len(),
            total = relay.introduced_total,
        );
    }

    /// Counts and logs a request a sharing relay leaves waiting because the
    /// requester is at a limit.
    pub fn sharing_record_refusal(
        &mut self,
        requester: PeerId,
        refusal: P2pIntroductionRefusal,
        now: Timestamp,
    ) {
        if !self.config.sharing.is_relay() {
            return;
        }
        self.sharing_relay.refused_total = self.sharing_relay.refused_total.saturating_add(1);
        mina_core::info!(now;
            kind = "P2pSharingIntroductionRefused",
            summary = "a sharing browser's request waits at a limit",
            requester = display(requester),
            reason = display(refusal),
            total = self.sharing_relay.refused_total,
        );
    }
}
