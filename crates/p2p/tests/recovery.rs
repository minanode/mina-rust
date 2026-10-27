//! Deterministic timer/reducer regressions. Transport success is supplied at
//! the FinalizeSuccess boundary; real Noise handshakes are covered in backoff.rs.
use mina_p2p::{
    bootstrap::P2pNetworkKadBootstrapState,
    channels::{
        best_tip::P2pChannelsBestTipAction,
        rpc::P2pChannelsRpcAction,
        signaling::{
            discovery::P2pChannelsSignalingDiscoveryAction,
            exchange::P2pChannelsSignalingExchangeAction,
        },
        snark::P2pChannelsSnarkAction,
        snark_job_commitment::P2pChannelsSnarkJobCommitmentAction,
        streaming_rpc::P2pChannelsStreamingRpcAction,
        transaction::P2pChannelsTransactionAction,
        P2pChannelsEffectfulAction,
    },
    connection::{
        incoming_effectful::P2pConnectionIncomingEffectfulAction,
        outgoing::P2pConnectionOutgoingAction,
        outgoing_effectful::P2pConnectionOutgoingEffectfulAction,
    },
    disconnection::P2pDisconnectionAction,
    disconnection_effectful::P2pDisconnectionEffectfulAction,
    identify::P2pIdentifyAction,
    network::identify::{
        stream_effectful::P2pNetworkIdentifyStreamEffectfulAction, P2pNetworkIdentifyState,
        P2pNetworkIdentifyStreamAction,
    },
    peer::P2pPeerAction,
    MioEvent, P2pAction, P2pEffectfulAction, P2pEvent, P2pNetworkKadBootstrapAction,
    P2pNetworkKadEffectfulAction, P2pNetworkKadRequestAction, P2pNetworkKademliaAction,
    P2pNetworkKademliaStreamAction, P2pNetworkSchedulerAction, P2pNetworkYamuxAction, P2pState,
    P2pStateTrait, PeerId,
};
use redux::{ActionMeta, EnablingCondition};

use mina_core::{impl_substate_access, Substate, MAINNET_CHAIN_ID};
use mina_p2p::{
    channels::ChannelId,
    connection::{
        outgoing::{P2pConnectionOutgoingError, P2pConnectionOutgoingInitOpts},
        P2pConnectionState,
    },
    identity::SecretKey,
    P2pCallbacks, P2pConfig, P2pMeshsubConfig, P2pPeerState, P2pPeerStatus, P2pPeerStatusReady,
};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    net::{Ipv4Addr, SocketAddr},
};
thread_local! { static TRACE: RefCell<Vec<P2pAction>> = const { RefCell::new(Vec::new()) }; }

#[path = "recovery/yamux_limits.rs"]
mod yamux_limits;

struct State(P2pState);
#[derive(Debug)]
enum Action {
    P2p(P2pAction),
    P2pEffectful(P2pEffectfulAction),
}
impl From<P2pAction> for Action {
    fn from(a: P2pAction) -> Self {
        Self::P2p(a)
    }
}
impl From<P2pEffectfulAction> for Action {
    fn from(a: P2pEffectfulAction) -> Self {
        Self::P2pEffectful(a)
    }
}
impl From<redux::AnyAction> for Action {
    fn from(a: redux::AnyAction) -> Self {
        Self::P2p(*a.0.downcast().unwrap())
    }
}
impl EnablingCondition<State> for Action {
    fn is_enabled(&self, s: &State, t: redux::Timestamp) -> bool {
        match self {
            Self::P2p(a) => a.is_enabled(&s.0, t),
            Self::P2pEffectful(a) => a.is_enabled(&s.0, t),
        }
    }
}
impl_substate_access!(State, P2pState, 0);

macro_rules! impl_p2p_state_access {
    ($state:ty, $substate_type:ty) => {
        impl mina_core::SubstateAccess<$substate_type> for $state {
            fn substate(&self) -> mina_core::SubstateResult<&$substate_type> {
                let substate: &P2pState = self.substate()?;
                substate.substate()
            }

            fn substate_mut(&mut self) -> mina_core::SubstateResult<&mut $substate_type> {
                let substate: &mut P2pState = self.substate_mut()?;
                substate.substate_mut()
            }
        }
    };
}

impl_p2p_state_access!(State, P2pNetworkIdentifyState);
impl_p2p_state_access!(State, mina_p2p::P2pNetworkState);
impl_p2p_state_access!(State, P2pNetworkKadBootstrapState);
impl_p2p_state_access!(State, mina_p2p::P2pNetworkKadState);
impl_p2p_state_access!(State, mina_p2p::P2pNetworkSchedulerState);
impl_p2p_state_access!(State, mina_p2p::P2pLimits);
impl_p2p_state_access!(State, mina_p2p::P2pNetworkPubsubState);
impl_p2p_state_access!(State, mina_p2p::P2pConfig);

impl P2pStateTrait for State {}

macro_rules! impl_from_p2p {
    ($sub_action:ty) => {
        impl From<$sub_action> for Action {
            fn from(value: $sub_action) -> Self {
                {
                    let action = P2pAction::from(value);
                    TRACE.with_borrow_mut(|trace| trace.push(action.clone()));
                    Self::P2p(action)
                }
            }
        }
    };
    (effectful $sub_action:ty) => {
        impl From<$sub_action> for Action {
            fn from(value: $sub_action) -> Self {
                Self::P2pEffectful(P2pEffectfulAction::from(value))
            }
        }
    };
}

