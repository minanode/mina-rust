//! MINAWEB-OPT-IN-SHARING: the sharing browser's boundary and caps, the
//! sharing relay's introductions, and that sharing off is the PR 173
//! boundary. Reducers and enabling conditions only.
use super::*;
use binprot::BinProtRead;
use mina_core::block::{ArcBlockWithHash, BlockWithHash};
use mina_p2p::{
    channels::{
        best_tip::P2pChannelsBestTipState,
        signaling::{
            discovery::{P2pChannelsSignalingDiscoveryState, SignalingDiscoveryState},
            exchange::{P2pChannelsSignalingExchangeState, SignalingExchangeState},
        },
    },
    connection::{
        incoming::{
            IncomingSignalingMethod, P2pConnectionIncomingAction, P2pConnectionIncomingInitOpts,
        },
        RejectionReason,
    },
    sharing::P2pIntroduction,
    webrtc::SignalingMethod,
    P2pSharing, P2pSharingRelayLimits,
};
use mina_p2p_messages::{list::List, rpc_kernel::ResponsePayload, v2::MinaBlockBlockStableV2};
use std::sync::Arc;

/// A real block, at `height`.
fn block_at(height: u32) -> ArcBlockWithHash {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../p2p-messages/tests/files/mesa/get_transition_chain_v2_563900_zkapp.bin"
    );
    let bytes = std::fs::read(path).unwrap();
    let payload =
        ResponsePayload::<Option<List<MinaBlockBlockStableV2>>>::binprot_read(&mut &bytes[..])
            .unwrap();
    let mut block = payload.0.unwrap().0.unwrap().iter().next().unwrap().clone();
    block
        .header
        .protocol_state
        .body
        .consensus_state
        .blockchain_length = height.into();
    BlockWithHash::try_new(Arc::new(block)).unwrap()
}

/// Our best tip is at this height.
const TIP: u32 = 583_189;

/// A ready WebRTC peer, with the signalling channels ready and asking
/// (`asks`: an introduction; `takes`: introduced offers) or not opened at all
/// (`opened` false), its best tip at `height`, and ours sent to it.
fn webrtc_peer(n: u8, height: Option<u32>, opened: bool, asks: bool, takes: bool) -> P2pPeerState {
    let mut ready = P2pPeerStatusReady::new(false, at(0), &ChannelId::iter_all().collect());
    if opened {
        ready.channels.signaling.discovery = P2pChannelsSignalingDiscoveryState::Ready {
            time: at(0),
            local: if asks {
                SignalingDiscoveryState::DiscoveryRequested { time: at(0) }
            } else {
                SignalingDiscoveryState::WaitingForRequest { time: at(0) }
            },
            remote: SignalingDiscoveryState::WaitingForRequest { time: at(0) },
        };
        ready.channels.signaling.exchange = P2pChannelsSignalingExchangeState::Ready {
            time: at(0),
            local: SignalingExchangeState::WaitingForRequest { time: at(0) },
            remote: if takes {
                SignalingExchangeState::Requested { time: at(0) }
            } else {
                SignalingExchangeState::WaitingForRequest { time: at(0) }
            },
        };
    }
    ready.channels.best_tip = P2pChannelsBestTipState::Ready {
        time: at(0),
        local: mina_p2p::channels::best_tip::BestTipPropagationState::WaitingForRequest {
            time: at(0),
        },
        remote: mina_p2p::channels::best_tip::BestTipPropagationState::Responded { time: at(0) },
        last_sent: Some(block_at(TIP)),
        last_received: None,
    };
    ready.best_tip = height.map(block_at);
    P2pPeerState {
        is_libp2p: false,
        dial_opts: Some(web_opts(n, 9360)),
        status: P2pPeerStatus::Ready(Box::new(ready)),
        identify: None,
    }
}

fn connecting_browser(n: u8) -> P2pPeerState {
    let opts = relayed(n, 1);
    P2pPeerState {
        is_libp2p: false,
        dial_opts: None,
        status: P2pPeerStatus::Connecting(P2pConnectionState::outgoing_init(&opts)),
        identify: None,
    }
}

/// A dial to browser `n`, introduced by `relay`.
fn relayed(n: u8, relay: u8) -> P2pConnectionOutgoingInitOpts {
    P2pConnectionOutgoingInitOpts::WebRTC {
        peer_id: peer(n),
        signaling: SignalingMethod::P2p {
            relay_peer_id: peer(relay),
        },
    }
}

fn dial(opts: P2pConnectionOutgoingInitOpts) -> P2pConnectionOutgoingAction {
    P2pConnectionOutgoingAction::Init {
        opts,
        rpc_id: None,
        on_success: None,
    }
}

/// A browser's description: an mDNS host and a public reflexive candidate.
const HONEST_SDP: &str = "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n\
m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 0.0.0.0\r\n\
a=candidate:1 1 udp 2122260223 2e5a5b7c-1c2f-4d7e-9f4b-123456789abc.local 54321 typ host\r\n\
a=candidate:2 1 udp 1686052607 8.8.4.4 54321 typ srflx raddr 0.0.0.0 rport 0\r\n";

fn offer_from(s: &State, n: u8) -> Box<mina_p2p::webrtc::Offer> {
    Box::new(mina_p2p::webrtc::Offer {
        sdp: HONEST_SDP.to_owned(),
        chain_id: MAINNET_CHAIN_ID.clone(),
        identity_pub_key: SecretKey::from_bytes([n; 32]).public_key(),
        target_peer_id: s.0.my_id(),
        host: "127.0.0.1".parse().unwrap(),
        listen_port: None,
        max_rpc_requests: None,
    })
}

fn incoming(s: &State, n: u8, signaling: IncomingSignalingMethod) -> P2pConnectionIncomingAction {
    P2pConnectionIncomingAction::Init {
        opts: P2pConnectionIncomingInitOpts {
            peer_id: peer(n),
            signaling,
            offer: offer_from(s, n),
            remote_ip: None,
        },
        rpc_id: None,
    }
}

fn via(relay: u8) -> IncomingSignalingMethod {
    IncomingSignalingMethod::P2p {
        relay_peer_id: peer(relay),
    }
}

/// The channel inits `channels_init` dispatches for peer `n`.
fn channel_inits(s: &State, n: u8) -> Vec<String> {
    let mut dispatcher = redux::Dispatcher::<Action, State>::new();
    s.0.channels_init(&mut dispatcher, peer(n));
    std::iter::from_fn(|| dispatcher.pop())
        .map(|a| format!("{a:?}"))
        .collect()
}

