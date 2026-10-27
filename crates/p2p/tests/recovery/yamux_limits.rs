//! Reducer-only tests: no sockets or network services are started.
use super::*;
use binprot::BinProtWrite;
use mina_p2p::{
    channels::rpc::{CatchupRequest, P2pRpcRequest, P2pRpcResponse},
    ConnectionAddr, P2pNetworkAction, P2pNetworkAuthState, P2pNetworkConnectionMuxState,
    P2pNetworkNoiseState, P2pNetworkNoiseStateInner, P2pNetworkRpcAction, P2pNetworkRpcState,
    P2pNetworkSelectAction, P2pNetworkStreamResetReason, P2pNetworkYamuxState, RpcMessage,
    YamuxFlags, YamuxFrame, YamuxFrameInner, YamuxStreamState,
};
use mina_p2p_messages::{
    list::List,
    rpc::{
        AnswerSyncLedgerQueryV2, CompletedSnarks, GetBestTipV2, GetCompletedSnarksV1,
        GetTransitionChainProofV1ForV2, GetTransitionChainV2,
    },
    rpc_kernel::{NeedsLength, QueryHeader, RpcMethod},
    v2::StateHash,
};

fn connection(s: &mut State) -> ConnectionAddr {
    connection_for(s, 1)
}

fn connection_for(s: &mut State, n: u8) -> ConnectionAddr {
    insert(s, n, ready_since(0));
    apply(s, P2pChannelsRpcAction::Ready { peer_id: peer(n) }, 0);
    let addr = ConnectionAddr {
        sock_addr: addr(n),
        incoming: false,
    };
    apply(
        s,
        P2pNetworkSchedulerAction::OutgoingConnect {
            addr: addr.sock_addr,
        },
        0,
    );
    let mut noise = P2pNetworkNoiseState::new(s.0.config.identity_pub_key.clone(), None);
    noise.inner = Some(P2pNetworkNoiseStateInner::Done {
        incoming: false,
        send_key: [0; 32].into(),
        recv_key: [0; 32].into(),
        recv_nonce: 0,
        send_nonce: 0,
        remote_pk: SecretKey::from_bytes([n; 32]).public_key(),
        remote_peer_id: peer(n),
    });
    let conn = s.0.network.scheduler.connections.get_mut(&addr).unwrap();
    conn.auth = Some(P2pNetworkAuthState::Noise(noise));
    conn.mux = Some(P2pNetworkConnectionMuxState::Yamux(
        P2pNetworkYamuxState::default(),
    ));
    addr
}

fn open(s: &mut State, addr: ConnectionAddr, id: u32) -> bool {
    // An incoming go-yamux WINDOW_UPDATE + SYN, with no RPC query yet.
    let mut bytes = vec![0, 1, 0, 1];
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    apply(
        s,
        P2pNetworkYamuxAction::IncomingData {
            addr,
            data: bytes.into(),
        },
        0,
    );
    let actions = apply_traced(s, P2pNetworkYamuxAction::IncomingFrame { addr }, 0);
    if let Some(reset) = actions.iter().find(|a| {
        matches!(a,
            P2pAction::Network(P2pNetworkAction::Scheduler(P2pNetworkSchedulerAction::StreamReset {
                stream_id, reason: P2pNetworkStreamResetReason::StreamLimit, ..
            })) if *stream_id == id
        )
    }) {
        assert!(!actions.iter().any(|a| matches!(
            a,
            P2pAction::Network(P2pNetworkAction::Select(
                P2pNetworkSelectAction::Init { .. }
            ))
        )));
        apply(s, reset.clone(), 0);
        assert!(!s.0.network.scheduler.connections[&addr]
            .streams
            .contains_key(&id));
        assert!(!s.0.network.scheduler.connections[&addr]
            .yamux_state()
            .unwrap()
            .streams
            .contains_key(&id));
        false
    } else {
        assert!(actions.iter().any(|a| matches!(
            a,
            P2pAction::Network(P2pNetworkAction::Select(
                P2pNetworkSelectAction::Init { .. }
            ))
        )));
        true
    }
}