impl_from_p2p!(P2pNetworkKademliaAction);
impl_from_p2p!(P2pNetworkKademliaStreamAction);
impl_from_p2p!(P2pNetworkKadRequestAction);
impl_from_p2p!(P2pNetworkKadBootstrapAction);
impl_from_p2p!(P2pPeerAction);
impl_from_p2p!(P2pNetworkYamuxAction);
impl_from_p2p!(P2pConnectionOutgoingAction);
impl_from_p2p!(P2pNetworkSchedulerAction);
impl_from_p2p!(P2pNetworkIdentifyStreamAction);
impl_from_p2p!(P2pIdentifyAction);
impl_from_p2p!(mina_p2p::P2pNetworkSelectAction);
impl_from_p2p!(mina_p2p::P2pNetworkPnetAction);
impl_from_p2p!(mina_p2p::P2pNetworkNoiseAction);
impl_from_p2p!(mina_p2p::connection::incoming::P2pConnectionIncomingAction);
impl_from_p2p!(mina_p2p::P2pNetworkPubsubAction);
impl_from_p2p!(P2pChannelsSignalingDiscoveryAction);
impl_from_p2p!(P2pChannelsSignalingExchangeAction);
impl_from_p2p!(P2pChannelsTransactionAction);
impl_from_p2p!(P2pChannelsSnarkAction);
impl_from_p2p!(mina_p2p::P2pNetworkRpcAction);
impl_from_p2p!(P2pChannelsRpcAction);
impl_from_p2p!(P2pDisconnectionAction);
impl_from_p2p!(P2pChannelsBestTipAction);
impl_from_p2p!(P2pChannelsSnarkJobCommitmentAction);
impl_from_p2p!(P2pChannelsStreamingRpcAction);

impl_from_p2p!(effectful P2pNetworkKadEffectfulAction);
impl_from_p2p!(effectful P2pConnectionIncomingEffectfulAction);
impl_from_p2p!(effectful mina_p2p::P2pNetworkSchedulerEffectfulAction);
impl_from_p2p!(effectful mina_p2p::P2pNetworkPnetEffectfulAction);
impl_from_p2p!(effectful mina_p2p::P2pNetworkPubsubEffectfulAction);
impl_from_p2p!(effectful P2pNetworkIdentifyStreamEffectfulAction);
impl_from_p2p!(effectful P2pConnectionOutgoingEffectfulAction);
impl_from_p2p!(effectful P2pDisconnectionEffectfulAction);
impl_from_p2p!(effectful P2pChannelsEffectfulAction);

impl mina_p2p::P2pActionTrait<State> for Action {}

fn peer(n: u8) -> PeerId {
    SecretKey::from_bytes([n; 32]).public_key().peer_id()
}
fn addr(n: u8) -> SocketAddr {
    (Ipv4Addr::new(10, 0, 0, n), 8302).into()
}
fn opts(n: u8) -> P2pConnectionOutgoingInitOpts {
    P2pConnectionOutgoingInitOpts::LibP2P((peer(n), addr(n)).into())
}
fn at(s: u64) -> redux::Timestamp {
    redux::Timestamp::new(1_000_000_000_000 + s * 1_000_000_000)
}
fn state(seeds: &[u8]) -> State {
    State(P2pState::new(
        P2pConfig {
            libp2p_port: None,
            listen_port: None,
            identity_pub_key: SecretKey::from_bytes([200; 32]).public_key(),
            initial_peers: seeds.iter().map(|n| opts(*n)).collect(),
            external_addrs: vec![],
            enabled_channels: ChannelId::iter_all().collect(),
            timeouts: Default::default(),
            limits: Default::default(),
            peer_discovery: true,
            meshsub: P2pMeshsubConfig::default(),
            catchup_rpc: Default::default(),
        },
        P2pCallbacks::default(),
        &MAINNET_CHAIN_ID,
    ))
}
fn apply(s: &mut State, a: impl Into<P2pAction>, t: u64) {
    let mut d = redux::Dispatcher::<Action, State>::new();
    P2pState::reducer(
        Substate::new(s, &mut d),
        ActionMeta::zero_custom(at(t)).with_action(a.into()),
    )
    .unwrap();
}
fn tick(s: &mut State, t: u64) -> Vec<P2pAction> {
    TRACE.with_borrow_mut(Vec::clear);
    let mut d = redux::Dispatcher::<Action, State>::new();
    P2pState::p2p_timeout_dispatch(Substate::new(s, &mut d), &ActionMeta::zero_custom(at(t)))
        .unwrap();
    TRACE.with_borrow(Clone::clone)
}

fn ready(s: &mut State, n: u8, t: u64) {
    for action in [
        P2pConnectionOutgoingAction::FinalizePending { peer_id: peer(n) },
        P2pConnectionOutgoingAction::FinalizeSuccess {
            peer_id: peer(n),
            remote_auth: None,
        },
        P2pConnectionOutgoingAction::Success { peer_id: peer(n) },
    ] {
        assert!(action.is_enabled(&s.0, at(t)), "{action:?}");
        apply(s, action, t);
    }
    let action = P2pPeerAction::Ready {
        peer_id: peer(n),
        incoming: false,
    };
    assert!(action.is_enabled(&s.0, at(t)));
    apply(s, action, t);
}

fn timer_dials(s: &mut State, t: u64) -> Vec<PeerId> {
    let mut dialled = Vec::new();
    for action in tick(s, t) {
        if let P2pAction::Connection(P2pConnectionStateAction::Outgoing(action)) = action {
            let opts = match &action {
                P2pConnectionOutgoingAction::Init { opts, .. }
                | P2pConnectionOutgoingAction::Reconnect { opts, .. }
                | P2pConnectionOutgoingAction::Recover { opts } => opts,
                _ => continue,
            };
            if action.is_enabled(&s.0, at(t)) {
                dialled.push(*opts.peer_id());
                apply(s, action, t);
            }
        }
    }
    dialled
}
use mina_p2p::connection::P2pConnectionAction as P2pConnectionStateAction;