fn opens_signaling(inits: &[String]) -> bool {
    inits
        .iter()
        .any(|a| a.contains("SignalingDiscovery") || a.contains("SignalingExchange"))
}

/// The DiscoveredSend actions the relay's responder dispatches now.
fn introductions(s: &State, t: u64) -> Vec<(PeerId, PeerId)> {
    let mut dispatcher = redux::Dispatcher::<Action, State>::new();
    respond(s, &mut dispatcher, t);
    std::iter::from_fn(|| dispatcher.pop())
        .filter_map(|a| match a {
            Action::P2p(P2pAction::Channels(
                mina_p2p::channels::P2pChannelsAction::SignalingDiscovery(
                    P2pChannelsSignalingDiscoveryAction::DiscoveredSend {
                        peer_id,
                        target_public_key,
                    },
                ),
            )) => Some((peer_id, target_public_key.peer_id())),
            _ => None,
        })
        .collect()
}

/// Runs the responder through a reducer that calls it, on a copy: the
/// discovery request of a peer that is asking.
fn respond(s: &State, dispatcher: &mut redux::Dispatcher<Action, State>, t: u64) {
    let mut copy = State(s.0.clone());
    // DiscoveryRequestReceived needs local Requested; make one asking peer's
    // state so on the copy, then let the reducer run the responder.
    let asking = copy
        .0
        .ready_peers_iter()
        .find(|(_, p)| p.channels.signaling.is_looking_for_peer())
        .map(|(id, _)| *id);
    let Some(asking) = asking else { return };
    if let Some(P2pChannelsSignalingDiscoveryState::Ready { local, .. }) = copy
        .0
        .get_ready_peer_mut(&asking)
        .map(|p| &mut p.channels.signaling.discovery)
    {
        *local = SignalingDiscoveryState::Requested { time: at(0) };
    }
    P2pState::reducer(
        Substate::new(&mut copy, dispatcher),
        ActionMeta::zero_custom(at(t)).with_action(
            P2pChannelsSignalingDiscoveryAction::DiscoveryRequestReceived { peer_id: asking }
                .into(),
        ),
    )
    .unwrap();
}

fn sharing_browser() -> State {
    let mut s = state(&[]);
    s.0.config.initial_peers = vec![web_opts(1, 9360), web_opts(2, 9361)];
    s.0.config.seed_only = true;
    s.0.config.peer_discovery = true;
    s.0.config.enabled_channels = ChannelId::iter_all().collect();
    s.0.config.sharing = P2pSharing::peer();
    s.0.peers
        .insert(peer(1), webrtc_peer(1, Some(TIP), true, false, false));
    s
}

fn sharing_relay() -> State {
    let mut s = state(&[]);
    s.0.config.enabled_channels = ChannelId::iter_all().collect();
    s.0.config.sharing = P2pSharing::relay();
    s
}

/// Adds browser `n` to the relay, from `ip`.
fn join(s: &mut State, n: u8, ip: &str, browser: P2pPeerState) {
    s.0.peers.insert(peer(n), browser);
    s.0.sharing_record_address(peer(n), ip.parse().unwrap());
}

#[test]
fn sharing_off_keeps_the_seed_only_boundary() {
    let mut s = state(&[]);
    s.0.config.initial_peers = vec![web_opts(1, 9360)];
    s.0.config.seed_only = true;
    s.0.config.peer_discovery = false;
    s.0.config.enabled_channels = ChannelId::without_signaling().collect();
    assert_eq!(s.0.config.sharing, P2pSharing::Off);
    // Even with stale signalling state that says the relay is asking.
    s.0.peers
        .insert(peer(1), webrtc_peer(1, Some(TIP), true, true, true));

    assert!(!opens_signaling(&channel_inits(&s, 1)));
    let introduced = relayed(5, 1);
    assert!(!s.0.config.allows_dial(&introduced));
    assert!(!dial(introduced).is_enabled(&s.0, at(1)));
    assert_eq!(
        s.0.incoming_accept(peer(5), &offer_from(&s, 5), &via(1)),
        Err(RejectionReason::PeerCapacityFull)
    );
    assert!(!incoming(&s, 5, via(1)).is_enabled(&s.0, at(1)));
    for action in [
        P2pChannelsSignalingDiscoveryAction::RequestSend { peer_id: peer(1) }.into(),
        P2pChannelsSignalingDiscoveryAction::DiscoveryRequestSend { peer_id: peer(1) }.into(),
        P2pChannelsSignalingExchangeAction::RequestSend { peer_id: peer(1) }.into(),
    ] {
        let action: P2pAction = action;
        assert!(!action.is_enabled(&s.0, at(100)), "{action:?}");
    }
    // The reducer of a relayed incoming connection does nothing either.
    let action = incoming(&s, 5, via(1));
    let mut dispatcher = redux::Dispatcher::<Action, State>::new();
    P2pState::reducer(
        Substate::new(&mut s, &mut dispatcher),
        ActionMeta::zero_custom(at(1)).with_action(action.into()),
    )
    .unwrap();
    assert!(dispatcher.pop().is_none());
    assert!(!s.0.peers.contains_key(&peer(5)));
}

#[test]
fn sharing_browser_uses_signalling_with_its_seeds_only() {
    let mut s = sharing_browser();
    s.0.peers
        .insert(peer(7), webrtc_peer(7, Some(TIP), true, true, true));
    assert!(opens_signaling(&channel_inits(&s, 1)));
    assert!(!opens_signaling(&channel_inits(&s, 7)));
    // It never takes a request to introduce, from a seed or a browser.
    for n in [1, 7] {
        assert!(
            !P2pChannelsSignalingDiscoveryAction::RequestSend { peer_id: peer(n) }
                .is_enabled(&s.0, at(100))
        );
    }
    let target = SecretKey::from_bytes([9; 32]).public_key();
    assert!(!P2pChannelsSignalingDiscoveryAction::DiscoveredSend {
        peer_id: peer(7),
        target_public_key: target,
    }
    .is_enabled(&s.0, at(100)));
    // A browser peer's signalling messages are refused outright.
    assert!(
        !P2pChannelsSignalingExchangeAction::RequestSend { peer_id: peer(7) }
            .is_enabled(&s.0, at(100))
    );
    assert!(!P2pChannelsSignalingExchangeAction::OfferSend {
        peer_id: peer(7),
        offerer_pub_key: SecretKey::from_bytes([8; 32]).public_key(),
        offer: vec![0_u8; 32].into(),
    }
    .is_enabled(&s.0, at(100)));
    // With one seed asking it to introduce someone and another seed taking
    // offers, it introduces no one.
    s.0.peers.remove(&peer(7));
    s.0.peers
        .insert(peer(1), webrtc_peer(1, Some(TIP), true, true, false));
    s.0.peers
        .insert(peer(2), webrtc_peer(2, Some(TIP), true, false, true));
    assert!(introductions(&s, 100).is_empty());
    // The same peers on a relay would be introduced.
    s.0.config.sharing = P2pSharing::Off;
    s.0.config.seed_only = false;
    assert_eq!(introductions(&s, 100), vec![(peer(1), peer(2))]);
}