fn query<M: RpcMethod>(
    s: &mut State,
    addr: ConnectionAddr,
    id: u32,
    input: M::Query,
) -> CatchupRequest {
    let mut bytes = Vec::new();
    NeedsLength(input).binprot_write(&mut bytes).unwrap();
    s.0.network
        .scheduler
        .rpc_incoming_streams
        .entry(peer(1))
        .or_default()
        .insert(id, P2pNetworkRpcState::new(addr, id));
    let actions = apply_traced(
        s,
        P2pNetworkRpcAction::IncomingMessage {
            addr,
            peer_id: peer(1),
            stream_id: id,
            message: RpcMessage::Query {
                header: QueryHeader {
                    tag: M::NAME.into(),
                    version: M::VERSION,
                    id: 1,
                },
                bytes: bytes.into(),
            },
        },
        0,
    );
    for action in actions {
        if let P2pAction::Channels(mina_p2p::channels::P2pChannelsAction::Rpc(
            ref request_action @ P2pChannelsRpcAction::RequestReceived { ref request, .. },
        )) = action
        {
            if let P2pRpcRequest::Catchup(ref request) = **request {
                assert!(request_action.is_enabled(&s.0, at(0)));
                let request = request.clone();
                apply(s, action, 0);
                return request;
            }
        }
    }
    panic!("public catch-up query must reach the responder")
}

#[test]
fn ordinary_peer_keeps_128_stream_limit_and_can_request_catchup() {
    let mut s = state(&[]);
    let addr = connection(&mut s);
    assert!(s.0.config.catchup_rpc.allows(&peer(1)));
    // Eight other incoming streams plus both recovery streams fit. Keep the
    // proof stream alive, as OCaml can until our heartbeat timeout expires.
    // OCaml: mina_networking.ml:266-272, gossip_net/libp2p.ml:894-910 at 84b8d969.
    for id in (2..=20).step_by(2) {
        assert!(open(&mut s, addr, id));
    }
    let hash = StateHash::zero().inner().0.clone();
    assert!(
        matches!(query::<GetTransitionChainProofV1ForV2>(&mut s, addr, 18, hash.clone()),
        CatchupRequest::TransitionChainProof(got) if got == hash)
    );
    let hashes: List<_> = [hash].into_iter().collect();
    assert!(
        matches!(query::<GetTransitionChainV2>(&mut s, addr, 20, hashes.clone()),
        CatchupRequest::TransitionChain(got) if got == hashes)
    );
    for id in (22..=256).step_by(2) {
        assert!(open(&mut s, addr, id));
    }
    assert!(!open(&mut s, addr, 258));
    assert_eq!(s.0.network.scheduler.connections[&addr].streams.len(), 128);
}

#[test]
fn allowlisted_peer_has_the_same_per_connection_stream_limit() {
    let mut s = state(&[]);
    s.0.config.catchup_rpc.peers.insert(peer(1));
    let addr = connection(&mut s);
    for id in (2..=256).step_by(2) {
        assert!(open(&mut s, addr, id));
    }
    assert!(!open(&mut s, addr, 258));
    assert_eq!(s.0.network.scheduler.connections[&addr].streams.len(), 128);
}

#[test]
fn another_allowlisted_peer_does_not_raise_this_peers_limit() {
    let mut s = state(&[]);
    s.0.config.catchup_rpc.peers.insert(peer(2));
    let addr = connection(&mut s);
    for id in (2..=256).step_by(2) {
        assert!(open(&mut s, addr, id));
    }
    assert!(!open(&mut s, addr, 258));
}

fn queue_one_megabyte(s: &mut State, addr: ConnectionAddr, stream_id: u32) -> Vec<P2pAction> {
    let connection = s.0.network.scheduler.connections.get_mut(&addr).unwrap();
    let P2pNetworkConnectionMuxState::Yamux(yamux) = connection.mux.as_mut().unwrap();
    let mut stream = YamuxStreamState::incoming();
    stream.window_theirs = 0; // the remote peer stopped reading
    yamux.streams.insert(stream_id, stream);
    apply_traced(
        s,
        P2pNetworkYamuxAction::OutgoingFrame {
            addr,
            frame: YamuxFrame {
                flags: YamuxFlags::empty(),
                stream_id,
                inner: YamuxFrameInner::Data(vec![0; 1_000_000].into()),
            },
        },
        0,
    )
}

fn budget_usage(s: &State, peer_id: PeerId) -> (usize, usize) {
    let scheduler = &s.0.network.scheduler;
    let (queued, _) = scheduler.queued_outgoing_bytes(&peer_id);
    let (reserved, _) = scheduler.reserved_rpc_response_bytes(&peer_id);
    (reserved, queued)
}