#[test]
fn seed_only_timer_recovers_after_full_quarantine() {
    let mut s = state(&[1]);
    assert_eq!(timer_dials(&mut s, 0), vec![peer(1)]);
    apply(
        &mut s,
        P2pPeerAction::Stale {
            peer_id: peer(1),
            addr: addr(1),
            answered_by: peer(2),
        },
        1,
    );
    apply(
        &mut s,
        P2pConnectionOutgoingAction::Error {
            peer_id: peer(1),
            error: P2pConnectionOutgoingError::Timeout,
        },
        1,
    );
    assert!(s.0.peers[&peer(1)].dial_opts.is_none());
    for t in [16, 60, 1800, 3600] {
        assert!(timer_dials(&mut s, t).is_empty());
        assert!(s.0.peers[&peer(1)].dial_opts.is_none());
    }
    assert_eq!(s.0.ready_peers_iter().count(), 0);
    assert!(s
        .0
        .network
        .scheduler
        .discovery_state
        .as_ref()
        .unwrap()
        .routing_table
        .look_up(&mina_p2p::P2pNetworkKadKey::try_from(&peer(1)).unwrap())
        .is_none());
    assert_eq!(timer_dials(&mut s, 3601), vec![peer(1)]);
    assert_eq!(s.0.peers[&peer(1)].dial_opts, Some(opts(1)));
    ready(&mut s, 1, 3601);
    assert_eq!(s.0.ready_peers_iter().count(), 1);
}

fn outage() -> State {
    let mut s = state(&[1, 2, 3]);
    for n in 1..=3 {
        // All peers were connected, then all failed. Repeated outage retries
        // have reached the 8-12 minute exponential backoff.
        s.0.peers.insert(
            peer(n),
            P2pPeerState {
                is_libp2p: true,
                dial_opts: Some(opts(n)),
                status: P2pPeerStatus::Disconnected { time: at(6000) },
                identify: None,
            },
        );
        s.0.dial_backoff.on_ready(peer(n), false, at(0));
        s.0.record_libp2p_dial_end(peer(n), true, at(1));
        for t in [1000, 2000, 3000, 4000, 5000, 6000] {
            s.0.record_libp2p_dial(&opts(n), at(t));
            s.0.record_libp2p_dial_end(peer(n), false, at(t));
        }
        assert!(s.0.dial_backoff.retry_at(&peer(n), Some(addr(n))).unwrap() >= at(6480));
    }
    s
}

#[test]
fn correlated_outage_recovers_three_peers_in_45_seconds() {
    let mut s = outage();
    let mut times = Vec::new();
    for t in 6001..=6045 {
        for id in timer_dials(&mut s, t) {
            let n = (1..=3).find(|n| peer(*n) == id).unwrap();
            ready(&mut s, n, t);
            times.push(t - 6000);
        }
    }
    assert_eq!(times, vec![15, 30, 45]);
    assert_eq!(s.0.ready_peers_iter().count(), 3);
    assert_eq!(s.0.dial_backoff.redials_in_window(at(6045)), 6);
    // Existing three outage retries plus three recovery probes.
    assert!(s.0.recovery_candidate(at(6060)).is_none());
}

#[test]
fn recovery_rotates_failed_candidates_and_obeys_cap_and_quarantine() {
    let mut s = outage();
    let config = s.0.dial_backoff_config();
    for n in 1..=3 {
        s.0.dial_backoff
            .on_stale(peer(n), addr(n), peer(9), at(6000));
    }
    assert!(timer_dials(&mut s, 6015).is_empty());
    // Fresh outage: first peer fails again; the next probe must choose another.
    let mut s = outage();
    let first = timer_dials(&mut s, 6015)[0];
    apply(
        &mut s,
        P2pConnectionOutgoingAction::Error {
            peer_id: first,
            error: P2pConnectionOutgoingError::Timeout,
        },
        6015,
    );
    assert!(timer_dials(&mut s, 6029).is_empty());
    let second = timer_dials(&mut s, 6030)[0];
    assert_ne!(first, second);
    for _ in s.0.dial_backoff.redials_in_window(at(6030))..20 {
        s.0.dial_backoff.on_dial(peer(9), Some(addr(9)), at(6030));
    }
    // First dial of peer 9 was exempt; fill the remaining slot too.
    s.0.dial_backoff.on_dial(peer(9), Some(addr(9)), at(6030));
    assert!(s.0.dial_backoff.redials_in_window(at(6045)) >= config.max_redials_per_minute);
    assert!(timer_dials(&mut s, 6045).is_empty());
}

#[test]
fn mixed_native_node_keeps_webrtc_seed_reconnects_and_initial_dials() {
    let mut s = state(&[]);
    let web = P2pConnectionOutgoingInitOpts::WebRTC {
        peer_id: peer(1),
        signaling: "/http/127.0.0.1/4999".parse().unwrap(),
    };
    s.0.config.initial_peers = vec![web.clone()];
    for n in [2, 3, 4] {
        s.0.peers.insert(
            peer(n),
            P2pPeerState {
                is_libp2p: true,
                dial_opts: Some(opts(n)),
                status: P2pPeerStatus::Ready(Box::new(P2pPeerStatusReady::new(
                    false,
                    at(0),
                    &BTreeSet::new(),
                ))),
                identify: None,
            },
        );
    }
    assert_eq!(timer_dials(&mut s, 100), vec![peer(1)]);
    s.0.peers.get_mut(&peer(1)).unwrap().status = P2pPeerStatus::Disconnected { time: at(100) };
    assert!(s.0.reconnect_candidates(at(129)).next().is_none());
    assert_eq!(
        s.0.reconnect_candidates(at(131)).collect::<Vec<_>>(),
        vec![&web]
    );
    assert_eq!(timer_dials(&mut s, 131), vec![peer(1)]);
}