#[test]
fn sharing_browser_dials_and_accepts_only_seed_introductions() {
    let s = sharing_browser();
    // Introduced by a connected seed: allowed.
    assert!(s.0.config.allows_dial(&relayed(5, 1)));
    assert!(dial(relayed(5, 1)).is_enabled(&s.0, at(1)));
    assert!(incoming(&s, 5, via(1)).is_enabled(&s.0, at(1)));
    // By a seed it isn't connected to: the shape is allowed, the dial isn't.
    assert!(s.0.config.allows_dial(&relayed(5, 2)));
    assert!(!dial(relayed(5, 2)).is_enabled(&s.0, at(1)));
    assert!(!incoming(&s, 5, via(2)).is_enabled(&s.0, at(1)));
    // By a browser, or to a seed through another seed: never.
    let mut s = s;
    s.0.peers
        .insert(peer(7), webrtc_peer(7, Some(TIP), false, false, false));
    assert!(!s.0.config.allows_dial(&relayed(5, 7)));
    assert!(!dial(relayed(5, 7)).is_enabled(&s.0, at(1)));
    assert!(!incoming(&s, 5, via(7)).is_enabled(&s.0, at(1)));
    assert!(!s.0.config.allows_dial(&relayed(2, 1)));
    // Learned addresses stay refused, and so does HTTP signalling.
    assert!(!s.0.config.allows_dial(&web_opts(5, 9360)));
    assert!(!incoming(&s, 5, IncomingSignalingMethod::Http).is_enabled(&s.0, at(1)));
    // A seed's exact address is still dialled.
    assert!(s.0.config.allows_dial(&web_opts(2, 9361)));
}

#[test]
fn sharing_browser_caps_browser_peers_and_bootstraps() {
    let mut s = sharing_browser();
    let room = |s: &State| {
        (
            s.0.sharing_has_room(),
            dial(relayed(20, 1)).is_enabled(&s.0, at(1)),
            incoming(s, 20, via(1)).is_enabled(&s.0, at(1)),
            P2pChannelsSignalingExchangeAction::RequestSend { peer_id: peer(1) }
                .is_enabled(&s.0, at(1)),
        )
    };
    assert_eq!(room(&s), (true, true, true, true));
    assert!(s.0.wants_introduction());

    // Two synced browsers: no more asking, still room for introduced ones.
    for n in [10, 11] {
        s.0.peers
            .insert(peer(n), webrtc_peer(n, Some(TIP - 2), false, false, false));
    }
    assert_eq!(s.0.sharing_browser_peers(), (2, 0));
    assert!(!s.0.wants_introduction());
    let mut asked = s.0.clone();
    if let Some(P2pChannelsSignalingDiscoveryState::Ready { remote, .. }) = asked
        .get_ready_peer_mut(&peer(1))
        .map(|p| &mut p.channels.signaling.discovery)
    {
        *remote = SignalingDiscoveryState::Requested { time: at(0) };
    }
    assert!(
        !P2pChannelsSignalingDiscoveryAction::DiscoveryRequestSend { peer_id: peer(1) }
            .is_enabled(&asked, at(1))
    );
    assert_eq!(room(&s), (true, true, true, true));

    // Four: full.
    for n in [12, 13] {
        s.0.peers
            .insert(peer(n), webrtc_peer(n, Some(TIP), false, false, false));
    }
    assert_eq!(s.0.sharing_browser_peers(), (4, 0));
    assert_eq!(room(&s), (false, false, false, false));

    // Two that are connecting or still syncing: full, though only three.
    s.0.peers.remove(&peer(12));
    s.0.peers.remove(&peer(13));
    s.0.peers.remove(&peer(11));
    s.0.peers.insert(peer(12), connecting_browser(12));
    s.0.peers.insert(
        peer(13),
        webrtc_peer(13, Some(545_434), false, false, false),
    );
    assert_eq!(s.0.sharing_browser_peers(), (3, 2));
    assert_eq!(room(&s), (false, false, false, false));

    // The connecting one gives up: room again.
    s.0.peers.remove(&peer(12));
    assert_eq!(s.0.sharing_browser_peers(), (2, 1));
    assert_eq!(room(&s), (true, true, true, true));
}

#[test]
fn sharing_browser_offers_itself_again_after_answering() {
    let mut s = sharing_browser();
    if let Some(P2pChannelsSignalingExchangeState::Ready { local, .. }) =
        s.0.get_ready_peer_mut(&peer(1))
            .map(|p| &mut p.channels.signaling.exchange)
    {
        *local = SignalingExchangeState::Answered { time: at(0) };
    }
    let rearm = |actions: &[P2pAction]| {
        actions
            .iter()
            .any(|a| format!("{a:?}").contains("SignalingExchange(RequestSend"))
    };
    let actions = tick(&mut s, 10);
    assert!(rearm(&actions));
    assert!(actions
        .iter()
        .filter(|a| format!("{a:?}").contains("SignalingExchange(RequestSend"))
        .all(|a| EnablingCondition::is_enabled(a, &s.0, at(10))));
    // Not when full, and never to a browser peer.
    for n in [10, 11, 12, 13] {
        s.0.peers
            .insert(peer(n), webrtc_peer(n, Some(TIP), true, false, false));
    }
    let actions = tick(&mut s, 20);
    assert!(actions
        .iter()
        .filter(|a| format!("{a:?}").contains("SignalingExchange(RequestSend"))
        .all(|a| !EnablingCondition::is_enabled(a, &s.0, at(20))));
    s.0.config.sharing = P2pSharing::Off;
    assert!(!rearm(&tick(&mut s, 30)));
}