#[test]
fn many_slow_peers_cannot_exceed_node_queue_budget() {
    let mut s = state(&[]);
    s.0.config.limits =
        s.0.config
            .limits
            .with_yamux_pending_outgoing_total(Some(4_000_000))
            .with_yamux_pending_outgoing_peer_share(Some(1_000_000))
            .with_yamux_pending_outgoing_fair_share(Some(500_000))
            .with_yamux_pending_outgoing_reserve(Some(1_000_000));
    for n in 1..=24 {
        let addr = connection_for(&mut s, n);
        let actions = queue_one_megabyte(&mut s, addr, 2);
        let rejected = actions.iter().any(|action| {
            matches!(
                action,
                P2pAction::Network(P2pNetworkAction::Scheduler(
                    P2pNetworkSchedulerAction::StreamReset {
                        reason: P2pNetworkStreamResetReason::OutgoingQueueLimit,
                        ..
                    }
                ))
            )
        });
        assert_eq!(rejected, n > 3);
        assert!(s.0.network.scheduler.queued_outgoing_bytes(&peer(1)).0 <= 4_000_000);
        for action in actions.into_iter().filter(|action| {
            matches!(
                action,
                P2pAction::Network(P2pNetworkAction::Scheduler(
                    P2pNetworkSchedulerAction::StreamReset { .. }
                ))
            )
        }) {
            apply(&mut s, action, 0);
        }
        assert!(s.0.network.scheduler.queued_outgoing_bytes(&peer(1)).0 <= 4_000_000);
        assert!(s.0.network.scheduler.connections[&addr].closed.is_none());
    }
}

#[test]
fn one_slow_peer_cannot_take_the_whole_queue_budget() {
    let mut s = state(&[]);
    s.0.config.limits =
        s.0.config
            .limits
            .with_yamux_pending_outgoing_total(Some(4_000_000))
            .with_yamux_pending_outgoing_peer_share(Some(2_000_000))
            .with_yamux_pending_outgoing_fair_share(Some(500_000))
            .with_yamux_pending_outgoing_reserve(Some(1_000_000));
    let addr = connection(&mut s);
    for stream_id in [2, 4] {
        assert!(queue_one_megabyte(&mut s, addr, stream_id).is_empty());
    }
    let actions = queue_one_megabyte(&mut s, addr, 6);
    assert!(actions.iter().any(|action| matches!(
        action,
        P2pAction::Network(P2pNetworkAction::Scheduler(
            P2pNetworkSchedulerAction::StreamReset {
                reason: P2pNetworkStreamResetReason::OutgoingQueueLimit,
                ..
            }
        ))
    )));
    assert_eq!(
        s.0.network.scheduler.queued_outgoing_bytes(&peer(1)),
        (2_000_000, 2_000_000)
    );
}

#[test]
fn one_bootstrapping_peer_can_serve_one_hundred_reading_ledger_streams() {
    let mut s = state(&[]);
    let addr = connection(&mut s);
    for stream_id in (2..=200).step_by(2) {
        let connection = s.0.network.scheduler.connections.get_mut(&addr).unwrap();
        let P2pNetworkConnectionMuxState::Yamux(yamux) = connection.mux.as_mut().unwrap();
        let mut stream = YamuxStreamState::incoming();
        stream.window_theirs = 1_000_000;
        yamux.streams.insert(stream_id, stream);
        let actions = apply_traced(
            &mut s,
            P2pNetworkYamuxAction::OutgoingFrame {
                addr,
                frame: YamuxFrame {
                    flags: YamuxFlags::empty(),
                    stream_id,
                    inner: YamuxFrameInner::Data(vec![0; 1_000_000].into()),
                },
            },
            0,
        );
        assert!(!actions.iter().any(|action| matches!(
            action,
            P2pAction::Network(P2pNetworkAction::Scheduler(
                P2pNetworkSchedulerAction::StreamReset { .. }
            ))
        )));
    }
    assert_eq!(
        s.0.network.scheduler.queued_outgoing_bytes(&peer(1)),
        (0, 0)
    );
}