#[test]
fn kad_filters_stale_pairs_but_keeps_alternate_addresses_and_expiry() {
    let mut s = state(&[1]);
    apply(
        &mut s,
        P2pPeerAction::Stale {
            peer_id: peer(1),
            addr: addr(1),
            answered_by: peer(2),
        },
        0,
    );
    let stale: multiaddr::Multiaddr = "/ip4/10.0.0.1/tcp/8302".parse().unwrap();
    let alternate: multiaddr::Multiaddr = "/ip4/10.0.0.1/tcp/8303".parse().unwrap();
    let key = mina_p2p::P2pNetworkKadKey::try_from(&peer(1)).unwrap();
    for t in [10, 20, 3600] {
        let addrs = if t == 20 {
            vec![stale.clone(), alternate.clone()]
        } else {
            vec![stale.clone()]
        };
        let entry = mina_p2p::P2pNetworkKadEntry::new(peer(1), addrs).unwrap();
        apply(
            &mut s,
            P2pNetworkKademliaAction::UpdateFindNodeRequest {
                closest_peers: vec![entry],
                peer_id: peer(3),
                stream_id: 1,
                addr: mina_p2p::ConnectionAddr {
                    sock_addr: addr(3),
                    incoming: false,
                },
            },
            t,
        );
        let table =
            &s.0.network
                .scheduler
                .discovery_state
                .as_ref()
                .unwrap()
                .routing_table;
        if t == 10 {
            assert!(table.look_up(&key).is_none());
        }
        if t == 20 {
            assert_eq!(
                table.look_up(&key).unwrap().addresses(),
                &vec![alternate.clone()]
            );
        }
        if t == 3600 {
            assert!(table.look_up(&key).unwrap().addresses().contains(&stale));
        }
    }
}

#[test]
fn kad_routing_updates_filter_stale_addresses() {
    let mut s = state(&[1]);
    apply(
        &mut s,
        P2pPeerAction::Stale {
            peer_id: peer(1),
            addr: addr(1),
            answered_by: peer(2),
        },
        0,
    );
    let stale: multiaddr::Multiaddr = "/ip4/10.0.0.1/tcp/8302".parse().unwrap();
    let alternate: multiaddr::Multiaddr = "/ip4/10.0.0.2/tcp/8302".parse().unwrap();
    apply(
        &mut s,
        P2pNetworkKademliaAction::UpdateRoutingTable {
            peer_id: peer(1),
            addrs: vec![stale, alternate.clone()],
        },
        10,
    );
    let key = mina_p2p::P2pNetworkKadKey::try_from(&peer(1)).unwrap();
    assert_eq!(
        s.0.network
            .scheduler
            .discovery_state
            .as_ref()
            .unwrap()
            .routing_table
            .look_up(&key)
            .unwrap()
            .addresses(),
        &vec![alternate]
    );
}

#[test]
fn recovery_stops_at_floor_and_does_not_duplicate_inflight_dials() {
    let mut s = outage();
    let first = timer_dials(&mut s, 6015)[0];
    // A queued duplicate cannot reuse a consumed recovery slot.
    let first_opts = s.0.peers[&first].dial_opts.clone().unwrap();
    assert!(!P2pConnectionOutgoingAction::Recover { opts: first_opts }.is_enabled(&s.0, at(6015)));
    let next = s.0.recovery_candidate(at(6030)).unwrap();
    assert_ne!(*next.peer_id(), first);
    for n in [10, 11, 12] {
        s.0.peers.insert(
            peer(n),
            P2pPeerState {
                is_libp2p: true,
                dial_opts: Some(opts(n)),
                status: P2pPeerStatus::Ready(Box::new(P2pPeerStatusReady::new(
                    false,
                    at(6020),
                    &BTreeSet::new(),
                ))),
                identify: None,
            },
        );
    }
    assert!(s.0.recovery_candidate(at(6030)).is_none());
    assert!(timer_dials(&mut s, 6030).is_empty());
}

#[test]
fn seed_expiry_preserves_a_discovered_alternate_address() {
    let mut s = state(&[1]);
    timer_dials(&mut s, 0);
    apply(
        &mut s,
        P2pPeerAction::Stale {
            peer_id: peer(1),
            addr: addr(1),
            answered_by: peer(2),
        },
        1,
    );
    apply(
        &mut s,
        P2pConnectionOutgoingAction::Error {
            peer_id: peer(1),
            error: P2pConnectionOutgoingError::Timeout,
        },
        1,
    );
    let alternate = P2pConnectionOutgoingInitOpts::LibP2P((peer(1), addr(3)).into());
    apply(
        &mut s,
        P2pPeerAction::Discovered {
            peer_id: peer(1),
            dial_opts: Some(alternate.clone()),
        },
        2,
    );
    tick(&mut s, 3601);
    assert_eq!(s.0.peers[&peer(1)].dial_opts, Some(alternate));
}

// I18b: keep a target number of libp2p peers, newly discovered peers first,
// and keep Kademlia discovering.

fn insert(s: &mut State, n: u8, status: P2pPeerStatus) {
    s.0.peers.insert(
        peer(n),
        P2pPeerState {
            is_libp2p: true,
            dial_opts: Some(opts(n)),
            status,
            identify: None,
        },
    );
}

fn ready_since(t: u64) -> P2pPeerStatus {
    P2pPeerStatus::Ready(Box::new(P2pPeerStatusReady::new(
        false,
        at(t),
        &BTreeSet::new(),
    )))
}

/// Applies `a` and returns the actions its reducer dispatched.
fn apply_traced(s: &mut State, a: impl Into<P2pAction>, t: u64) -> Vec<P2pAction> {
    TRACE.with_borrow_mut(Vec::clear);
    apply(s, a, t);
    TRACE.with_borrow(Clone::clone)
}

fn has(actions: &[P2pAction], what: &str) -> bool {
    actions.iter().any(|a| format!("{a:?}").contains(what))
}

fn kad(s: &mut State) -> &mut mina_p2p::P2pNetworkKadState {
    s.0.network.scheduler.discovery_state.as_mut().unwrap()
}