#[test]
fn sharing_relay_introduces_only_peers_that_opened_the_channels() {
    let mut s = sharing_relay();
    // A synced page that didn't opt in: its channels were never opened.
    join(
        &mut s,
        3,
        "198.51.100.3",
        webrtc_peer(3, Some(TIP), false, false, false),
    );
    // A newcomer asking.
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(545_434), true, true, false),
    );
    assert!(introductions(&s, 100).is_empty());
    // A synced page that opened them and takes offers.
    join(
        &mut s,
        5,
        "198.51.100.5",
        webrtc_peer(5, Some(TIP - 1), true, false, true),
    );
    assert_eq!(introductions(&s, 100), vec![(peer(4), peer(5))]);
    // libp2p peers never get the channels.
    s.0.peers.insert(
        peer(6),
        P2pPeerState {
            is_libp2p: true,
            dial_opts: Some(opts(6)),
            status: ready_since(0),
            identify: None,
        },
    );
    assert!(!opens_signaling(&channel_inits(&s, 6)));
    assert!(opens_signaling(&channel_inits(&s, 3)));
    // The relay asks no one for introductions and takes no offers.
    for action in [
        P2pChannelsSignalingDiscoveryAction::DiscoveryRequestSend { peer_id: peer(5) }.into(),
        P2pChannelsSignalingExchangeAction::RequestSend { peer_id: peer(5) }.into(),
    ] {
        let action: P2pAction = action;
        assert!(!action.is_enabled(&s.0, at(100)), "{action:?}");
    }
}

#[test]
fn sharing_relay_pairs_newcomers_with_synced_pages_only() {
    let mut s = sharing_relay();
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(545_434), true, true, false),
    );
    join(
        &mut s,
        5,
        "198.51.100.5",
        webrtc_peer(5, Some(545_434), true, false, true),
    );
    join(
        &mut s,
        6,
        "198.51.100.6",
        webrtc_peer(6, None, true, false, true),
    );
    assert!(introductions(&s, 100).is_empty());
    // A synced requester may meet a newcomer: it serves it.
    join(
        &mut s,
        7,
        "198.51.100.7",
        webrtc_peer(7, Some(TIP), true, true, false),
    );
    let pairs = introductions(&s, 100);
    assert!(pairs.iter().all(|(r, _)| *r == peer(7)), "{pairs:?}");
    assert_eq!(pairs.len(), 1);
}

#[test]
fn sharing_relay_spreads_newcomers_over_synced_pages() {
    let mut s = sharing_relay();
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(545_434), true, true, false),
    );
    join(
        &mut s,
        5,
        "198.51.100.5",
        webrtc_peer(5, Some(TIP), true, false, true),
    );
    join(
        &mut s,
        6,
        "198.51.100.6",
        webrtc_peer(6, Some(TIP), true, false, true),
    );
    join(
        &mut s,
        30,
        "198.51.100.30",
        webrtc_peer(30, Some(TIP), true, false, false),
    );
    join(
        &mut s,
        31,
        "198.51.100.31",
        webrtc_peer(31, Some(TIP), true, false, false),
    );
    s.0.sharing_record_introduction(peer(30), peer(5), at(10));
    s.0.sharing_record_introduction(peer(31), peer(5), at(20));
    s.0.sharing_record_introduction(peer(31), peer(6), at(30));
    for t in [100, 101, 102, 103] {
        assert_eq!(introductions(&s, t), vec![(peer(4), peer(6))]);
    }
}

#[test]
fn sharing_relay_limits_each_peer_address_and_the_total() {
    let limits = P2pSharingRelayLimits::default();
    assert_eq!(
        (
            limits.window.as_secs(),
            limits.per_peer,
            limits.per_address,
            limits.total
        ),
        (3600, 6, 12, 120)
    );
    // Per peer: a requester introduced six times waits, then a target.
    let mut s = sharing_relay();
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(TIP), true, true, false),
    );
    join(
        &mut s,
        5,
        "198.51.100.5",
        webrtc_peer(5, Some(TIP), true, false, true),
    );
    for n in 40..46 {
        join(
            &mut s,
            n,
            &format!("203.0.113.{n}"),
            webrtc_peer(n, Some(TIP), true, false, false),
        );
        s.0.sharing_record_introduction(peer(4), peer(n), at(u64::from(n)));
    }
    assert!(introductions(&s, 100).is_empty());
    assert_eq!(
        s.0.sharing_requester_check(&peer(4), at(100)),
        Err(mina_p2p::sharing::P2pIntroductionRefusal::Peer)
    );
    // The window passes.
    assert_eq!(introductions(&s, 3700), vec![(peer(4), peer(5))]);

    // Per address: one /64 with many keys.
    let mut s = sharing_relay();
    join(
        &mut s,
        5,
        "198.51.100.5",
        webrtc_peer(5, Some(TIP), true, false, true),
    );
    for n in 50..62 {
        join(
            &mut s,
            n,
            &format!("2001:db8:1:2::{n}"),
            webrtc_peer(n, Some(TIP), true, false, false),
        );
    }
    for n in 62..74 {
        join(
            &mut s,
            n,
            &format!("203.0.113.{n}"),
            webrtc_peer(n, Some(TIP), true, false, false),
        );
    }
    for n in 50..62 {
        s.0.sharing_record_introduction(peer(n), peer(n + 12), at(u64::from(n)));
    }
    join(
        &mut s,
        80,
        "2001:db8:1:2:ffff::1",
        webrtc_peer(80, Some(TIP), true, true, false),
    );
    assert!(introductions(&s, 100).is_empty());
    assert_eq!(
        s.0.sharing_requester_check(&peer(80), at(100)),
        Err(mina_p2p::sharing::P2pIntroductionRefusal::Address)
    );
    // Another /64 is another address.
    join(
        &mut s,
        81,
        "2001:db8:1:3::1",
        webrtc_peer(81, Some(TIP), true, true, false),
    );
    assert!(s.0.sharing_requester_check(&peer(81), at(100)).is_ok());

    // Total.
    let mut s = sharing_relay();
    s.0.config.sharing = P2pSharing::Relay(P2pSharingRelayLimits {
        total: 3,
        ..Default::default()
    });
    for n in 90..96 {
        join(
            &mut s,
            n,
            &format!("203.0.113.{n}"),
            webrtc_peer(n, Some(TIP), true, false, false),
        );
    }
    for n in [90, 92, 94] {
        s.0.sharing_record_introduction(peer(n), peer(n + 1), at(u64::from(n)));
    }
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(TIP), true, true, false),
    );
    join(
        &mut s,
        5,
        "198.51.100.5",
        webrtc_peer(5, Some(TIP), true, false, true),
    );
    assert!(introductions(&s, 100).is_empty());
    assert_eq!(introductions(&s, 3700), vec![(peer(4), peer(5))]);
}