#[test]
fn four_borrowers_leave_room_for_a_new_peer() {
    let mut s = state(&[]);
    s.0.config.limits =
        s.0.config
            .limits
            .with_yamux_pending_outgoing_total(Some(8_000_000))
            .with_yamux_pending_outgoing_peer_share(Some(2_000_000))
            .with_yamux_pending_outgoing_fair_share(Some(1_000_000))
            .with_yamux_pending_outgoing_reserve(Some(2_000_000));
    for n in 1..=3 {
        let addr = connection_for(&mut s, n);
        assert!(queue_one_megabyte(&mut s, addr, 2).is_empty());
        assert!(queue_one_megabyte(&mut s, addr, 4).is_empty());
    }
    let fourth = connection_for(&mut s, 4);
    assert!(queue_one_megabyte(&mut s, fourth, 2).is_empty());
    let fifth = connection_for(&mut s, 5);
    assert!(queue_one_megabyte(&mut s, fifth, 2).is_empty());
    assert_eq!(
        s.0.network.scheduler.queued_outgoing_bytes(&peer(5)),
        (8_000_000, 1_000_000)
    );
}

#[test]
fn rejected_sixth_ordinary_request_has_no_reservation() {
    let mut s = state(&[]);
    s.0.config.catchup_rpc.enabled = false;
    let addr = connection(&mut s);
    let mut ids = Vec::new();
    for n in 0..6 {
        let stream_id = 2 + 2 * n;
        let mut bytes = Vec::new();
        NeedsLength(()).binprot_write(&mut bytes).unwrap();
        let mut rpc = P2pNetworkRpcState::new(addr, stream_id);
        rpc.is_incoming = true;
        s.0.network
            .scheduler
            .rpc_incoming_streams
            .entry(peer(1))
            .or_default()
            .insert(stream_id, rpc);
        let actions = apply_traced(
            &mut s,
            P2pNetworkRpcAction::IncomingMessage {
                addr,
                peer_id: peer(1),
                stream_id,
                message: RpcMessage::Query {
                    header: QueryHeader {
                        tag: GetBestTipV2::NAME.into(),
                        version: GetBestTipV2::VERSION,
                        id: n as u64,
                    },
                    bytes: bytes.into(),
                },
            },
            0,
        );
        if n < 5 {
            let action = actions
                .into_iter()
                .find(|action| {
                    matches!(
                        action,
                        P2pAction::Channels(mina_p2p::channels::P2pChannelsAction::Rpc(
                            P2pChannelsRpcAction::RequestReceived { .. }
                        ))
                    )
                })
                .expect("accepted query reaches channel");
            assert!(action.is_enabled(&s.0, at(0)));
            apply(&mut s, action, 0);
            ids.push(
                *s.0.network.scheduler.rpc_incoming_streams[&peer(1)][&stream_id]
                    .pending_incoming
                    .keys()
                    .next()
                    .unwrap(),
            );
            assert_eq!(
                budget_usage(&s, peer(1)),
                ((n + 1) as usize * 4 * 1024 * 1024, 0)
            );
        } else {
            assert!(!actions.iter().any(|action| matches!(
                action,
                P2pAction::Channels(mina_p2p::channels::P2pChannelsAction::Rpc(
                    P2pChannelsRpcAction::RequestReceived { .. }
                ))
            )));
            assert!(actions.iter().any(|action| matches!(
                action,
                P2pAction::Network(P2pNetworkAction::Rpc(
                    P2pNetworkRpcAction::OutgoingData { .. }
                ))
            )));
            assert!(
                s.0.network.scheduler.rpc_incoming_streams[&peer(1)][&stream_id]
                    .pending_incoming
                    .is_empty()
            );
            assert_eq!(budget_usage(&s, peer(1)), (20 * 1024 * 1024, 0));
        }
    }
    let unavailable = P2pChannelsRpcAction::ResponseSend {
        peer_id: peer(1),
        id: ids[0],
        response: None,
    };
    assert!(unavailable.is_enabled(&s.0, at(1)));
    let actions = apply_traced(&mut s, unavailable, 1);
    let response = actions
        .into_iter()
        .find(|action| {
            matches!(
                action,
                P2pAction::Network(P2pNetworkAction::Rpc(
                    P2pNetworkRpcAction::OutgoingResponse { .. }
                ))
            )
        })
        .expect("unavailable answer is sent on the wire");
    apply(&mut s, response, 1);
    assert_eq!(budget_usage(&s, peer(1)), (16 * 1024 * 1024, 0));
    apply(
        &mut s,
        P2pNetworkSchedulerAction::StreamReset {
            addr,
            peer_id: peer(1),
            stream_id: 4,
            protocol: None,
            reason: P2pNetworkStreamResetReason::OutgoingQueueLimit,
        },
        2,
    );
    assert_eq!(budget_usage(&s, peer(1)), (12 * 1024 * 1024, 0));
    apply(&mut s, P2pNetworkSchedulerAction::Prune { addr }, 3);
    assert_eq!(budget_usage(&s, peer(1)), (0, 0));
}