fn lookup_keys(actions: &[P2pAction]) -> Vec<PeerId> {
    actions
        .iter()
        .filter_map(|a| match a {
            P2pAction::Network(mina_p2p::P2pNetworkAction::Kad(
                mina_p2p::P2pNetworkKadAction::System(P2pNetworkKademliaAction::StartBootstrap {
                    key,
                }),
            )) => Some(*key),
            _ => None,
        })
        .collect()
}

/// A lookup in progress with `peers` asked and `successful` requests done.
fn lookup_in_progress(s: &mut State, peers: &[u8], successful: usize, started: u64) {
    use mina_p2p::bootstrap::{
        P2pNetworkKadBoostrapRequestState, P2pNetworkKadBootstrapOngoingRequest,
        P2pNetworkKadBootstrapRequestStat,
    };
    let mut lookup = P2pNetworkKadBootstrapState::new(s.0.my_id()).unwrap();
    lookup.successful_requests = successful;
    lookup.started_at = Some(at(started));
    for n in peers {
        lookup.requests.insert(
            peer(*n),
            P2pNetworkKadBoostrapRequestState {
                addr: addr(*n),
                time: at(started),
                addrs_to_use: vec![addr(*n)],
            },
        );
        lookup
            .stats
            .requests
            .push(P2pNetworkKadBootstrapRequestStat::Ongoing(
                P2pNetworkKadBootstrapOngoingRequest {
                    peer_id: peer(*n),
                    address: opts(*n),
                    start: at(started),
                },
            ));
    }
    kad(s).status = mina_p2p::P2pNetworkKadStatus::Bootstrapping(lookup);
}

/// The m8 situation: a few peers ready, the peers that failed before due
/// again, and newly discovered peers. The timer dials the new ones first,
/// all of them before any peer that failed before, never a seed, only as
/// many at a time as the target lacks, and stops at the target.
#[test]
fn the_timer_reaches_the_target_dialling_new_peers_first() {
    let mut s = state(&[1]);
    // The seed and three other peers are ready.
    for n in 1..=4 {
        insert(&mut s, n, ready_since(0));
    }
    // Thirty peers failed once at the start and are due again.
    for n in 10..40 {
        insert(&mut s, n, P2pPeerStatus::Disconnected { time: at(0) });
        s.0.record_libp2p_dial(&opts(n), at(0));
        s.0.record_libp2p_dial_end(peer(n), false, at(0));
    }
    // Forty peers were just discovered, as a Kademlia reply reports them.
    for n in 50..90 {
        let action = P2pPeerAction::Discovered {
            peer_id: peer(n),
            dial_opts: Some(opts(n)),
        };
        assert!(action.is_enabled(&s.0, at(100)));
        apply(&mut s, action, 100);
    }
    // A quarter of the peers accept us; the others fail.
    let mut dials = Vec::new();
    let mut reached = None;
    for t in 100..400 {
        let ticked = timer_dials(&mut s, t);
        assert!(
            ticked.len() <= 16,
            "{} dials at {t}: more than the target lacks",
            ticked.len()
        );
        for id in ticked {
            let n = (1..=90u8).find(|n| peer(*n) == id).unwrap();
            assert!(
                reached.is_none(),
                "dialled {n} at {t} after reaching the target"
            );
            dials.push((t, n));
            if n % 4 == 0 {
                ready(&mut s, n, t);
            } else {
                apply(
                    &mut s,
                    P2pConnectionOutgoingAction::Error {
                        peer_id: id,
                        error: P2pConnectionOutgoingError::Timeout,
                    },
                    t,
                );
            }
        }
        if reached.is_none() && s.0.libp2p_ready_peers_count() >= 20 {
            reached = Some(t);
        }
    }
    let reached = reached.expect("the target is reached");
    assert_eq!(s.0.libp2p_ready_peers_count(), 20);
    // The first 16 dials, all new peers, went out at once.
    assert_eq!(dials.iter().filter(|(t, _)| *t == 100).count(), 16);
    // Every new peer before any peer that failed before.
    let first_old = dials
        .iter()
        .position(|(_, n)| (10..40).contains(n))
        .unwrap();
    assert_eq!(first_old, 40, "{dials:?}");
    assert_eq!(
        dials[..40].iter().map(|(_, n)| *n).collect::<BTreeSet<_>>(),
        (50..90).collect::<BTreeSet<_>>()
    );
    // Never the seed.
    assert!(!dials.iter().any(|(_, n)| *n == 1));
    // Redials stay within I18's cap, all dials within the overall cap.
    let mut seen = BTreeSet::new();
    let redial_times = dials
        .iter()
        .filter(|(_, n)| (10..40).contains(n) || !seen.insert(*n))
        .map(|(t, _)| *t)
        .collect::<Vec<_>>();
    for start in 100u64..400 {
        let window = start..start + 60;
        assert!(redial_times.iter().filter(|t| window.contains(*t)).count() <= 20);
        assert!(dials.iter().filter(|(t, _)| window.contains(t)).count() <= 100);
    }
    assert!(reached < 250, "reached the target at {reached}");
}