#[test]
fn sharing_relay_never_repeats_a_pair_and_needs_an_address() {
    let mut s = sharing_relay();
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(TIP), true, true, false),
    );
    join(
        &mut s,
        5,
        "198.51.100.5",
        webrtc_peer(5, Some(TIP), true, false, true),
    );
    s.0.sharing_record_introduction(peer(5), peer(4), at(10));
    assert!(introductions(&s, 100).is_empty());
    assert_eq!(introductions(&s, 3700), vec![(peer(4), peer(5))]);
    // A peer that didn't come through HTTP signalling has no address.
    let mut s = sharing_relay();
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(TIP), true, true, false),
    );
    s.0.peers
        .insert(peer(5), webrtc_peer(5, Some(TIP), true, false, true));
    assert!(introductions(&s, 100).is_empty());
}

#[test]
fn sharing_relay_records_introductions_refusals_and_addresses() {
    let mut s = sharing_relay();
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(TIP), true, true, false),
    );
    join(
        &mut s,
        5,
        "::ffff:198.51.100.5",
        webrtc_peer(5, Some(TIP), true, false, true),
    );
    assert_eq!(
        s.0.sharing_relay.addresses[&peer(5)],
        mina_p2p::sharing::P2pAddressGroup::V4([198, 51, 100, 5])
    );
    if let Some(P2pChannelsSignalingDiscoveryState::Ready { local, .. }) =
        s.0.get_ready_peer_mut(&peer(4))
            .map(|p| &mut p.channels.signaling.discovery)
    {
        *local = SignalingDiscoveryState::Requested { time: at(0) };
    }
    let actions = apply_traced(
        &mut s,
        P2pChannelsSignalingDiscoveryAction::DiscoveryRequestReceived { peer_id: peer(4) },
        100,
    );
    assert!(has(&actions, "DiscoveredSend"), "{actions:?}");
    let send = actions
        .into_iter()
        .find(|a| format!("{a:?}").contains("DiscoveredSend"))
        .unwrap();
    assert!(EnablingCondition::is_enabled(&send, &s.0, at(100)));
    apply(&mut s, send.clone(), 100);
    assert_eq!(s.0.sharing_relay.introduced_total, 1);
    assert_eq!(
        s.0.sharing_relay.recent.back(),
        Some(&P2pIntroduction {
            time: at(100),
            requester: peer(4),
            target: peer(5),
            requester_address: mina_p2p::sharing::P2pAddressGroup::V4([198, 51, 100, 4]),
            target_address: mina_p2p::sharing::P2pAddressGroup::V4([198, 51, 100, 5]),
        })
    );
    // A second send is refused.
    assert!(!EnablingCondition::is_enabled(&send, &s.0, at(101)));

    // A request at a limit is counted and waits.
    let mut s = sharing_relay();
    s.0.config.sharing = P2pSharing::Relay(P2pSharingRelayLimits {
        total: 0,
        ..Default::default()
    });
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(TIP), true, false, false),
    );
    if let Some(P2pChannelsSignalingDiscoveryState::Ready { local, .. }) =
        s.0.get_ready_peer_mut(&peer(4))
            .map(|p| &mut p.channels.signaling.discovery)
    {
        *local = SignalingDiscoveryState::Requested { time: at(0) };
    }
    apply(
        &mut s,
        P2pChannelsSignalingDiscoveryAction::DiscoveryRequestReceived { peer_id: peer(4) },
        100,
    );
    assert_eq!(s.0.sharing_relay.refused_total, 1);

    // The incoming connection's address is kept by a sharing relay only.
    for (sharing, kept) in [(P2pSharing::relay(), true), (P2pSharing::Off, false)] {
        let mut s = sharing_relay();
        s.0.config.sharing = sharing;
        let P2pConnectionIncomingAction::Init { mut opts, rpc_id } =
            incoming(&s, 9, IncomingSignalingMethod::Http)
        else {
            unreachable!()
        };
        opts.remote_ip = "203.0.113.9".parse().ok();
        apply(
            &mut s,
            P2pConnectionIncomingAction::Init { opts, rpc_id },
            1,
        );
        assert_eq!(s.0.sharing_relay.addresses.contains_key(&peer(9)), kept);
    }
}

// The security review of PR 261 (R1 to R3), and its non-blocking points.

/// Peer `n` repeats our own tip, holding no ledger, with its streaming RPC
/// ready.
fn echoing_browser(s: &mut State, n: u8) {
    use mina_p2p::channels::streaming_rpc::{
        P2pChannelsStreamingRpcState, P2pStreamingRpcLocalState, P2pStreamingRpcRemoteState,
    };
    let replay =
        s.0.get_ready_peer(&peer(1))
            .unwrap()
            .channels
            .best_tip
            .last_sent()
            .unwrap()
            .clone();
    s.0.peers
        .insert(peer(n), webrtc_peer(n, None, false, false, false));
    apply(
        s,
        P2pPeerAction::BestTipUpdate {
            peer_id: peer(n),
            best_tip: replay,
        },
        100,
    );
    s.0.get_ready_peer_mut(&peer(n))
        .unwrap()
        .channels
        .streaming_rpc = P2pChannelsStreamingRpcState::Ready {
        time: at(100),
        local: P2pStreamingRpcLocalState::WaitingForRequest { time: at(100) },
        remote: P2pStreamingRpcRemoteState::WaitingForRequest { time: at(100) },
        remote_last_responded: at(0),
    };
}

fn staged_ledger_request(n: u8) -> P2pChannelsStreamingRpcAction {
    use mina_p2p::channels::streaming_rpc::P2pStreamingRpcRequest;
    P2pChannelsStreamingRpcAction::RequestReceived {
        peer_id: peer(n),
        id: 0,
        request: Box::new(P2pStreamingRpcRequest::StagedLedgerParts(
            block_at(TIP).hash().clone(),
        )),
    }
}

/// Applies `action` and, as the store would, the enabled actions it
/// dispatches, depth first. Returns them all.
fn apply_through(s: &mut State, action: impl Into<P2pAction>, t: u64) -> Vec<P2pAction> {
    let mut done = Vec::new();
    let mut todo = vec![action.into()];
    while let Some(action) = todo.pop() {
        if !EnablingCondition::is_enabled(&action, &s.0, at(t)) || done.len() > 64 {
            continue;
        }
        let next = apply_traced(s, action.clone(), t);
        done.push(action);
        todo.extend(next.into_iter().rev());
    }
    done
}