#[test]
fn one_hundred_parallel_ledger_queries_fit_work_budget() {
    use mina_p2p_messages::{bigint::BigInt, v2::MinaLedgerSyncLedgerQueryStableV1};
    let mut s = state(&[]);
    let addr = connection(&mut s);
    let mut bytes = Vec::new();
    NeedsLength((
        BigInt::zero(),
        MinaLedgerSyncLedgerQueryStableV1::NumAccounts,
    ))
    .binprot_write(&mut bytes)
    .unwrap();
    for n in 0..100u32 {
        let stream_id = 2 + 2 * n;
        let mut rpc = P2pNetworkRpcState::new(addr, stream_id);
        rpc.is_incoming = true;
        s.0.network
            .scheduler
            .rpc_incoming_streams
            .entry(peer(1))
            .or_default()
            .insert(stream_id, rpc);
        let actions = apply_traced(
            &mut s,
            P2pNetworkRpcAction::IncomingMessage {
                addr,
                peer_id: peer(1),
                stream_id,
                message: RpcMessage::Query {
                    header: QueryHeader {
                        tag: AnswerSyncLedgerQueryV2::NAME.into(),
                        version: AnswerSyncLedgerQueryV2::VERSION,
                        id: n as u64,
                    },
                    bytes: bytes.clone().into(),
                },
            },
            0,
        );
        let request = actions
            .into_iter()
            .find(|action| {
                matches!(
                    action,
                    P2pAction::Channels(mina_p2p::channels::P2pChannelsAction::Rpc(
                        P2pChannelsRpcAction::RequestReceived { .. }
                    ))
                )
            })
            .expect("ledger query admitted");
        assert!(request.is_enabled(&s.0, at(0)));
        apply(&mut s, request, 0);
    }
    assert_eq!(budget_usage(&s, peer(1)), (100 * 1024 * 1024, 0));
    let mina_p2p::channels::rpc::P2pChannelsRpcState::Ready { remote, .. } =
        &s.0.get_ready_peer(&peer(1)).unwrap().channels.rpc
    else {
        panic!("RPC channel ready")
    };
    assert_eq!(remote.pending_requests.len(), 100);
}

#[test]
fn response_send_releases_channel_owner_and_reservation() {
    let mut s = state(&[]);
    let addr = connection(&mut s);
    let mut bytes = Vec::new();
    NeedsLength(()).binprot_write(&mut bytes).unwrap();
    let mut rpc = P2pNetworkRpcState::new(addr, 2);
    rpc.is_incoming = true;
    s.0.network
        .scheduler
        .rpc_incoming_streams
        .entry(peer(1))
        .or_default()
        .insert(2, rpc);
    let actions = apply_traced(
        &mut s,
        P2pNetworkRpcAction::IncomingMessage {
            addr,
            peer_id: peer(1),
            stream_id: 2,
            message: RpcMessage::Query {
                header: QueryHeader {
                    tag: GetCompletedSnarksV1::NAME.into(),
                    version: GetCompletedSnarksV1::VERSION,
                    id: 9,
                },
                bytes: bytes.into(),
            },
        },
        0,
    );
    let request = actions
        .into_iter()
        .find(|action| {
            matches!(
                action,
                P2pAction::Channels(mina_p2p::channels::P2pChannelsAction::Rpc(
                    P2pChannelsRpcAction::RequestReceived { .. }
                ))
            )
        })
        .expect("handler dispatch");
    let P2pAction::Channels(mina_p2p::channels::P2pChannelsAction::Rpc(
        P2pChannelsRpcAction::RequestReceived { id, .. },
    )) = &request
    else {
        unreachable!()
    };
    let id = *id;
    apply(&mut s, request, 0);
    assert_eq!(budget_usage(&s, peer(1)), (8 * 1024 * 1024, 0));
    let actions = apply_traced(
        &mut s,
        P2pChannelsRpcAction::ResponseSend {
            peer_id: peer(1),
            id,
            response: Some(Box::new(P2pRpcResponse::CompletedSnarks(CompletedSnarks(
                List::new(),
            )))),
        },
        1,
    );
    let response = actions
        .into_iter()
        .find(|action| {
            matches!(
                action,
                P2pAction::Network(P2pNetworkAction::Rpc(
                    P2pNetworkRpcAction::OutgoingResponse { .. }
                ))
            )
        })
        .expect("encoded response");
    apply(&mut s, response, 1);
    assert_eq!(budget_usage(&s, peer(1)), (0, 0));
    let mina_p2p::channels::rpc::P2pChannelsRpcState::Ready { remote, .. } =
        &s.0.get_ready_peer(&peer(1)).unwrap().channels.rpc
    else {
        panic!("RPC channel ready")
    };
    assert!(remote.pending_requests.is_empty());
}

