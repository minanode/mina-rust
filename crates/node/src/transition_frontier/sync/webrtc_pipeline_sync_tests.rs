//! WEB-FAR-RELAY-LOOP and WEB-RELAY-RTT-THROUGHPUT: ledger queries pipelined
//! to a WebRTC peer, and requests that time out on silence, not slowness.
//! Scheduler tests with a synthetic ledger sync, as in `no_outcome_sync_tests`.
use super::*;
use crate::{
    ledger::LedgerAddress,
    p2p::{
        channels::rpc::{
            P2pChannelsRpcAction, P2pChannelsRpcState, P2pRpcLocalState, P2pRpcRemoteState,
            P2pRpcRequest, MAX_P2P_RPC_REMOTE_CONCURRENT_REQUESTS,
        },
        peer::P2pPeerAction,
        P2pPeerState, P2pPeerStatus, P2pPeerStatusReady, PeerId,
    },
    transition_frontier::sync::ledger::snarked::{
        LedgerAddressQueryPending, PeerRpcState as LedgerRpc,
        TransitionFrontierSyncLedgerSnarkedAction as SnarkedAction,
    },
};
use redux::{ActionMeta, Dispatcher, EnablingCondition};

fn at(secs: u64) -> Timestamp {
    Timestamp::new(secs.saturating_mul(1_000_000_000))
}

/// A node syncing the staking ledger from one peer, with `queued` account
/// subtrees left to ask for.
fn node(is_libp2p: bool, queued: usize) -> (crate::State, PeerId) {
    let root = block(&template_block(), 100, None, 7);
    let mut state = sync_test_node(vec![AppliedBlock {
        block: root.clone(),
        just_emitted_a_proof: false,
    }]);
    let chain_id = serde_json::from_str(
        r#""0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886""#,
    )
    .unwrap();
    state.p2p.initialize(&chain_id).unwrap();
    let peer = PeerId::from_bytes([94; 32]);
    let mut ready = P2pPeerStatusReady::new(false, Timestamp::ZERO, &Default::default());
    ready.best_tip = Some(root.clone());
    ready.channels.rpc = P2pChannelsRpcState::Ready {
        time: Timestamp::ZERO,
        local: P2pRpcLocalState::WaitingForRequest {
            time: Timestamp::ZERO,
        },
        remote: P2pRpcRemoteState {
            pending_requests: Default::default(),
            last_responded: Timestamp::ZERO,
        },
    };
    ready.channels.rpc_pipeline = crate::p2p::channels::rpc::P2pRpcPipeline::for_peer(is_libp2p, 5);
    state.p2p.ready_mut().unwrap().peers.insert(
        peer,
        P2pPeerState {
            is_libp2p,
            dial_opts: None,
            identify: None,
            status: P2pPeerStatus::Ready(Box::new(ready)),
        },
    );
    let mut address = LedgerAddress::first(29);
    let mut queue = std::collections::BTreeMap::new();
    for n in 0..queued {
        queue.insert(
            address.clone(),
            ledger_hash(100u64.saturating_add(n as u64)),
        );
        address = address.next().unwrap();
    }
    let target = SyncLedgerTarget::staking_epoch(&root);
    state.transition_frontier.sync =
        TransitionFrontierSyncState::StakingLedgerPending(TransitionFrontierSyncLedgerPending {
            time: Timestamp::ZERO,
            best_tip: root.clone(),
            root_block: root.clone(),
            blocks_inbetween: vec![],
            root_block_updates: vec![],
            ledger: TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncPending {
                time: Timestamp::ZERO,
                target,
                total_accounts_expected: 64u64.saturating_mul(queued as u64),
                synced_accounts_count: 0,
                synced_hashes_count: 0,
                queue,
                pending_addresses: Default::default(),
                unavailable_peers: Default::default(),
                peer_query_errors: Default::default(),
            }
            .into(),
        });
    (state, peer)
}

