//! Privacy policy at the node callback and RPC boundaries.
use crate::{
    p2p::{
        callbacks::P2pCallbacksAction,
        channels::{rpc::P2pRpcResponse, ChannelId},
        connection::outgoing::{P2pConnectionOutgoingAction, P2pConnectionOutgoingInitOpts},
        identity::SecretKey,
    },
    transaction_pool::submission_tests::{fresh_state, run},
};

#[test]
fn browser_ignores_initial_peers_rpc_and_refuses_operator_dial_outside_seeds() {
    let mut state = fresh_state();
    state.p2p.initialize(&mina_core::MAINNET_CHAIN_ID).unwrap();
    let seed = P2pConnectionOutgoingInitOpts::WebRTC {
        peer_id: SecretKey::from_bytes([1; 32]).public_key().peer_id(),
        signaling: "/http/127.0.0.1/9360".parse().unwrap(),
    };
    let learned = P2pConnectionOutgoingInitOpts::WebRTC {
        peer_id: SecretKey::from_bytes([2; 32]).public_key().peer_id(),
        signaling: "/http/127.0.0.1/9361".parse().unwrap(),
    };
    let p2p = state.p2p.ready_mut().unwrap();
    p2p.config.seed_only = true;
    p2p.config.peer_discovery = false;
    p2p.config.enabled_channels = ChannelId::without_signaling().collect();
    p2p.config.initial_peers = vec![seed.clone()];
    run(
        &mut state,
        P2pCallbacksAction::P2pChannelsRpcResponseReceived {
            peer_id: *seed.peer_id(),
            id: 0,
            response: Some(Box::new(P2pRpcResponse::InitialPeers(
                [learned.clone()].into_iter().collect(),
            ))),
        },
    );
    assert!(!state
        .p2p
        .ready()
        .unwrap()
        .peers
        .contains_key(learned.peer_id()));
    assert_eq!(state.p2p.ready().unwrap().config.initial_peers, vec![seed]);
    let ran = run(
        &mut state,
        P2pConnectionOutgoingAction::Init {
            opts: learned,
            rpc_id: Some(crate::rpc::RpcId::new_unchecked(1, 1)),
            on_success: None,
        },
    );
    assert!(ran.is_empty());
}

/// The HTTP signalling source address reaches the p2p state only on a
/// sharing relay, and no RPC request record keeps it (MINAWEB-OPT-IN-SHARING
/// review, R2).
#[test]
fn signalling_source_address_stays_out_of_requests_unless_sharing_relay() {
    use crate::{
        p2p::{
            connection::incoming::{
                IncomingSignalingMethod, P2pConnectionIncomingAction, P2pConnectionIncomingInitOpts,
            },
            webrtc::Offer,
            P2pSharing,
        },
        rpc::{RpcAction, RpcRequest},
        Action,
    };
    for (sharing, passed) in [(P2pSharing::Off, false), (P2pSharing::relay(), true)] {
        let mut state = fresh_state();
        state.p2p.initialize(&mina_core::MAINNET_CHAIN_ID).unwrap();
        let p2p = state.p2p.ready_mut().unwrap();
        p2p.config.sharing = sharing;
        let key = SecretKey::from_bytes([3; 32]).public_key();
        let opts = P2pConnectionIncomingInitOpts {
            peer_id: key.peer_id(),
            signaling: IncomingSignalingMethod::Http,
            offer: Box::new(Offer {
                sdp: "v=0\r\n".into(),
                chain_id: p2p.chain_id.clone(),
                identity_pub_key: key,
                target_peer_id: p2p.my_id(),
                host: "127.0.0.1".parse().unwrap(),
                listen_port: None,
            }),
            remote_ip: "198.51.100.77".parse().ok(),
        };
        let rpc_id = crate::rpc::RpcId::new_unchecked(1, 1);
        let ran = run(
            &mut state,
            RpcAction::P2pConnectionIncomingInit {
                rpc_id,
                opts: opts.clone(),
            },
        );
        match &state.rpc.requests.get(&rpc_id).unwrap().req {
            RpcRequest::P2pConnectionIncoming(stored) => assert_eq!(stored.remote_ip, None),
            other => panic!("{other:?}"),
        }
        let init = ran
            .iter()
            .find_map(|a| match a {
                Action::P2p(crate::p2p::P2pAction::Connection(
                    crate::p2p::connection::P2pConnectionAction::Incoming(
                        P2pConnectionIncomingAction::Init { opts, .. },
                    ),
                )) => Some(opts),
                _ => None,
            })
            .expect("the connection starts");
        assert_eq!(init.remote_ip.is_some(), passed, "{sharing:?}");
        assert!(!format!("{ran:?}").contains("198.51.100.77"));
    }
}