#[test]
fn unavailable_catchup_response_releases_reservation() {
    let mut s = state(&[]);
    let addr = connection(&mut s);
    let hash = StateHash::zero().inner().0.clone();
    let request: List<_> = [hash].into_iter().collect();
    assert!(matches!(
        query::<GetTransitionChainV2>(&mut s, addr, 2, request),
        CatchupRequest::TransitionChain(_)
    ));
    let id = *s.0.network.scheduler.rpc_incoming_streams[&peer(1)][&2]
        .pending_incoming
        .keys()
        .next()
        .unwrap();
    assert_eq!(budget_usage(&s, peer(1)), (1024 * 1024, 0));
    let unavailable = P2pChannelsRpcAction::ResponseSend {
        peer_id: peer(1),
        id,
        response: None,
    };
    assert!(unavailable.is_enabled(&s.0, at(1)));
    let actions = apply_traced(&mut s, unavailable, 1);
    let response = actions
        .into_iter()
        .find(|action| {
            matches!(
                action,
                P2pAction::Network(P2pNetworkAction::Rpc(
                    P2pNetworkRpcAction::OutgoingResponse { .. }
                ))
            )
        })
        .expect("catchup unavailable answer is sent");
    apply(&mut s, response, 1);
    assert_eq!(budget_usage(&s, peer(1)), (0, 0));
}

#[test]
fn stalled_response_stream_is_reset_after_thirty_seconds() {
    let mut s = state(&[]);
    let addr = connection(&mut s);
    s.0.network
        .scheduler
        .rpc_incoming_streams
        .entry(peer(1))
        .or_default()
        .insert(2, P2pNetworkRpcState::new(addr, 2));
    assert!(queue_one_megabyte(&mut s, addr, 2).is_empty());
    assert!(!tick(&mut s, 30).iter().any(|action| matches!(
        action,
        P2pAction::Network(P2pNetworkAction::Scheduler(
            P2pNetworkSchedulerAction::StreamReset {
                reason: P2pNetworkStreamResetReason::OutgoingQueueLimit,
                ..
            }
        ))
    )));
    assert!(tick(&mut s, 31).iter().any(|action| matches!(
        action,
        P2pAction::Network(P2pNetworkAction::Scheduler(
            P2pNetworkSchedulerAction::StreamReset {
                reason: P2pNetworkStreamResetReason::OutgoingQueueLimit,
                ..
            }
        ))
    )));
}

#[test]
fn progressing_reader_cannot_hold_response_beyond_hard_deadline() {
    let mut s = state(&[]);
    let addr = connection(&mut s);
    s.0.network
        .scheduler
        .rpc_incoming_streams
        .entry(peer(1))
        .or_default()
        .insert(2, P2pNetworkRpcState::new(addr, 2));
    assert!(queue_one_megabyte(&mut s, addr, 2).is_empty());
    let P2pNetworkConnectionMuxState::Yamux(yamux) =
        s.0.network
            .scheduler
            .connections
            .get_mut(&addr)
            .unwrap()
            .mux
            .as_mut()
            .unwrap();
    let stream = yamux.streams.get_mut(&2).unwrap();
    stream.pending_since = Some(at(100)); // a small receive-window update made progress
    assert_eq!(stream.pending_first_at, Some(at(0)));
    assert!(!tick(&mut s, 120).iter().any(|action| matches!(
        action,
        P2pAction::Network(P2pNetworkAction::Scheduler(
            P2pNetworkSchedulerAction::StreamReset { stream_id: 2, .. }
        ))
    )));
    let reset = tick(&mut s, 121)
        .into_iter()
        .find(|action| {
            matches!(
                action,
                P2pAction::Network(P2pNetworkAction::Scheduler(
                    P2pNetworkSchedulerAction::StreamReset { stream_id: 2, .. }
                ))
            )
        })
        .expect("hard deadline resets progressing slow reader");
    apply(&mut s, reset, 121);
    assert_eq!(budget_usage(&s, peer(1)), (0, 0));
    assert!(s.0.network.scheduler.connections[&addr].closed.is_none());
}

