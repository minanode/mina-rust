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