/// Whether the reducer answered the request at once with nothing.
fn answered_empty(actions: &[P2pAction]) -> bool {
    actions.iter().any(|a| {
        let a = format!("{a:?}");
        (a.contains("ResponseSendInit") || a.contains("ResponseSend {"))
            && a.contains("response: None")
    })
}

#[test]
fn echoed_tips_do_not_buy_more_than_two_bootstraps() {
    let mut s = sharing_browser();
    for n in 10..14 {
        assert!(incoming(&s, n, via(1)).is_enabled(&s.0, at(100)));
        echoing_browser(&mut s, n);
    }
    // Admission believes them, as before: four browsers, none syncing.
    assert_eq!(s.0.sharing_browser_peers(), (4, 0));
    // Serving doesn't: two staged-ledger streams, then empty answers.
    for (n, served) in [(10, true), (11, true), (12, false), (13, false)] {
        let request = staged_ledger_request(n);
        assert!(request.is_enabled(&s.0, at(101)));
        let actions = apply_through(&mut s, request, 101);
        assert_eq!(answered_empty(&actions), !served, "peer {n}: {actions:?}");
        // A served one waits for the ledger; a refused one is answered.
        let todo =
            s.0.get_ready_peer(&peer(n))
                .unwrap()
                .channels
                .streaming_rpc
                .remote_todo_request();
        assert_eq!(todo.is_some(), served, "peer {n}");
    }
    // Admission now counts the two being served.
    assert_eq!(s.0.sharing_browser_peers(), (4, 2));
    // A ledger query or a block from a third browser: empty too.
    let query = P2pChannelsRpcAction::RequestReceived {
        peer_id: peer(12),
        id: 1,
        request: Box::new(mina_p2p::channels::rpc::P2pRpcRequest::Block(
            block_at(TIP).hash().clone(),
        )),
    };
    s.0.get_ready_peer_mut(&peer(12)).unwrap().channels.rpc =
        mina_p2p::channels::rpc::P2pChannelsRpcState::Ready {
            time: at(0),
            local: mina_p2p::channels::rpc::P2pRpcLocalState::WaitingForRequest { time: at(0) },
            remote: mina_p2p::channels::rpc::P2pRpcRemoteState {
                pending_requests: Default::default(),
                last_responded: at(0),
            },
        };
    let actions = apply_traced(&mut s, query.clone(), 102);
    assert!(answered_empty(&actions), "{actions:?}");
    // Once the two served streams end and SERVING_WINDOW passes, a slot frees.
    for n in [10, 11] {
        s.0.get_ready_peer_mut(&peer(n)).unwrap().channels.streaming_rpc =
            mina_p2p::channels::streaming_rpc::P2pChannelsStreamingRpcState::Ready {
                time: at(100),
                local: mina_p2p::channels::streaming_rpc::P2pStreamingRpcLocalState::WaitingForRequest { time: at(100) },
                remote: mina_p2p::channels::streaming_rpc::P2pStreamingRpcRemoteState::WaitingForRequest { time: at(100) },
                remote_last_responded: at(110),
            };
    }
    assert!(!s.0.sharing_serves(&peer(13), at(120)));
    assert!(s.0.sharing_serves(&peer(13), at(140)));
}

#[test]
fn a_seed_and_sharing_off_are_always_served() {
    let mut s = sharing_browser();
    for n in 10..12 {
        echoing_browser(&mut s, n);
        apply(&mut s, staged_ledger_request(n), 101);
    }
    assert!(!s.0.sharing_serves(&peer(12), at(102)));
    assert!(s.0.sharing_serves(&peer(1), at(102)));
    // Sharing off: no cap, nothing recorded.
    let mut s = sharing_browser();
    s.0.config.sharing = P2pSharing::Off;
    for n in 10..14 {
        echoing_browser(&mut s, n);
        let actions = apply_traced(&mut s, staged_ledger_request(n), 101);
        assert!(!answered_empty(&actions), "{actions:?}");
    }
    assert!(s.0.sharing_served.is_empty());
}

#[test]
fn the_relay_sees_a_page_fetching_bootstrap_data_as_syncing_whatever_its_tip() {
    let mut s = sharing_relay();
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(545_434), true, true, false),
    );
    join(
        &mut s,
        5,
        "198.51.100.5",
        webrtc_peer(5, Some(545_434), true, false, true),
    );
    assert!(introductions(&s, 100).is_empty());
    // Page 4 repeats the relay's tip, but it asks the relay for the staged
    // ledger: still syncing, so not paired with page 5.
    let replay =
        s.0.get_ready_peer(&peer(4))
            .unwrap()
            .channels
            .best_tip
            .last_sent()
            .unwrap()
            .clone();
    apply(
        &mut s,
        P2pPeerAction::BestTipUpdate {
            peer_id: peer(4),
            best_tip: replay,
        },
        100,
    );
    s.0.get_ready_peer_mut(&peer(4))
        .unwrap()
        .channels
        .streaming_rpc = mina_p2p::channels::streaming_rpc::P2pChannelsStreamingRpcState::Ready {
        time: at(100),
        local: mina_p2p::channels::streaming_rpc::P2pStreamingRpcLocalState::WaitingForRequest {
            time: at(100),
        },
        remote: mina_p2p::channels::streaming_rpc::P2pStreamingRpcRemoteState::WaitingForRequest {
            time: at(100),
        },
        remote_last_responded: at(0),
    };
    let actions = apply_traced(&mut s, staged_ledger_request(4), 100);
    // The relay serves every page, sharing or not.
    assert!(!answered_empty(&actions), "{actions:?}");
    assert!(introductions(&s, 101).is_empty());
}

#[test]
fn incoming_logs_carry_neither_the_source_address_nor_the_sdp() {
    use mina_core::log::{ActionEvent, EventContext};
    use std::{
        io,
        sync::{Arc, Mutex},
    };
    #[derive(Clone)]
    struct Writer(Arc<Mutex<Vec<u8>>>);
    impl io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    struct Context;
    impl EventContext for Context {
        fn timestamp(&self) -> redux::Timestamp {
            at(0)
        }
        fn time(&self) -> &dyn tracing::Value {
            &0u64
        }
        fn node_id(&self) -> &dyn tracing::Value {
            &0u64
        }
        fn log_node_id(&self) -> bool {
            false
        }
    }
    for sharing in [P2pSharing::Off, P2pSharing::relay()] {
        let mut s = state(&[]);
        s.0.config.sharing = sharing;
        let P2pConnectionIncomingAction::Init { mut opts, rpc_id } =
            incoming(&s, 4, IncomingSignalingMethod::Http)
        else {
            unreachable!()
        };
        opts.remote_ip = "198.51.100.77".parse().ok();
        opts.offer.sdp = "v=0 SDP-SENTINEL 192.0.2.66".into();
        let effectful =
            mina_p2p::connection::incoming_effectful::P2pConnectionIncomingEffectfulAction::Init {
                opts: opts.clone(),
            };
        let action = P2pConnectionIncomingAction::Init { opts, rpc_id };
        let output = Arc::new(Mutex::new(Vec::new()));
        let writer = Writer(output.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            action.action_event(&Context);
            effectful.action_event(&Context);
        });
        let mut text = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        text.push_str(&format!("{action:?} {effectful:?}"));
        assert!(text.contains("redacted"), "{text}");
        for secret in ["198.51.100.77", "SDP-SENTINEL", "192.0.2.66"] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    }
}