#[test]
fn dispatched_work_without_output_expires_even_with_heartbeats() {
    let mut s = state(&[]);
    let addr = connection(&mut s);
    let mut bytes = Vec::new();
    NeedsLength(()).binprot_write(&mut bytes).unwrap();
    let mut rpc = P2pNetworkRpcState::new(addr, 2);
    rpc.is_incoming = true;
    s.0.network
        .scheduler
        .rpc_incoming_streams
        .entry(peer(1))
        .or_default()
        .insert(2, rpc);
    let P2pNetworkConnectionMuxState::Yamux(yamux) =
        s.0.network
            .scheduler
            .connections
            .get_mut(&addr)
            .unwrap()
            .mux
            .as_mut()
            .unwrap();
    yamux.streams.insert(2, YamuxStreamState::incoming());
    let actions = apply_traced(
        &mut s,
        P2pNetworkRpcAction::IncomingMessage {
            addr,
            peer_id: peer(1),
            stream_id: 2,
            message: RpcMessage::Query {
                header: QueryHeader {
                    tag: GetBestTipV2::NAME.into(),
                    version: GetBestTipV2::VERSION,
                    id: 7,
                },
                bytes: bytes.into(),
            },
        },
        0,
    );
    let request = actions
        .into_iter()
        .find(|action| {
            matches!(
                action,
                P2pAction::Channels(mina_p2p::channels::P2pChannelsAction::Rpc(
                    P2pChannelsRpcAction::RequestReceived { .. }
                ))
            )
        })
        .expect("handler dispatch");
    apply(&mut s, request, 0);
    assert_eq!(budget_usage(&s, peer(1)), (4 * 1024 * 1024, 0));
    s.0.network
        .scheduler
        .rpc_incoming_streams
        .get_mut(&peer(1))
        .unwrap()
        .get_mut(&2)
        .unwrap()
        .last_received = Some(at(119));
    assert!(!tick(&mut s, 120).iter().any(|action| matches!(
        action,
        P2pAction::Network(P2pNetworkAction::Scheduler(
            P2pNetworkSchedulerAction::StreamReset { stream_id: 2, .. }
        ))
    )));
    let reset = tick(&mut s, 121)
        .into_iter()
        .find(|action| {
            matches!(
                action,
                P2pAction::Network(P2pNetworkAction::Scheduler(
                    P2pNetworkSchedulerAction::StreamReset { stream_id: 2, .. }
                ))
            )
        })
        .expect("in-flight work hard deadline");
    apply(&mut s, reset, 121);
    assert_eq!(budget_usage(&s, peer(1)), (0, 0));
}

fn data_frame(stream_id: u32, len: u32, body: bool) -> Vec<u8> {
    let mut bytes = vec![0, 0, 0, 0];
    bytes.extend_from_slice(&stream_id.to_be_bytes());
    bytes.extend_from_slice(&len.to_be_bytes());
    if body {
        bytes.resize(12 + len as usize, 7);
    }
    bytes
}

fn overflow(actions: &[P2pAction], id: u32) -> bool {
    actions.iter().any(|a| {
        matches!(a,
            P2pAction::Network(P2pNetworkAction::Scheduler(P2pNetworkSchedulerAction::Error {
                error: mina_p2p::P2pNetworkConnectionError::YamuxOverflow(stream_id), ..
            })) if *stream_id == id
        )
    })
}

fn window(s: &State, addr: ConnectionAddr, id: u32) -> u32 {
    s.0.network.scheduler.connections[&addr]
        .yamux_state()
        .unwrap()
        .streams[&id]
        .window_ours
}

#[test]
fn a_data_frame_bigger_than_the_window_is_refused_at_its_header() {
    let mut s = state(&[]);
    let addr = connection(&mut s);
    assert!(open(&mut s, addr, 2));
    let header = data_frame(2, window(&s, addr, 2) + 1, false);
    let actions = apply_traced(
        &mut s,
        P2pNetworkYamuxAction::IncomingData {
            addr,
            data: header.into(),
        },
        0,
    );
    assert!(overflow(&actions, 2));
    let yamux = s.0.network.scheduler.connections[&addr]
        .yamux_state()
        .unwrap();
    assert!(yamux.incoming.is_empty());
    // Nothing more is buffered for the closing connection.
    let buffered = yamux.buffer.len();
    apply(
        &mut s,
        P2pNetworkYamuxAction::IncomingData {
            addr,
            data: vec![0; 1 << 16].into(),
        },
        0,
    );
    let yamux = s.0.network.scheduler.connections[&addr]
        .yamux_state()
        .unwrap();
    assert_eq!(yamux.buffer.len(), buffered);
}