/// A lookup ends once it has had 20 successful requests and its last
/// request ends. Before I18b nothing ended it: Kademlia stayed
/// `Bootstrapping` and never looked up again.
#[test]
fn a_kademlia_lookup_ends_after_twenty_successful_requests() {
    let mut s = state(&[1]);
    lookup_in_progress(&mut s, &[2, 3], 19, 0);
    let done = |n: u8| P2pNetworkKadBootstrapAction::RequestDone {
        peer_id: peer(n),
        closest_peers: Default::default(),
    };
    // The 20th: no more requests, but one is still in flight.
    let actions = apply_traced(&mut s, done(2), 10);
    assert!(!has(&actions, "CreateRequests"), "{actions:?}");
    assert!(!has(&actions, "BootstrapFinished"), "{actions:?}");
    // The last one in flight ends it.
    let actions = apply_traced(&mut s, done(3), 11);
    assert!(has(&actions, "BootstrapFinished"), "{actions:?}");
    apply(&mut s, P2pNetworkKademliaAction::BootstrapFinished, 11);
    assert!(kad(&mut s).is_bootstrapped());
    assert!(kad(&mut s).has_bootstrapped);

    // A failure after the 20th success ends it too.
    lookup_in_progress(&mut s, &[4], 20, 20);
    let actions = apply_traced(
        &mut s,
        P2pNetworkKadBootstrapAction::RequestError {
            peer_id: peer(4),
            error: "refused".to_owned(),
        },
        21,
    );
    assert!(has(&actions, "BootstrapFinished"), "{actions:?}");

    // Below 20 successes it asks more peers, as before.
    lookup_in_progress(&mut s, &[5], 3, 30);
    let actions = apply_traced(&mut s, done(5), 31);
    assert!(has(&actions, "CreateRequests"), "{actions:?}");
    assert!(!has(&actions, "BootstrapFinished"), "{actions:?}");
}

/// A request whose peer never answers has no timeout of its own: the lookup
/// is ended after two minutes, with its requests, so that discovery goes on.
#[test]
fn a_kademlia_lookup_that_hangs_is_ended() {
    let mut s = state(&[1]);
    lookup_in_progress(&mut s, &[2], 5, 0);
    let my_id = s.0.my_id();
    kad(&mut s).create_request(addr(2), peer(2), my_id).unwrap();
    let actions = tick(&mut s, 119);
    assert!(!has(&actions, "BootstrapFinished"), "{actions:?}");
    let actions = tick(&mut s, 120);
    assert!(has(&actions, "BootstrapFinished"), "{actions:?}");
    assert!(
        has(&actions, "Kademlia lookup timed out"),
        "the hung request is ended: {actions:?}"
    );
}

/// The first lookup is for our own id, whatever the peer count. Later ones
/// start while we have fewer ready libp2p peers than the target, each for
/// a random id, and not at the target.
#[test]
fn kademlia_looks_up_again_only_below_the_target() {
    let mut s = state(&[1]);
    let my_id = s.0.my_id();
    for n in 2..=21 {
        insert(&mut s, n, ready_since(0));
    }
    assert_eq!(lookup_keys(&tick(&mut s, 0)), vec![my_id]);

    kad(&mut s).status = mina_p2p::P2pNetworkKadStatus::Bootstrapped {
        time: at(0),
        stats: Default::default(),
    };
    kad(&mut s).has_bootstrapped = true;
    // At the target: no lookup.
    assert!(lookup_keys(&tick(&mut s, 100)).is_empty());
    // Below it: a lookup for a random id, another one each time.
    for n in 2..=6 {
        s.0.peers.get_mut(&peer(n)).unwrap().status = P2pPeerStatus::Disconnected { time: at(100) };
    }
    let first = lookup_keys(&tick(&mut s, 101));
    let second = lookup_keys(&tick(&mut s, 102));
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_ne!(first[0], my_id);
    assert_ne!(first[0], second[0]);
}

/// Lookups keep I18's seed policy now that they repeat: a seed is dialled
/// for one only while we want seed connections. And after the first
/// lookup they dial only while we are short of the target.
#[test]
fn kademlia_lookups_dial_seeds_and_new_peers_only_when_wanted() {
    let mut s = state(&[1]);
    let my_id = s.0.my_id();
    let new = |n: u8| P2pNetworkKadRequestAction::New {
        peer_id: peer(n),
        addr: addr(n),
        key: my_id,
    };
    let dials = |actions: &[P2pAction], n: u8| {
        actions.iter().any(|a| {
            matches!(
                a,
                P2pAction::Connection(P2pConnectionStateAction::Outgoing(
                    P2pConnectionOutgoingAction::Init { opts, .. }
                )) if *opts.peer_id() == peer(n)
            )
        })
    };
    // No other peer: the seed is dialled.
    let actions = apply_traced(&mut s, new(1), 0);
    assert!(dials(&actions, 1), "{actions:?}");
    apply(
        &mut s,
        P2pNetworkKadRequestAction::Prune { peer_id: peer(1) },
        0,
    );

    // Three other peers ready: not the seed.
    for n in 2..=4 {
        insert(&mut s, n, ready_since(0));
    }
    let actions = apply_traced(&mut s, new(1), 1);
    assert!(!dials(&actions, 1), "{actions:?}");
    assert!(has(&actions, "not dialling a seed"), "{actions:?}");
    apply(
        &mut s,
        P2pNetworkKadRequestAction::Prune { peer_id: peer(1) },
        1,
    );

    // Before the first lookup ended, new peers are dialled whatever the
    // count.
    for n in 5..=21 {
        insert(&mut s, n, ready_since(0));
    }
    assert_eq!(s.0.libp2p_dial_deficit(), 0);
    let actions = apply_traced(&mut s, new(50), 2);
    assert!(dials(&actions, 50), "{actions:?}");

    // After it, only while short of the target.
    kad(&mut s).has_bootstrapped = true;
    let actions = apply_traced(&mut s, new(51), 3);
    assert!(!dials(&actions, 51), "{actions:?}");
    assert!(has(&actions, "not dialling: enough peers"), "{actions:?}");
    for n in 5..=10 {
        s.0.peers.remove(&peer(n));
    }
    let actions = apply_traced(&mut s, new(52), 4);
    assert!(dials(&actions, 52), "{actions:?}");
}