/// A description with these candidates only.
fn sdp_with(candidates: &[&str]) -> String {
    let mut sdp = HONEST_SDP[..HONEST_SDP.find("a=candidate").unwrap()].to_owned();
    for c in candidates {
        sdp.push_str(&format!("a=candidate:{c}\r\n"));
    }
    sdp
}

const LAN: &str = "1 1 udp 2122260223 192.168.1.20 4444 typ host";
const LOOPBACK: &str = "2 1 udp 2122260223 127.0.0.1 4444 typ host";
const TAILNET: &str = "3 1 udp 1686052607 100.101.102.103 41641 typ srflx raddr 0.0.0.0 rport 0";
const MDNS_HOST: &str =
    "4 1 udp 2122260223 2e5a5b7c-1c2f-4d7e-9f4b-123456789abc.local 54321 typ host";

#[test]
fn an_introduced_offer_with_no_public_candidate_is_refused() {
    let mut s = sharing_browser();
    let mut offer = *offer_from(&s, 10);
    offer.sdp = sdp_with(&[LAN, LOOPBACK, TAILNET]);
    // Through the relay's exchange channel, decrypted.
    if let Some(
        mina_p2p::channels::signaling::exchange::P2pChannelsSignalingExchangeState::Ready {
            local,
            ..
        },
    ) =
        s.0.get_ready_peer_mut(&peer(1))
            .map(|p| &mut p.channels.signaling.exchange)
    {
        *local = SignalingExchangeState::Offered {
            time: at(0),
            offerer_pub_key: SecretKey::from_bytes([10; 32]).public_key(),
        };
    }
    let actions = apply_traced(
        &mut s,
        P2pChannelsSignalingExchangeAction::OfferDecryptSuccess {
            peer_id: peer(1),
            offer: offer.clone(),
        },
        100,
    );
    assert!(has(&actions, "UnacceptableSdp"), "{actions:?}");
    assert!(!has(&actions, "Incoming(Init"), "{actions:?}");
    // Directly, too: admission refuses it.
    let P2pConnectionIncomingAction::Init { mut opts, rpc_id } = incoming(&s, 10, via(1)) else {
        unreachable!()
    };
    opts.offer.sdp = offer.sdp;
    assert!(!P2pConnectionIncomingAction::Init { opts, rpc_id }.is_enabled(&s.0, at(100)));
}

#[test]
fn the_webrtc_stack_gets_only_allowed_candidates_and_auth_keeps_the_original() {
    let mut s = sharing_browser();
    let P2pConnectionIncomingAction::Init { mut opts, rpc_id } = incoming(&s, 10, via(1)) else {
        unreachable!()
    };
    let original = sdp_with(&[LAN, MDNS_HOST, LOOPBACK, TAILNET]);
    opts.offer.sdp = original.clone();
    let action = P2pConnectionIncomingAction::Init { opts, rpc_id };
    assert!(action.is_enabled(&s.0, at(100)));
    let mut dispatcher = redux::Dispatcher::<Action, State>::new();
    P2pState::reducer(
        Substate::new(&mut s, &mut dispatcher),
        ActionMeta::zero_custom(at(100)).with_action(action.into()),
    )
    .unwrap();
    let effect = std::iter::from_fn(|| dispatcher.pop())
        .find_map(|a| match a {
            Action::P2pEffectful(P2pEffectfulAction::Connection(
                mina_p2p::connection::P2pConnectionEffectfulAction::Incoming(
                    P2pConnectionIncomingEffectfulAction::Init { opts },
                ),
            )) => Some(opts),
            _ => None,
        })
        .expect("the WebRTC stack is asked to answer");
    assert_eq!(effect.offer.sdp, sdp_with(&[MDNS_HOST]));
    // The state keeps the offer as sent: the authentication hashes it.
    let stored = match &s.0.peers[&peer(10)].status {
        P2pPeerStatus::Connecting(P2pConnectionState::Incoming(
            mina_p2p::connection::incoming::P2pConnectionIncomingState::Init { offer, .. },
        )) => offer.sdp.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(stored, original);

    // Our own dial: the answer.
    let mut s = sharing_browser();
    let opts = relayed(11, 1);
    let me = s.0.my_id();
    let answer_of = |sdp: String| {
        Box::new(mina_p2p::webrtc::Answer {
            sdp,
            identity_pub_key: SecretKey::from_bytes([11; 32]).public_key(),
            target_peer_id: me,
            max_rpc_requests: None,
        })
    };
    let private_only = answer_of(sdp_with(&[LAN, LOOPBACK]));
    let mixed = answer_of(sdp_with(&[LAN, MDNS_HOST]));
    s.0.peers.insert(
        peer(11),
        P2pPeerState {
            is_libp2p: false,
            dial_opts: None,
            status: P2pPeerStatus::Connecting(P2pConnectionState::Outgoing(
                mina_p2p::connection::outgoing::P2pConnectionOutgoingState::AnswerRecvPending {
                    time: at(100),
                    opts,
                    offer: offer_from(&s, 11),
                    rpc_id: None,
                    on_success: None,
                },
            )),
            identify: None,
        },
    );
    assert!(!P2pConnectionOutgoingAction::AnswerRecvSuccess {
        peer_id: peer(11),
        answer: private_only,
    }
    .is_enabled(&s.0, at(101)));
    let success = P2pConnectionOutgoingAction::AnswerRecvSuccess {
        peer_id: peer(11),
        answer: mixed.clone(),
    };
    assert!(success.is_enabled(&s.0, at(101)));
    let mut dispatcher = redux::Dispatcher::<Action, State>::new();
    P2pState::reducer(
        Substate::new(&mut s, &mut dispatcher),
        ActionMeta::zero_custom(at(101)).with_action(success.into()),
    )
    .unwrap();
    let set = std::iter::from_fn(|| dispatcher.pop())
        .find_map(|a| match a {
            Action::P2pEffectful(P2pEffectfulAction::Connection(
                mina_p2p::connection::P2pConnectionEffectfulAction::Outgoing(
                    P2pConnectionOutgoingEffectfulAction::AnswerSet { answer, .. },
                ),
            )) => Some(answer),
            _ => None,
        })
        .expect("the answer is set");
    assert_eq!(set.sdp, sdp_with(&[MDNS_HOST]));
    match &s.0.peers[&peer(11)].status {
        P2pPeerStatus::Connecting(P2pConnectionState::Outgoing(
            mina_p2p::connection::outgoing::P2pConnectionOutgoingState::AnswerRecvSuccess {
                answer,
                ..
            },
        )) => assert_eq!(answer.sdp, mixed.sdp),
        other => panic!("{other:?}"),
    }
}

#[test]
fn seeds_and_sharing_off_hand_descriptions_over_unchanged() {
    let s = sharing_browser();
    let lan = sdp_with(&[LAN]);
    assert_eq!(s.0.sharing_stack_sdp(false, &lan), Ok(None));
    let mut off = sharing_browser();
    off.0.config.sharing = P2pSharing::Off;
    assert_eq!(off.0.sharing_stack_sdp(true, &lan), Ok(None));
    assert!(s.0.sharing_stack_sdp(true, &lan).is_err());
}

#[test]
fn a_relay_forgets_departed_addresses_and_bounds_the_rest() {
    let mut s = sharing_relay();
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(TIP), true, true, false),
    );
    s.0.peers.get_mut(&peer(4)).unwrap().status = P2pPeerStatus::Disconnected { time: at(100) };
    tick(&mut s, 101);
    assert!(!s.0.sharing_relay.addresses.contains_key(&peer(4)));
    // Pending offers can't grow the map past its bound.
    let mut s = sharing_relay();
    for i in 0..(mina_p2p::sharing::MAX_ADDRESSES + 50) {
        let id = SecretKey::from_bytes({
            let mut b = [7u8; 32];
            b[..4].copy_from_slice(&(i as u32).to_be_bytes());
            b
        })
        .public_key()
        .peer_id();
        s.0.peers.insert(
            id,
            P2pPeerState {
                is_libp2p: false,
                dial_opts: None,
                status: P2pPeerStatus::Connecting(P2pConnectionState::outgoing_init(&relayed(
                    9, 1,
                ))),
                identify: None,
            },
        );
        s.0.sharing_record_address(id, "198.51.100.7".parse().unwrap());
    }
    assert_eq!(
        s.0.sharing_relay.addresses.len(),
        mina_p2p::sharing::MAX_ADDRESSES
    );
}