/// Runs `action` and what it queues, depth first and checking each action
/// when it runs, as the store does, without effects. Returns what ran.
fn run(state: &mut crate::State, action: impl Into<crate::Action>, now: Timestamp) -> Vec<String> {
    fn go(state: &mut crate::State, action: crate::Action, now: Timestamp, ran: &mut Vec<String>) {
        if !action.is_enabled(state, now) {
            return;
        }
        let mut dispatcher = Dispatcher::new();
        crate::reducer(
            state,
            &ActionMeta::zero_custom(now).with_action(action.clone()),
            &mut dispatcher,
        );
        ran.push(format!("{action:?}"));
        assert!(ran.len() < 10_000);
        while let Some(next) = dispatcher.pop() {
            go(state, next, now, ran);
        }
    }
    let mut ran = vec![];
    let action = action.into();
    assert!(action.is_enabled(state, now), "{action:?}");
    go(state, action, now, &mut ran);
    ran
}

fn pending_rpc_ids(state: &crate::State, peer: &PeerId) -> Vec<u64> {
    let Some(TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncPending {
        pending_addresses,
        ..
    }) = state
        .transition_frontier
        .sync
        .ledger()
        .and_then(|l| l.snarked())
    else {
        panic!("not syncing addresses");
    };
    let mut ids = pending_addresses
        .values()
        .filter_map(
            |p: &LedgerAddressQueryPending| match p.attempts.get(peer)? {
                LedgerRpc::Pending { rpc_id, .. } => Some(*rpc_id),
                _ => None,
            },
        )
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

/// One `PeersQuery` gives a WebRTC peer as many ledger queries as it takes at
/// once, five, and a libp2p peer one, as before. Answers find their address
/// by id, whichever slot the request went in.
#[test]
fn a_webrtc_peer_takes_five_ledger_queries_at_once() {
    let (mut libp2p, peer) = node(true, 8);
    run(&mut libp2p, SnarkedAction::PeersQuery, at(0));
    assert_eq!(pending_rpc_ids(&libp2p, &peer), vec![0]);

    let (mut state, peer) = node(false, 8);
    run(&mut state, SnarkedAction::PeersQuery, at(0));
    assert_eq!(
        pending_rpc_ids(&state, &peer),
        (0..MAX_P2P_RPC_REMOTE_CONCURRENT_REQUESTS as u64).collect::<Vec<_>>()
    );
    let channels = &state.p2p.get_ready_peer(&peer).unwrap().channels;
    assert!(!channels.can_send_sync_request());
    assert_eq!(channels.unanswered_rpc_ids().count(), 5);
    // Nothing more to the peer, of any kind, until it answers.
    assert!(!P2pChannelsRpcAction::RequestSend {
        peer_id: peer,
        id: 5,
        request: Box::new(P2pRpcRequest::BestTipWithProof),
        on_init: None,
    }
    .is_enabled(&state, at(0)));

    // The answer to a pipelined request: data unavailable, so that address's
    // attempt fails and the others stay pending.
    let ran = run(
        &mut state,
        P2pChannelsRpcAction::ResponseReceived {
            peer_id: peer,
            id: 3,
            response: None,
        },
        at(1),
    );
    assert!(ran.iter().any(|a| a.contains("PeerQueryAddressError")
        && a.contains("rpc_id: 3")
        && a.contains("DataUnavailable")));
    assert_eq!(pending_rpc_ids(&state, &peer), vec![0, 1, 2, 4]);
    // A refusal backs the peer off for a while; then a query takes the room.
    run(&mut state, SnarkedAction::PeersQuery, at(10));
    assert_eq!(pending_rpc_ids(&state, &peer), vec![0, 1, 2, 4, 5]);
}

/// A WebRTC peer that advertised a deeper limit when it connected gets that
/// many ledger queries from one `PeersQuery`, and no more.
#[test]
fn a_webrtc_peer_takes_as_many_ledger_queries_as_it_advertised() {
    let (mut state, peer) = node(false, 20);
    let limit = crate::p2p::channels::rpc::peer_rpc_limit(Some(16));
    assert_eq!(limit, 16);
    state
        .p2p
        .ready_mut()
        .unwrap()
        .get_ready_peer_mut(&peer)
        .unwrap()
        .channels
        .rpc_pipeline
        .peer_limit = limit;
    run(&mut state, SnarkedAction::PeersQuery, at(0));
    assert_eq!(
        pending_rpc_ids(&state, &peer),
        (0..limit as u64).collect::<Vec<_>>()
    );
    let channels = &state.p2p.get_ready_peer(&peer).unwrap().channels;
    assert!(!channels.can_send_sync_request());
    assert_eq!(channels.sync_request_room(), 0);
    assert_eq!(channels.unanswered_rpc_ids().count(), limit);
}

/// A WebRTC request times out after its timeout without a byte from the
/// peer, counted from the last bytes; the peer is dropped at the ceiling
/// counted from the send. A libp2p peer keeps the plain timeouts.
#[test]
fn a_webrtc_request_times_out_on_silence_and_drops_the_peer_at_the_ceiling() {
    let (mut state, peer) = node(false, 2);
    run(&mut state, SnarkedAction::PeersQuery, at(0));
    assert_eq!(pending_rpc_ids(&state, &peer), vec![0, 1]);
    let timeout = |id| P2pChannelsRpcAction::Timeout { peer_id: peer, id };

    // Part of a long message came at 3 s: the 4 s ledger timeout counts
    // from there, for the slot (0) and the pipeline (1).
    run(
        &mut state,
        P2pPeerAction::Receiving { peer_id: peer },
        at(3),
    );
    for id in [0, 1] {
        assert!(!timeout(id).is_enabled(&state, at(6)));
        assert!(timeout(id).is_enabled(&state, at(7)));
    }
    for id in [0, 1] {
        let ran = run(&mut state, timeout(id), at(7));
        assert!(ran.iter().any(|a| a.contains("PeerQueryAddressError")
            && a.contains(&format!("rpc_id: {id}"))
            && a.contains("Timeout")));
    }
    // Silent since, but still connected: no second timeout before the
    // ceiling, 60 s after the send at 0.
    for id in [0, 1] {
        assert!(!timeout(id).is_enabled(&state, at(59)));
        assert!(timeout(id).is_enabled(&state, at(60)));
    }
    let ran = run(&mut state, timeout(1), at(60));
    assert!(ran
        .iter()
        .any(|a| a.contains("Init") && a.contains("TransitionFrontierRpcTimeout(LedgerQuery)")));
    assert!(state.p2p.get_ready_peer(&peer).is_none());

    let (mut libp2p, peer) = node(true, 1);
    run(&mut libp2p, SnarkedAction::PeersQuery, at(0));
    run(
        &mut libp2p,
        P2pPeerAction::Receiving { peer_id: peer },
        at(3),
    );
    let timeout = P2pChannelsRpcAction::Timeout {
        peer_id: peer,
        id: 0,
    };
    assert!(timeout.is_enabled(&libp2p, at(4)));
    run(&mut libp2p, timeout.clone(), at(4));
    assert!(timeout.is_enabled(&libp2p, at(8)));
}

/// Blocks between the root and the best tip go to a WebRTC peer five at a
/// time too, each starting its attempt only when the request can go; a
/// libp2p peer takes one.
#[test]
fn a_webrtc_peer_takes_five_block_requests_at_once() {
    use crate::transition_frontier::sync::{
        TransitionFrontierSyncAction as SyncAction, TransitionFrontierSyncBlockState as BlockState,
    };
    for is_libp2p in [false, true] {
        let (mut state, peer) = node(is_libp2p, 0);
        let template = template_block();
        let mut parent = block(&template, 100, None, 7);
        let mut chain = vec![];
        for height in 101..=107 {
            let next = block(&template, height, Some(&parent), 7);
            chain.push(next.clone());
            parent = next;
        }
        state.transition_frontier.sync = TransitionFrontierSyncState::BlocksPending {
            time: at(0),
            chain: chain
                .iter()
                .map(|b| BlockState::FetchPending {
                    time: at(0),
                    block_hash: b.hash().clone(),
                    attempts: Default::default(),
                })
                .collect(),
            root_snarked_ledger_updates: Default::default(),
            needed_protocol_states: Default::default(),
        };
        let mut started = 0;
        for (rpc_id, b) in chain.iter().enumerate() {
            let init = SyncAction::BlocksPeerQueryInit {
                peer_id: peer,
                hash: b.hash().clone(),
            };
            if !init.is_enabled(&state, at(0)) {
                break;
            }
            run(&mut state, init, at(0));
            // The request the init's effect sends.
            run(
                &mut state,
                P2pChannelsRpcAction::RequestSend {
                    peer_id: peer,
                    id: rpc_id as u64,
                    request: Box::new(P2pRpcRequest::Block(b.hash().clone())),
                    on_init: None,
                },
                at(0),
            );
            run(
                &mut state,
                SyncAction::BlocksPeerQueryPending {
                    hash: b.hash().clone(),
                    peer_id: peer,
                    rpc_id: rpc_id as u64,
                },
                at(0),
            );
            started += 1;
        }
        assert_eq!(started, if is_libp2p { 1 } else { 5 });
        let pending = chain
            .iter()
            .filter(|b| {
                state
                    .transition_frontier
                    .sync
                    .block_state(b.hash())
                    .and_then(|s| s.fetch_pending_from_peer_rpc_id(&peer))
                    .is_some()
            })
            .count();
        assert_eq!(pending, started);
    }
}

/// Five ledger queries sent at 0 (the slot and four pipelined) to a WebRTC
/// peer that keeps sending other data every second but never answers. The
/// ceiling, 60 s for a ledger query, is an absolute deadline from the send:
/// the first timeout comes at 60 s and the peer may be dropped at once, not
/// a second ceiling later. If the bytes stop at 30 s, the first timeout
/// comes on the silence schedule, at 34 s, and the drop still at 60 s.
#[test]
fn the_ceiling_holds_from_the_send_while_bytes_trickle_or_stop() {
    for stop in [None, Some(30u64)] {
        let (mut state, peer) = node(false, 8);
        run(&mut state, SnarkedAction::PeersQuery, at(0));
        let ids = (0..MAX_P2P_RPC_REMOTE_CONCURRENT_REQUESTS as u64).collect::<Vec<_>>();
        assert_eq!(pending_rpc_ids(&state, &peer), ids);
        let timeout = |id| P2pChannelsRpcAction::Timeout { peer_id: peer, id };
        let first = stop.map_or(60, |stop| stop + 4);
        for t in 1..first {
            if stop.is_none_or(|stop| t <= stop) {
                run(
                    &mut state,
                    P2pPeerAction::Receiving { peer_id: peer },
                    at(t),
                );
            }
            for id in &ids {
                assert!(!timeout(*id).is_enabled(&state, at(t)), "{id} at {t}");
            }
        }
        // The first timeouts, of the slot and of each pipelined request.
        for id in &ids {
            let ran = run(&mut state, timeout(*id), at(first));
            assert!(ran.iter().any(|a| a.contains("PeerQueryAddressError")
                && a.contains(&format!("rpc_id: {id}"))
                && a.contains("Timeout")));
        }
        let channels = &state.p2p.get_ready_peer(&peer).unwrap().channels;
        assert!(channels.rpc.timed_out_local_rpc().is_some());
        assert!(channels
            .rpc_pipeline
            .requests
            .iter()
            .all(|r| r.sent == at(0) && r.timed_out == Some(at(first))));
        // Bytes until the deadline do not move it.
        for t in first..60 {
            if stop.is_none_or(|stop| t <= stop) {
                run(
                    &mut state,
                    P2pPeerAction::Receiving { peer_id: peer },
                    at(t),
                );
            }
            for id in &ids {
                assert!(!timeout(*id).is_enabled(&state, at(t)), "{id} at {t}");
            }
        }
        for id in &ids {
            assert!(timeout(*id).is_enabled(&state, at(60)), "{id}");
        }
        let ran = run(&mut state, timeout(0), at(60));
        assert!(
            ran.iter()
                .any(|a| a.contains("Init")
                    && a.contains("TransitionFrontierRpcTimeout(LedgerQuery)"))
        );
        assert!(state.p2p.get_ready_peer(&peer).is_none());
    }
}

/// Answers may come in any order: each finds its own request, in the slot
/// or the pipeline, and frees exactly its room.
#[test]
fn out_of_order_answers_find_their_requests() {
    let (mut state, peer) = node(false, 8);
    run(&mut state, SnarkedAction::PeersQuery, at(0));
    assert_eq!(pending_rpc_ids(&state, &peer), vec![0, 1, 2, 3, 4]);
    let mut left = vec![0, 1, 2, 3, 4];
    for id in [3, 0, 4, 1] {
        let ran = run(
            &mut state,
            P2pChannelsRpcAction::ResponseReceived {
                peer_id: peer,
                id,
                response: None,
            },
            at(1),
        );
        assert!(ran
            .iter()
            .any(|a| a.contains("PeerQueryAddressError") && a.contains(&format!("rpc_id: {id}"))));
        left.retain(|left| *left != id);
        assert_eq!(pending_rpc_ids(&state, &peer), left);
        let channels = &state.p2p.get_ready_peer(&peer).unwrap().channels;
        assert_eq!(
            channels.unanswered_rpc_ids().collect::<Vec<_>>().len(),
            left.len()
        );
        // An answer to a request already answered is unexpected.
        assert!(!P2pChannelsRpcAction::ResponseReceived {
            peer_id: peer,
            id,
            response: None,
        }
        .is_enabled(&state, at(1)));
    }
    let channels = &state.p2p.get_ready_peer(&peer).unwrap().channels;
    assert!(channels.rpc.can_send_request());
    assert_eq!(channels.sync_request_room(), 4);
}

/// A new RPC channel, on a reconnect, starts with an empty pipeline: nothing
/// from the last connection counts against the peer's room.
#[test]
fn a_new_rpc_channel_starts_with_an_empty_pipeline() {
    for is_libp2p in [false, true] {
        let (mut state, peer) = node(is_libp2p, 0);
        let ready = state
            .p2p
            .ready_mut()
            .unwrap()
            .get_ready_peer_mut(&peer)
            .unwrap();
        ready
            .channels
            .rpc_pipeline
            .requests
            .push(crate::p2p::channels::rpc::P2pRpcPipelined {
                sent: at(0),
                id: 7,
                request: Box::new(P2pRpcRequest::LedgerQuery(
                    mina_p2p_messages::v2::LedgerHash::zero(),
                    mina_p2p_messages::v2::MinaLedgerSyncLedgerQueryStableV1::NumAccounts,
                )),
                timed_out: None,
            });
        ready.channels.rpc = P2pChannelsRpcState::Pending { time: at(0) };
        run(
            &mut state,
            P2pChannelsRpcAction::Ready { peer_id: peer },
            at(1),
        );
        let channels = &state.p2p.get_ready_peer(&peer).unwrap().channels;
        assert!(channels.rpc_pipeline.requests.is_empty());
        assert_eq!(channels.rpc_pipeline.enabled, !is_libp2p);
        // A new request may already have gone to the free slot (the ready
        // channel asks for a best tip); the old one does not count.
        assert!(!channels.is_rpc_unanswered(7));
        assert!(channels.unanswered_rpc_ids().count() <= 1);
    }
}