/// A full peer table forgets its worst failing peer for a newly discovered
/// one; before I18b discovery stopped there.
#[test]
fn a_discovered_peer_takes_the_place_of_the_worst_failing_one() {
    let mut s = state(&[]);
    for n in 1..=100u8 {
        insert(&mut s, n, P2pPeerStatus::Disconnected { time: at(0) });
        let failures = if n == 77 { 5 } else { 1 };
        for _ in 0..failures {
            s.0.record_libp2p_dial(&opts(n), at(0));
            s.0.record_libp2p_dial_end(peer(n), false, at(0));
        }
    }
    let action = P2pPeerAction::Discovered {
        peer_id: peer(150),
        dial_opts: Some(opts(150)),
    };
    assert!(action.is_enabled(&s.0, at(1000)));
    apply(&mut s, action, 1000);
    assert_eq!(s.0.peers.len(), 100);
    assert!(s.0.peers.contains_key(&peer(150)));
    assert!(!s.0.peers.contains_key(&peer(77)));
    // Its backoff is remembered: rediscovered, it is not new and makes no
    // room.
    assert!(s.0.dial_backoff.peer(&peer(77)).is_some());
    assert!(!P2pPeerAction::Discovered {
        peer_id: peer(77),
        dial_opts: Some(opts(77)),
    }
    .is_enabled(&s.0, at(1000)));
}