#[test]
fn data_frames_past_the_window_close_the_connection() {
    let mut s = state(&[]);
    let addr = connection(&mut s);
    assert!(open(&mut s, addr, 2));
    let full = window(&s, addr, 2);
    // Each fits the window on its own, both together don't, and taking the
    // first leaves more than half the window, so no window update is sent.
    let first = full / 2 - 1;
    let mut bytes = data_frame(2, first, true);
    bytes.extend(data_frame(2, full - first + 1, true));
    apply(
        &mut s,
        P2pNetworkYamuxAction::IncomingData {
            addr,
            data: bytes.into(),
        },
        0,
    );
    let actions = apply_traced(&mut s, P2pNetworkYamuxAction::IncomingFrame { addr }, 0);
    assert!(!overflow(&actions, 2));
    assert_eq!(window(&s, addr, 2), full - first);
    let actions = apply_traced(&mut s, P2pNetworkYamuxAction::IncomingFrame { addr }, 0);
    assert!(overflow(&actions, 2));
}

/// A peer that sends what our window updates allow, on every stream at once,
/// never breaks flow control, gets all its data through, and can't make the
/// connection's receive windows add up to more than the connection cap plus
/// one initial window per stream.
#[test]
fn honest_senders_on_every_stream_stay_within_the_connection_window() {
    use mina_p2p::{INITIAL_WINDOW_SIZE, MAX_CONNECTION_RECEIVE_WINDOW};
    let mut s = state(&[]);
    let addr = connection(&mut s);
    let ids: Vec<u32> = (2..=256).step_by(2).collect();
    for &id in &ids {
        assert!(open(&mut s, addr, id));
    }
    let windows = |s: &State| -> Vec<u32> { ids.iter().map(|id| window(s, addr, *id)).collect() };
    // What each stream's sender may still send, as it learns from us.
    let mut credit = windows(&s);
    let mut sent = vec![0usize; ids.len()];
    const EACH: usize = 8 << 20;
    const CHUNK: u32 = 256 << 10;
    let mut peak = 0usize;
    while sent.iter().any(|sent| *sent < EACH) {
        for (i, &id) in ids.iter().enumerate() {
            let len = CHUNK.min(credit[i]).min((EACH - sent[i]) as u32);
            if len == 0 {
                continue;
            }
            apply(
                &mut s,
                P2pNetworkYamuxAction::IncomingData {
                    addr,
                    data: data_frame(id, len, true).into(),
                },
                0,
            );
            let actions = apply_traced(&mut s, P2pNetworkYamuxAction::IncomingFrame { addr }, 0);
            assert!(!overflow(&actions, id), "stream {id}");
            credit[i] -= len;
            sent[i] += len as usize;
            for action in actions {
                if matches!(
                    &action,
                    P2pAction::Network(P2pNetworkAction::Yamux(
                        P2pNetworkYamuxAction::OutgoingFrame { frame, .. }
                    )) if matches!(frame.inner, YamuxFrameInner::WindowUpdate { .. })
                ) {
                    // The peer gets what the update grants when sent.
                    let before = window(&s, addr, id);
                    apply(&mut s, action, 0);
                    credit[i] += window(&s, addr, id) - before;
                }
            }
            peak = peak.max(windows(&s).iter().map(|w| *w as usize).sum());
        }
        assert!(
            credit.iter().zip(&sent).any(|(c, s)| *c > 0 || *s >= EACH),
            "every sender is stalled"
        );
    }
    let yamux = s.0.network.scheduler.connections[&addr]
        .yamux_state()
        .unwrap();
    assert!(
        peak <= MAX_CONNECTION_RECEIVE_WINDOW + ids.len() * INITIAL_WINDOW_SIZE as usize,
        "{peak}"
    );
    assert!(yamux.limit() < 256 << 20);
    // Some window did grow past the initial one: the cap limits the
    // connection, not each transfer's speed.
    assert!(peak > ids.len() * INITIAL_WINDOW_SIZE as usize, "{peak}");
}