#[test]
fn a_request_waiting_at_a_limit_is_retried_by_the_timer() {
    let mut s = sharing_relay();
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(TIP), true, true, false),
    );
    join(
        &mut s,
        5,
        "198.51.100.5",
        webrtc_peer(5, Some(TIP), true, false, true),
    );
    for n in 40..46 {
        join(
            &mut s,
            n,
            &format!("203.0.113.{n}"),
            webrtc_peer(n, Some(TIP), true, false, false),
        );
        s.0.sharing_record_introduction(peer(4), peer(n), at(u64::from(n)));
    }
    let sends = |actions: &[P2pAction]| has(actions, "DiscoveredSend");
    assert!(!sends(&tick(&mut s, 100)));
    // No signalling happens; the hour passes.
    assert!(sends(&tick(&mut s, 3700)));
}

/// A requester can't make a sharing relay walk its peers at will: the
/// relay answers one Discover per ask of its own (GetNext); a Discover while
/// one is open is unexpected and disconnects the peer; and the relay asks
/// again only 60 s after an ask ends.
#[test]
fn a_requester_gets_one_answer_per_ask_of_the_relay() {
    use mina_p2p::channels::{
        signaling::discovery::SignalingDiscoveryChannelMsg, ChannelMsg,
        P2pChannelsMessageReceivedAction,
    };
    let discover = || P2pChannelsMessageReceivedAction {
        peer_id: peer(4),
        message: Box::new(ChannelMsg::SignalingDiscovery(
            SignalingDiscoveryChannelMsg::Discover,
        )),
    };
    let ask = P2pChannelsSignalingDiscoveryAction::RequestSend { peer_id: peer(4) };
    let answer = P2pChannelsSignalingDiscoveryAction::DiscoveryRequestReceived { peer_id: peer(4) };

    let mut s = sharing_relay();
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(TIP), true, false, false),
    );
    // Before the relay asks, a Discover is unexpected.
    let actions = apply_traced(&mut s, discover(), 1);
    assert!(has(&actions, "P2pChannelMsgUnexpected"), "{actions:?}");

    let mut s = sharing_relay();
    join(
        &mut s,
        4,
        "198.51.100.4",
        webrtc_peer(4, Some(TIP), true, false, false),
    );
    assert!(ask.is_enabled(&s.0, at(1)));
    apply(&mut s, ask.clone(), 1);
    let actions = apply_traced(&mut s, discover(), 2);
    assert!(has(&actions, "DiscoveryRequestReceived"), "{actions:?}");
    assert!(!has(&actions, "P2pChannelMsgUnexpected"), "{actions:?}");
    apply(&mut s, answer.clone(), 2);
    // A second Discover while the first is open: unexpected, and the relay
    // doesn't ask again meanwhile.
    assert!(!answer.is_enabled(&s.0, at(3)));
    assert!(!ask.is_enabled(&s.0, at(3)));
    let actions = apply_traced(&mut s, discover(), 3);
    assert!(has(&actions, "P2pChannelMsgUnexpected"), "{actions:?}");

    // The ask ends (the requester turns down the target): the next comes
    // 60 s later, not before.
    if let Some(P2pChannelsSignalingDiscoveryState::Ready { local, .. }) =
        s.0.get_ready_peer_mut(&peer(4))
            .map(|p| &mut p.channels.signaling.discovery)
    {
        *local = SignalingDiscoveryState::DiscoveredRejected {
            time: at(10),
            target_public_key: SecretKey::from_bytes([5; 32]).public_key(),
        };
    }
    assert!(!answer.is_enabled(&s.0, at(11)));
    assert!(!ask.is_enabled(&s.0, at(69)));
    assert!(ask.is_enabled(&s.0, at(70)));
}