/// A connection that ends keeps the table small by forgetting a peer, but
/// not one we discovered and never tried: those wait with the time zero, so
/// they used to be the first to go.
#[test]
fn a_disconnection_does_not_forget_peers_we_never_tried() {
    let mut s = state(&[]);
    for n in 2..=56 {
        insert(
            &mut s,
            n,
            P2pPeerStatus::Disconnected {
                time: redux::Timestamp::ZERO,
            },
        );
    }
    // Three peers that had healthy connections, ended at 100, 200 and 300 s.
    for (n, t) in [(57, 100), (58, 200), (59, 300)] {
        s.0.record_libp2p_dial(&opts(n), at(0));
        s.0.dial_backoff.on_ready(peer(n), false, at(0));
        s.0.record_libp2p_dial_end(peer(n), true, at(t));
        insert(&mut s, n, P2pPeerStatus::Disconnected { time: at(t) });
    }
    insert(&mut s, 60, ready_since(0));
    s.0.record_libp2p_dial(&opts(60), at(0));
    s.0.dial_backoff.on_ready(peer(60), false, at(0));
    assert!(s.0.peers.len() > 50);
    let removed = |actions: &[P2pAction]| {
        actions
            .iter()
            .filter_map(|a| match a {
                P2pAction::Peer(P2pPeerAction::Remove { peer_id }) => Some(*peer_id),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let actions = apply_traced(
        &mut s,
        P2pDisconnectionAction::Finish { peer_id: peer(60) },
        1000,
    );
    assert_eq!(removed(&actions), vec![peer(57)]);

    // A peer that keeps failing goes before them.
    for _ in 0..3 {
        s.0.record_libp2p_dial(&opts(58), at(400));
        s.0.record_libp2p_dial_end(peer(58), false, at(400));
    }
    insert(&mut s, 61, ready_since(0));
    let actions = apply_traced(
        &mut s,
        P2pDisconnectionAction::Finish { peer_id: peer(61) },
        1100,
    );
    assert_eq!(removed(&actions), vec![peer(58)]);
}

// The Kademlia lookup's request bookkeeping (I18b review), through a real
// store: dispatch there is depth first, as in the node, so a request refused
// at once ends inside the batch of requests that made it.

thread_local! {
    static STORE_TRACE: RefCell<Vec<P2pAction>> = const { RefCell::new(Vec::new()) };
    static STORE_ERRORS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

struct KadService;

impl redux::TimeService for KadService {}

impl mina_p2p::P2pNetworkService for KadService {
    fn resolve_name(
        &mut self,
        _host: &str,
    ) -> Result<Vec<std::net::IpAddr>, mina_p2p::P2pNetworkServiceError> {
        Ok(vec![])
    }

    fn detect_local_ip(
        &mut self,
    ) -> Result<Vec<std::net::IpAddr>, mina_p2p::P2pNetworkServiceError> {
        Ok(vec![])
    }
}

type KadStore = redux::Store<State, KadService, Action>;

impl redux::SubStore<State, P2pState> for KadStore {
    type SubAction = P2pAction;
    type Service = KadService;

    fn state(&self) -> &P2pState {
        &self.state.get().0
    }

    fn service(&mut self) -> &mut KadService {
        &mut self.service
    }

    fn state_and_service(&mut self) -> (&P2pState, &mut KadService) {
        (&self.state.get().0, &mut self.service)
    }

    fn dispatch<A>(&mut self, action: A) -> bool
    where
        A: Into<P2pAction> + EnablingCondition<P2pState>,
    {
        self.sub_dispatch(action)
    }

    fn dispatch_callback<T>(&mut self, callback: redux::Callback<T>, args: T) -> bool
    where
        T: 'static,
        P2pAction: From<redux::AnyAction> + EnablingCondition<P2pState>,
    {
        KadStore::dispatch_callback(self, callback, args)
    }
}

/// The P2P reducers, recording what they reduce and the errors they return.
fn kad_store_reducer(
    state: &mut State,
    action: &redux::ActionWithMeta<Action>,
    dispatcher: &mut redux::Dispatcher<Action, State>,
) {
    if let Action::P2p(a) = action.action() {
        STORE_TRACE.with_borrow_mut(|trace| trace.push(a.clone()));
        let meta = action.meta().clone();
        if let Err(error) = P2pState::reducer(
            Substate::new(state, dispatcher),
            meta.with_action(a.clone()),
        ) {
            STORE_ERRORS.with_borrow_mut(|errors| errors.push(error));
        }
    }
}

/// Kademlia's effects only: `MakeRequest` turns a routing table entry into
/// `AppendRequest`. Dials go nowhere, so a request that is not refused stays
/// in flight.
fn kad_store_effects(store: &mut KadStore, action: redux::ActionWithMeta<Action>) {
    let (action, meta) = action.split();
    if let Action::P2pEffectful(P2pEffectfulAction::Network(
        mina_p2p::P2pNetworkEffectfulAction::Kad(action),
    )) = action
    {
        action.effects(&meta, store);
    }
}

fn kad_of(store: &KadStore) -> &mina_p2p::P2pNetworkKadState {
    store.state().0.network.scheduler.discovery_state().unwrap()
}

/// Peer id to the kinds of its request records in the lookup's statistics.
fn request_records(
    stats: &mina_p2p::bootstrap::P2pNetworkKadBootstrapStats,
) -> std::collections::BTreeMap<PeerId, Vec<&'static str>> {
    use mina_p2p::bootstrap::P2pNetworkKadBootstrapRequestStat as Stat;
    let mut records = std::collections::BTreeMap::<PeerId, Vec<&'static str>>::new();
    for record in &stats.requests {
        let (peer_id, kind) = match record {
            Stat::Ongoing(r) => (r.peer_id, "ongoing"),
            Stat::Successful(r) => (r.peer_id, "successful"),
            Stat::Failed(r) => (r.peer_id, "failed"),
        };
        records.entry(peer_id).or_default().push(kind);
    }
    records
}

/// A seed among the first three peers a lookup asks is refused at once, by
/// I18's seed policy, while the batch of ten requests is being assembled.
/// The lookup used to start another batch there, which asked again the
/// seven peers the first had yet to append and drained `requests_number`
/// under it; the first batch's appends then took the counter below zero (a
/// panic with overflow checks, a wrap without, after which its last peer
/// was never finalized) and asked those seven peers a second time. Now
/// each peer is asked once, the batch completes, and the lookup ends when
/// its last request does.
#[test]
fn a_refusal_inside_a_batch_of_kademlia_requests_asks_no_peer_twice() {
    let mut s = state(&[]);
    // Private test addresses, so that requests are made at all.
    kad(&mut s).filter_addrs = false;
    let routing = (10..20u8).collect::<Vec<_>>();
    for n in &routing {
        let multiaddr: multiaddr::Multiaddr = format!("/ip4/10.0.0.{n}/tcp/8302").parse().unwrap();
        let entry = mina_p2p::P2pNetworkKadEntry::new(peer(*n), vec![multiaddr]).unwrap();
        assert!(kad(&mut s).routing_table.insert(entry).unwrap());
    }
    // The peer a lookup for our own id asks first is a seed, and three other
    // peers are ready, so the seed is not dialled.
    let my_id = s.0.my_id();
    let my_key = mina_p2p::P2pNetworkKadKey::try_from(&my_id).unwrap();
    let first = kad(&mut s)
        .routing_table
        .closest_peers(&my_key)
        .next()
        .unwrap()
        .peer_id;
    let seed = *routing.iter().find(|n| peer(**n) == first).unwrap();
    s.0.config.initial_peers = vec![opts(seed)];
    for n in 1..=3 {
        insert(&mut s, n, ready_since(0));
    }
    assert!(s.0.is_seed(&peer(seed)) && !s.0.wants_seed_connections());

    STORE_TRACE.with_borrow_mut(Vec::clear);
    STORE_ERRORS.with_borrow_mut(Vec::clear);
    let mut store = KadStore::new(
        kad_store_reducer,
        kad_store_effects,
        KadService,
        std::time::SystemTime::now(),
        s,
    );
    assert!(store.sub_dispatch(P2pNetworkKademliaAction::StartBootstrap { key: my_id }));

    let trace = STORE_TRACE.with_borrow(Clone::clone);
    let refused = trace.iter().any(|a| {
        matches!(
            a,
            P2pAction::Network(mina_p2p::P2pNetworkAction::Kad(
                mina_p2p::P2pNetworkKadAction::Bootstrap(
                    P2pNetworkKadBootstrapAction::RequestError { peer_id, error }
                ),
            )) if *peer_id == peer(seed) && error.contains("not dialling a seed")
        )
    });
    assert!(refused, "the seed is refused at once: {trace:?}");

    // The batch is complete: nothing left to append or to finalize.
    let lookup = kad_of(&store).bootstrap_state().expect("still looking up");
    assert_eq!(lookup.requests_number, 0);
    assert!(lookup.peer_id_req_vec.is_empty());
    assert!(!lookup.is_assembling());
    // Each peer asked once: one record each, the seed's failed, the other
    // nine in flight.
    let records = request_records(&lookup.stats);
    assert_eq!(records.len(), 10, "{records:?}");
    for n in &routing {
        let expected = if *n == seed {
            vec!["failed"]
        } else {
            vec!["ongoing"]
        };
        assert_eq!(records[&peer(*n)], expected, "peer {n}: {records:?}");
        assert_eq!(lookup.requests.contains_key(&peer(*n)), *n != seed);
    }
    // Each of the nine dialled once, and no request created twice.
    for n in routing.iter().filter(|n| **n != seed) {
        let dials = trace
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    P2pAction::Connection(P2pConnectionStateAction::Outgoing(
                        P2pConnectionOutgoingAction::Init { opts, .. }
                    )) if *opts.peer_id() == peer(*n)
                )
            })
            .count();
        assert_eq!(dials, 1, "peer {n}");
    }
    assert_eq!(STORE_ERRORS.with_borrow(Clone::clone), Vec::<String>::new());

    // The nine fail in turn; the last one ends the lookup, with no record
    // left ongoing.
    for n in routing.iter().filter(|n| **n != seed) {
        assert!(kad_of(&store).bootstrap_state().is_some(), "peer {n}");
        assert!(store.sub_dispatch(P2pNetworkKadRequestAction::Error {
            peer_id: peer(*n),
            error: "test".to_owned(),
        }));
    }
    let kad = kad_of(&store);
    assert!(kad.is_bootstrapped());
    let records = request_records(kad.bootstrap_stats().unwrap());
    assert_eq!(records.len(), 10);
    assert!(
        records.values().all(|kinds| kinds == &vec!["failed"]),
        "{records:?}"
    );
    assert_eq!(STORE_ERRORS.with_borrow(Clone::clone), Vec::<String>::new());
}
