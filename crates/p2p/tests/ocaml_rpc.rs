//! What an OCaml node's RPCs see of a Rust node, from a libp2p node that
//! speaks the same RPC protocol (`libp2p-rpc-behaviour`).

use std::time::Duration;

use binprot::BinProtRead;
use mina_p2p::PeerId;
use mina_p2p_messages::{
    rpc::{GetTransitionKnowledgeV1ForV2, VERSIONED_RPC_MENU},
    rpc_kernel::{Error, ResponsePayload, RpcErrorVersion, RpcMethod},
};
use p2p_testing::{
    cluster::{Cluster, ClusterBuilder, ClusterEvent},
    futures::StreamExt,
    libp2p::swarm::SwarmEvent,
    libp2p_node::{Libp2pBehaviourEvent, Libp2pNodeConfig, Libp2pNodeId},
    libp2p_rpc_behaviour::{Event, Received, StreamId},
    rust_node::{RustNodeConfig, RustNodeId},
    stream::ClusterStreamExt,
    utils::{run_cluster, try_wait_for_nodes_to_connect, wait_for_all_nodes_to_listen},
};

/// Runs the cluster until the libp2p node `id` receives something on an RPC
/// stream, for at most `time`.
async fn next_rpc_received(
    cluster: &mut Cluster,
    id: Libp2pNodeId,
    time: Duration,
) -> Option<Received> {
    let mut stream = cluster.stream().take_during(time);
    while let Some(event) = stream.next().await {
        if let ClusterEvent::Libp2p {
            id: event_id,
            event:
                SwarmEvent::Behaviour(Libp2pBehaviourEvent::Rpc((
                    _,
                    Event::Stream {
                        received: received @ (Received::Menu(_) | Received::Response { .. }),
                        ..
                    },
                ))),
        } = event
        {
            if event_id == id {
                return Some(received);
            }
        }
    }
    None
}

fn assert_peer_is_ready(cluster: &Cluster, id: RustNodeId, peer_id: PeerId) {
    let state = cluster.rust_node(id).state();
    let peer = state.peers.get(&peer_id).expect("peer should exist");
    assert!(
        peer.status.as_ready().is_some(),
        "peer's status should be ready, but it is {:#?}",
        peer.status
    );
}

/// An OCaml node asks `__Versioned_rpc.Menu` on every RPC stream before its
/// query. It gets OCaml 4.0.2's menu; an RPC on the menu that we don't serve
/// gets `Unimplemented_rpc`, and the connection stays up.
#[tokio::test]
async fn menu_and_unimplemented_rpc_keep_the_connection() -> anyhow::Result<()> {
    unimplemented_for_peer(false, false).await
}

#[tokio::test]
async fn disabled_switch_does_not_serve_listed_peer() -> anyhow::Result<()> {
    unimplemented_for_peer(false, true).await
}

#[tokio::test]
async fn enabled_switch_does_not_serve_unlisted_peer() -> anyhow::Result<()> {
    unimplemented_for_peer(true, false).await
}

async fn unimplemented_for_peer(enabled: bool, listed: bool) -> anyhow::Result<()> {
    let mut cluster = ClusterBuilder::default()
        .ports_with_len(10)
        .total_duration(Duration::from_secs(30))
        .start()
        .await?;

    let libp2p_node = cluster.add_libp2p_node(Libp2pNodeConfig::default())?;
    let libp2p_peer_id = cluster.peer_id(libp2p_node);
    let rust_node = cluster.add_rust_node(RustNodeConfig {
        catchup_rpc: mina_p2p::P2pCatchupRpcConfig {
            enabled,
            peers: if listed {
                [libp2p_peer_id].into_iter().collect()
            } else {
                [mina_p2p::identity::SecretKey::deterministic(999)
                    .public_key()
                    .peer_id()]
                .into_iter()
                .collect()
            },
        },
        ..Default::default()
    })?;
    let rust_peer_id = cluster.peer_id(rust_node);

    assert!(wait_for_all_nodes_to_listen(&mut cluster, [rust_node], Duration::from_secs(2)).await);
    cluster.connect(libp2p_node, rust_node)?;
    assert!(
        try_wait_for_nodes_to_connect(
            &mut cluster,
            [(rust_node, libp2p_peer_id)],
            Duration::from_secs(5)
        )
        .await?
    );

    // A new outgoing RPC stream does the handshake and asks for the menu.
    let rust_libp2p_id: p2p_testing::libp2p::PeerId = rust_peer_id.try_into()?;
    cluster
        .libp2p_node_mut(libp2p_node)
        .swarm_mut()
        .behaviour_mut()
        .rpc
        .open(rust_libp2p_id, 0);
    let Some(Received::Menu(menu)) =
        next_rpc_received(&mut cluster, libp2p_node, Duration::from_secs(5)).await
    else {
        panic!("no menu");
    };
    let expected = VERSIONED_RPC_MENU
        .iter()
        .map(|(tag, version)| (tag.to_string(), *version))
        .collect::<Vec<_>>();
    assert_eq!(menu, expected);

    // An RPC from the menu that we don't serve.
    cluster
        .libp2p_node_mut(libp2p_node)
        .swarm_mut()
        .behaviour_mut()
        .rpc
        .query::<GetTransitionKnowledgeV1ForV2>(rust_libp2p_id, StreamId::Outgoing(0), 1, ())?;
    let Some(Received::Response { header, bytes }) =
        next_rpc_received(&mut cluster, libp2p_node, Duration::from_secs(5)).await
    else {
        panic!("no response");
    };
    assert_eq!(header.id, 1);
    type Payload = ResponsePayload<<GetTransitionKnowledgeV1ForV2 as RpcMethod>::Response>;
    let payload = Payload::binprot_read(&mut bytes.as_slice())?;
    assert_eq!(
        payload.0,
        Err(Error::Unimplemented_rpc(
            GetTransitionKnowledgeV1ForV2::NAME.into(),
            RpcErrorVersion::Version(1)
        ))
    );

    // The connection is kept.
    run_cluster(&mut cluster, Duration::from_secs(2)).await;
    assert_peer_is_ready(&cluster, rust_node, libp2p_peer_id);

    Ok(())
}

/// OCaml can send 100 queries before reading any response. IDs are local to
/// each RPC connection, so two streams may use the same IDs concurrently.
#[tokio::test]
async fn default_catchup_batch_and_colliding_stream_ids_are_answered_individually(
) -> anyhow::Result<()> {
    use mina_p2p::channels::rpc::{CatchupResponse, P2pChannelsRpcAction, P2pRpcResponse};
    use mina_p2p_messages::{list::List, rpc::GetTransitionChainV2};
    let mut cluster = ClusterBuilder::default()
        .ports_with_len(10)
        .total_duration(Duration::from_secs(30))
        .start()
        .await?;
    let client = cluster.add_libp2p_node(Libp2pNodeConfig::default())?;
    let client_id = cluster.peer_id(client);
    let server = cluster.add_rust_node(RustNodeConfig::default())?;
    let server_id = cluster.peer_id(server).try_into()?;
    assert!(wait_for_all_nodes_to_listen(&mut cluster, [server], Duration::from_secs(2)).await);
    cluster.connect(client, server)?;
    assert!(
        try_wait_for_nodes_to_connect(&mut cluster, [(server, client_id)], Duration::from_secs(5))
            .await?
    );
    for stream in 0..2 {
        cluster
            .libp2p_node_mut(client)
            .swarm_mut()
            .behaviour_mut()
            .rpc
            .open(server_id, stream);
        assert!(matches!(
            next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await,
            Some(Received::Menu(_))
        ));
    }
    for id in 1..=50 {
        cluster
            .libp2p_node_mut(client)
            .swarm_mut()
            .behaviour_mut()
            .rpc
            .query::<GetTransitionChainV2>(server_id, StreamId::Outgoing(0), id, List::new())?;
        cluster
            .libp2p_node_mut(client)
            .swarm_mut()
            .behaviour_mut()
            .rpc
            .query::<GetTransitionChainV2>(server_id, StreamId::Outgoing(1), id, List::new())?;
    }
    run_cluster(&mut cluster, Duration::from_secs(2)).await;
    let streams = &cluster
        .rust_node(server)
        .state()
        .network
        .scheduler
        .rpc_incoming_streams[&client_id];
    let pending = streams
        .values()
        .enumerate()
        .flat_map(|(stream, s)| {
            s.pending_incoming
                .iter()
                .map(move |(id, h)| (*id, h.header.clone(), stream))
        })
        .collect::<Vec<_>>();
    assert_eq!(pending.len(), 100);
    cluster
        .libp2p_node_mut(client)
        .swarm_mut()
        .behaviour_mut()
        .rpc
        .query::<GetTransitionChainV2>(server_id, StreamId::Outgoing(0), 51, List::new())?;
    let Some(Received::Response { header, bytes }) =
        next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await
    else {
        panic!("no overload response")
    };
    assert_eq!(header.id, 51);
    assert!(
        ResponsePayload::<List<mina_p2p_messages::common::StateHashV1>>::binprot_read(
            &mut bytes.as_slice()
        )?
        .0
        .is_err()
    );
    // Exercise responses in reverse order, with an observable payload for
    // each stream. No response may clear another request or another stream.
    for (id, _header, stream) in pending.into_iter().rev() {
        let response = if stream == 1 {
            CatchupResponse::TransitionChain(Some(List::new()))
        } else {
            CatchupResponse::TransitionChain(None)
        };
        assert!(cluster.rust_node_mut(server).dispatch_action(
            P2pChannelsRpcAction::ResponseSend {
                peer_id: client_id,
                id,
                response: Some(Box::new(P2pRpcResponse::Catchup(Box::new(response)))),
            }
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut events = cluster.stream().take_during(Duration::from_secs(5));
    while let Some(event) = events.next().await {
        if let ClusterEvent::Libp2p {
            id,
            event:
                SwarmEvent::Behaviour(Libp2pBehaviourEvent::Rpc((
                    _,
                    Event::Stream {
                        stream_id,
                        received: Received::Response { header, bytes },
                        ..
                    },
                ))),
        } = event
        {
            if id != client {
                continue;
            }
            let stream = match stream_id {
                StreamId::Outgoing(n) => n,
                _ => panic!("wrong direction"),
            };
            assert!(seen.insert((stream, header.id)), "duplicate response");
            assert_eq!(
                bytes.as_slice(),
                if stream == 0 {
                    &[0, 1, 0][..]
                } else {
                    &[0, 2, 1, 0][..]
                }
            );
            if seen.len() == 100 {
                break;
            }
        }
    }
    drop(events);
    assert_eq!(
        seen,
        (0..2)
            .flat_map(|s| (1..=50).map(move |id| (s, id)))
            .collect()
    );
    // Ordinary peers can fetch an ancestry proof without the experimental
    // switch or allowlist. Replay the captured OCaml bytes on this wire path.
    let fixture =
        include_bytes!("../../p2p-messages/tests/files/mesa/get_transition_chain_proof_v1.bin");
    type Proof = mina_p2p_messages::rpc::GetTransitionChainProofV1ForV2;
    let proof = ResponsePayload::<<Proof as RpcMethod>::Response>::binprot_read(&mut &fixture[..])?
        .0
        .unwrap()
        .0;
    let query = proof.as_ref().unwrap().0.clone();
    cluster
        .libp2p_node_mut(client)
        .swarm_mut()
        .behaviour_mut()
        .rpc
        .query::<Proof>(server_id, StreamId::Outgoing(0), 98, query.clone())?;
    run_cluster(&mut cluster, Duration::from_millis(500)).await;
    let state = cluster.rust_node(server).state();
    let rpc_state = &state.get_ready_peer(&client_id).unwrap().channels.rpc;
    let request = rpc_state
        .remote_todo_requests_iter()
        .chain(rpc_state.remote_pending_requests_iter())
        .find(|r| {
            matches!(&r.request, mina_p2p::channels::rpc::P2pRpcRequest::Catchup(
            mina_p2p::channels::rpc::CatchupRequest::TransitionChainProof(h)) if h == &query)
        })
        .expect("chain proof routed for an ordinary peer");
    let id = request.id;
    assert!(cluster
        .rust_node_mut(server)
        .dispatch_action(P2pChannelsRpcAction::ResponseSend {
            peer_id: client_id,
            id,
            response: Some(Box::new(P2pRpcResponse::Catchup(Box::new(
                CatchupResponse::TransitionChainProof(proof)
            )))),
        }));
    let Some(Received::Response { header, bytes }) =
        next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await
    else {
        panic!("no chain proof response")
    };
    assert_eq!(header.id, 98);
    assert_eq!(bytes, fixture);

    // An unavailable legacy optional RPC must also produce a real reply.
    cluster
        .libp2p_node_mut(client)
        .swarm_mut()
        .behaviour_mut()
        .rpc
        .query::<mina_p2p_messages::rpc::GetBestTipV2>(server_id, StreamId::Outgoing(0), 99, ())?;
    run_cluster(&mut cluster, Duration::from_millis(500)).await;
    let pending = cluster
        .rust_node(server)
        .state()
        .network
        .scheduler
        .rpc_incoming_streams[&client_id]
        .values()
        .flat_map(|s| s.pending_incoming.iter())
        .find(|(_, h)| h.header.id == 99)
        .map(|(id, _)| *id)
        .expect("best tip request");
    assert!(cluster
        .rust_node_mut(server)
        .dispatch_action(P2pChannelsRpcAction::ResponseSend {
            peer_id: client_id,
            id: pending,
            response: None,
        }));
    let Some(Received::Response { header, bytes }) =
        next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await
    else {
        panic!("no unavailable reply")
    };
    assert_eq!(header.id, 99);
    assert_eq!(bytes, vec![0, 1, 0]);
    assert_peer_is_ready(&cluster, server, client_id);
    Ok(())
}

/// OCaml's Connection.with_close opens a fresh stream for each call/batch.
/// More than the default ten-stream limit must work on one connection when
/// earlier streams have completed (I19b).
#[tokio::test]
async fn completed_rpc_streams_do_not_exhaust_the_incoming_limit() -> anyhow::Result<()> {
    let mut cluster = ClusterBuilder::default()
        .ports_with_len(10)
        .total_duration(Duration::from_secs(60))
        .start()
        .await?;
    let client = cluster.add_libp2p_node(Libp2pNodeConfig::default())?;
    let client_id = cluster.peer_id(client);
    let server = cluster.add_rust_node(RustNodeConfig::default())?;
    let server_id = cluster.peer_id(server).try_into()?;
    assert!(wait_for_all_nodes_to_listen(&mut cluster, [server], Duration::from_secs(2)).await);
    cluster.connect(client, server)?;
    assert!(
        try_wait_for_nodes_to_connect(&mut cluster, [(server, client_id)], Duration::from_secs(5))
            .await?
    );
    run_cluster(&mut cluster, Duration::from_millis(500)).await;
    for n in 0..25 {
        cluster
            .libp2p_node_mut(client)
            .swarm_mut()
            .behaviour_mut()
            .rpc
            .open(server_id, n);
        assert!(
            matches!(
                next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await,
                Some(Received::Menu(_))
            ),
            "stream {n} failed to negotiate"
        );
        let stream_id = cluster
            .rust_node(server)
            .state()
            .network
            .scheduler
            .rpc_incoming_streams[&client_id]
            .keys()
            .copied()
            .max()
            .unwrap();
        let addr = cluster
            .rust_node(server)
            .state()
            .network
            .scheduler
            .rpc_incoming_streams[&client_id][&stream_id]
            .addr;
        cluster
            .libp2p_node_mut(client)
            .swarm_mut()
            .behaviour_mut()
            .rpc
            .close(server_id, StreamId::Outgoing(n));
        if n % 2 == 1 {
            // go-yamux v4.0.0 stream.go:sendClose encodes WindowUpdate/FIN
            // with a zero delta. This independent header exercises the exact
            // frame type OCaml sends, in addition to rust-yamux's close above.
            let mut fin = vec![0, 1, 0, 4];
            fin.extend_from_slice(&stream_id.to_be_bytes());
            fin.extend_from_slice(&[0; 4]);
            assert!(cluster.rust_node_mut(server).dispatch_action(
                mina_p2p::P2pNetworkYamuxAction::IncomingData {
                    addr,
                    data: fin.into()
                }
            ));
            assert!(!cluster
                .rust_node(server)
                .state()
                .network
                .scheduler
                .rpc_incoming_streams[&client_id]
                .contains_key(&stream_id));
        }
        run_cluster(&mut cluster, Duration::from_millis(200)).await;
        // A delayed ACK/window update must not recreate the retired stream.
        let mut late = vec![0, 1, 0, 2];
        late.extend_from_slice(&stream_id.to_be_bytes());
        late.extend_from_slice(&1024u32.to_be_bytes());
        assert!(cluster.rust_node_mut(server).dispatch_action(
            mina_p2p::P2pNetworkYamuxAction::IncomingData {
                addr,
                data: late.into()
            }
        ));
        let scheduler = &cluster.rust_node(server).state().network.scheduler;
        let (_, connection) = scheduler.find_peer(&client_id).unwrap();
        assert!(
            !connection.streams.contains_key(&stream_id),
            "closed stream {n} retained by scheduler"
        );
        assert!(
            !connection
                .yamux_state()
                .unwrap()
                .streams
                .contains_key(&stream_id),
            "closed stream {n} retained by yamux"
        );
        assert!(
            !scheduler.rpc_incoming_streams[&client_id].contains_key(&stream_id),
            "closed stream {n} retained by RPC"
        );
        assert_peer_is_ready(&cluster, server, client_id);
    }
    Ok(())
}

#[tokio::test]
async fn closed_rpc_stream_cancels_only_its_pending_requests() -> anyhow::Result<()> {
    cancelled_batch(false).await
}

#[tokio::test]
async fn reset_rpc_stream_cancels_only_its_pending_requests() -> anyhow::Result<()> {
    cancelled_batch(true).await
}

async fn cancelled_batch(reset: bool) -> anyhow::Result<()> {
    use mina_p2p::channels::rpc::P2pChannelsRpcAction;
    use mina_p2p_messages::{list::List, rpc::GetTransitionChainV2};
    let mut cluster = ClusterBuilder::default()
        .ports_with_len(10)
        .total_duration(Duration::from_secs(30))
        .start()
        .await?;
    let client = cluster.add_libp2p_node(Libp2pNodeConfig::default())?;
    let client_id = cluster.peer_id(client);
    let server = cluster.add_rust_node(RustNodeConfig::default())?;
    let server_id = cluster.peer_id(server).try_into()?;
    assert!(wait_for_all_nodes_to_listen(&mut cluster, [server], Duration::from_secs(2)).await);
    cluster.connect(client, server)?;
    assert!(
        try_wait_for_nodes_to_connect(&mut cluster, [(server, client_id)], Duration::from_secs(5))
            .await?
    );
    for n in 0..2 {
        cluster
            .libp2p_node_mut(client)
            .swarm_mut()
            .behaviour_mut()
            .rpc
            .open(server_id, n);
        assert!(matches!(
            next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await,
            Some(Received::Menu(_))
        ));
        for id in 1..=50 {
            cluster
                .libp2p_node_mut(client)
                .swarm_mut()
                .behaviour_mut()
                .rpc
                .query::<GetTransitionChainV2>(server_id, StreamId::Outgoing(n), id, List::new())?;
        }
        run_cluster(&mut cluster, Duration::from_millis(300)).await;
    }
    let pending_ids = |cluster: &Cluster| {
        let rpc = &cluster.rust_node(server).state().peers[&client_id]
            .status
            .as_ready()
            .unwrap()
            .channels
            .rpc;
        rpc.remote_todo_requests_iter()
            .chain(rpc.remote_pending_requests_iter())
            .map(|r| r.id)
            .collect::<std::collections::BTreeSet<_>>()
    };
    let before = pending_ids(&cluster);
    assert_eq!(before.len(), 100);
    for &id in &before {
        assert!(cluster.rust_node_mut(server).dispatch_action(
            P2pChannelsRpcAction::ResponsePending {
                peer_id: client_id,
                id
            }
        ));
    }
    let rpc = &mut cluster
        .libp2p_node_mut(client)
        .swarm_mut()
        .behaviour_mut()
        .rpc;
    if reset {
        rpc.reset(server_id, StreamId::Outgoing(0));
    } else {
        rpc.close(server_id, StreamId::Outgoing(0));
    }
    run_cluster(&mut cluster, Duration::from_millis(500)).await;
    let after = pending_ids(&cluster);
    assert_eq!(
        after.len(),
        50,
        "abandoned queries must release the channel work budget"
    );
    assert!(after.is_subset(&before));
    for &id in before.difference(&after) {
        assert!(
            !cluster
                .rust_node_mut(server)
                .dispatch_action(P2pChannelsRpcAction::ResponseSend {
                    peer_id: client_id,
                    id,
                    response: None,
                }),
            "a late async result must not revive a cancelled request"
        );
    }
    cluster
        .libp2p_node_mut(client)
        .swarm_mut()
        .behaviour_mut()
        .rpc
        .reset(server_id, StreamId::Outgoing(1));
    run_cluster(&mut cluster, Duration::from_millis(500)).await;
    assert!(pending_ids(&cluster).is_empty());
    cluster
        .libp2p_node_mut(client)
        .swarm_mut()
        .behaviour_mut()
        .rpc
        .open(server_id, 2);
    assert!(matches!(
        next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await,
        Some(Received::Menu(_))
    ));
    for id in 1..=100 {
        cluster
            .libp2p_node_mut(client)
            .swarm_mut()
            .behaviour_mut()
            .rpc
            .query::<GetTransitionChainV2>(server_id, StreamId::Outgoing(2), id, List::new())?;
    }
    run_cluster(&mut cluster, Duration::from_millis(500)).await;
    assert_eq!(
        pending_ids(&cluster).len(),
        100,
        "the next full batch must fit"
    );
    assert_peer_is_ready(&cluster, server, client_id);
    Ok(())
}

/// EOF may share a data frame with the final RPC bytes. Dispatch those bytes
/// before retiring the stream, then cancel the request if no reply is ready.
#[tokio::test]
async fn final_rpc_payload_is_processed_before_eof_cleanup() -> anyhow::Result<()> {
    use binprot::BinProtWrite;
    use mina_p2p::P2pNetworkYamuxAction;
    use mina_p2p_messages::rpc_kernel::{Message, NeedsLength, Query};
    let mut cluster = ClusterBuilder::default()
        .ports_with_len(10)
        .total_duration(Duration::from_secs(30))
        .start()
        .await?;
    let client = cluster.add_libp2p_node(Libp2pNodeConfig::default())?;
    let client_id = cluster.peer_id(client);
    let server = cluster.add_rust_node(RustNodeConfig::default())?;
    let server_id = cluster.peer_id(server).try_into()?;
    assert!(wait_for_all_nodes_to_listen(&mut cluster, [server], Duration::from_secs(2)).await);
    cluster.connect(client, server)?;
    assert!(
        try_wait_for_nodes_to_connect(&mut cluster, [(server, client_id)], Duration::from_secs(5))
            .await?
    );
    cluster
        .libp2p_node_mut(client)
        .swarm_mut()
        .behaviour_mut()
        .rpc
        .open(server_id, 0);
    assert!(matches!(
        next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await,
        Some(Received::Menu(_))
    ));
    let network = &cluster.rust_node(server).state().network;
    let rpc = network.scheduler.rpc_incoming_streams[&client_id]
        .values()
        .next()
        .unwrap();
    let (addr, stream_id, before) = (rpc.addr, rpc.stream_id, network.next_incoming_rpc_id);
    let mut message = Vec::new();
    Message::<()>::Query(Query {
        tag: GetTransitionKnowledgeV1ForV2::NAME.into(),
        version: GetTransitionKnowledgeV1ForV2::VERSION,
        id: 77,
        data: NeedsLength(()),
    })
    .binprot_write(&mut message)?;
    let mut payload = (message.len() as u64).to_le_bytes().to_vec();
    payload.extend(message);
    // Yamux Data/FIN header, independently encoded from the specification.
    let mut frame = vec![0, 0, 0, 4];
    frame.extend(stream_id.to_be_bytes());
    frame.extend((payload.len() as u32).to_be_bytes());
    frame.extend(payload);
    cluster
        .libp2p_node_mut(client)
        .swarm_mut()
        .behaviour_mut()
        .rpc
        .close(server_id, StreamId::Outgoing(0));
    assert!(cluster
        .rust_node_mut(server)
        .dispatch_action(P2pNetworkYamuxAction::IncomingData {
            addr,
            data: frame.into()
        }));
    let state = cluster.rust_node(server).state();
    assert_eq!(
        state.network.next_incoming_rpc_id,
        before + 1,
        "the final query must have been parsed"
    );
    assert!(state.network.scheduler.rpc_incoming_streams[&client_id].is_empty());
    let rpc = &state.peers[&client_id]
        .status
        .as_ready()
        .unwrap()
        .channels
        .rpc;
    assert_eq!(
        rpc.remote_todo_requests_iter().count() + rpc.remote_pending_requests_iter().count(),
        0
    );
    assert_peer_is_ready(&cluster, server, client_id);
    Ok(())
}

/// OCaml 4.0.2 closes its RPC writer without calling the helper's CloseStream.
/// A burst must negotiate while completed streams await heartbeat expiry.
#[tokio::test]
async fn silent_ocaml_streams_expire_while_active_streams_survive() -> anyhow::Result<()> {
    let mut cluster = ClusterBuilder::default()
        .ports_with_len(10)
        .total_duration(Duration::from_secs(60))
        .start()
        .await?;
    let client = cluster.add_libp2p_node(Libp2pNodeConfig::default())?;
    let client_id = cluster.peer_id(client);
    let server = cluster.add_rust_node(RustNodeConfig {
        catchup_rpc: mina_p2p::P2pCatchupRpcConfig {
            enabled: true,
            peers: [client_id].into_iter().collect(),
        },
        ..Default::default()
    })?;
    let server_id = cluster.peer_id(server).try_into()?;
    assert!(wait_for_all_nodes_to_listen(&mut cluster, [server], Duration::from_secs(2)).await);
    cluster.connect(client, server)?;
    assert!(
        try_wait_for_nodes_to_connect(&mut cluster, [(server, client_id)], Duration::from_secs(5))
            .await?
    );
    for n in 0..25 {
        cluster
            .libp2p_node_mut(client)
            .swarm_mut()
            .behaviour_mut()
            .rpc
            .open(server_id, n);
        assert!(
            matches!(
                next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await,
                Some(Received::Menu(_))
            ),
            "stream {n} failed"
        );
        let rpc = &mut cluster
            .libp2p_node_mut(client)
            .swarm_mut()
            .behaviour_mut()
            .rpc;
        if n == 0 || n == 24 {
            rpc.query::<GetTransitionKnowledgeV1ForV2>(server_id, StreamId::Outgoing(n), 99, ())?;
        }
        if n != 24 {
            rpc.stop_heartbeats(server_id, StreamId::Outgoing(n));
        }
    }
    run_cluster(&mut cluster, Duration::from_millis(300)).await;
    let streams = &cluster
        .rust_node(server)
        .state()
        .network
        .scheduler
        .rpc_incoming_streams[&client_id];
    assert_eq!(streams.len(), 25);
    assert_eq!(
        streams
            .values()
            .map(|s| s.pending_incoming.len())
            .sum::<usize>(),
        2
    );
    let active_id = *streams.keys().max().unwrap();
    run_cluster(&mut cluster, Duration::from_secs(33)).await;
    let state = cluster.rust_node(server).state();
    let streams = &state.network.scheduler.rpc_incoming_streams[&client_id];
    assert_eq!(streams.len(), 1, "silent streams must expire without FIN");
    assert!(
        streams.contains_key(&active_id),
        "heartbeats keep the active stream alive"
    );
    assert_eq!(streams[&active_id].pending_incoming.len(), 1);
    let channel = &state.peers[&client_id]
        .status
        .as_ready()
        .unwrap()
        .channels
        .rpc;
    assert_eq!(
        channel
            .remote_todo_requests_iter()
            .chain(channel.remote_pending_requests_iter())
            .count(),
        1
    );
    assert_peer_is_ready(&cluster, server, client_id);
    cluster
        .libp2p_node_mut(client)
        .swarm_mut()
        .behaviour_mut()
        .rpc
        .open(server_id, 25);
    assert!(matches!(
        next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await,
        Some(Received::Menu(_))
    ));
    Ok(())
}

/// Five knowledge calls fit. The sixth gets Uncaught_exn, consumes no pending
/// slot and leaves the stream
/// usable. Invalid contents queries also get their terminal wrapped error.
#[tokio::test]
async fn served_limits_reject_without_pending_work_or_disconnect() -> anyhow::Result<()> {
    use mina_p2p::channels::rpc::{CatchupResponse, P2pChannelsRpcAction, P2pRpcResponse};
    use mina_p2p_messages::{bigint::BigInt, list::List, rpc::AnswerSyncLedgerQueryV2, v2};
    let mut cluster = ClusterBuilder::default()
        .ports_with_len(10)
        .total_duration(Duration::from_secs(30))
        .start()
        .await?;
    let client = cluster.add_libp2p_node(Libp2pNodeConfig::default())?;
    let client_id = cluster.peer_id(client);
    let server = cluster.add_rust_node(RustNodeConfig::default())?;
    let server_id = cluster.peer_id(server).try_into()?;
    assert!(wait_for_all_nodes_to_listen(&mut cluster, [server], Duration::from_secs(2)).await);
    cluster.connect(client, server)?;
    assert!(
        try_wait_for_nodes_to_connect(&mut cluster, [(server, client_id)], Duration::from_secs(5))
            .await?
    );
    cluster
        .libp2p_node_mut(client)
        .swarm_mut()
        .behaviour_mut()
        .rpc
        .open(server_id, 0);
    assert!(matches!(
        next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await,
        Some(Received::Menu(_))
    ));
    for id in 1..=5 {
        cluster
            .libp2p_node_mut(client)
            .swarm_mut()
            .behaviour_mut()
            .rpc
            .query::<GetTransitionKnowledgeV1ForV2>(server_id, StreamId::Outgoing(0), id, ())?;
        run_cluster(&mut cluster, Duration::from_millis(100)).await;
        let local_id = cluster.rust_node(server).state().peers[&client_id]
            .status
            .as_ready()
            .unwrap()
            .channels
            .rpc
            .remote_todo_requests_iter()
            .next()
            .expect("within budget")
            .id;
        assert!(cluster.rust_node_mut(server).dispatch_action(
            P2pChannelsRpcAction::ResponseSend {
                peer_id: client_id,
                id: local_id,
                response: Some(Box::new(P2pRpcResponse::Catchup(Box::new(
                    CatchupResponse::TransitionKnowledge(List::new())
                )))),
            }
        ));
        let Some(Received::Response { header, bytes }) =
            next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await
        else {
            panic!("missing admitted response")
        };
        assert_eq!(header.id, id);
        assert_eq!(bytes.as_slice(), [0, 1, 0]);
    }
    for id in 6..=8 {
        cluster
            .libp2p_node_mut(client)
            .swarm_mut()
            .behaviour_mut()
            .rpc
            .query::<GetTransitionKnowledgeV1ForV2>(server_id, StreamId::Outgoing(0), id, ())?;
        let Some(Received::Response { header, bytes }) =
            next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await
        else {
            panic!("missing rate rejection")
        };
        assert_eq!(header.id, id);
        // Async RPC Error, Uncaught_exn, Sexp.List with two Sexp.Atom fields:
        // Failure and the same capacity diagnostic as gossip_net/libp2p.ml.
        let mut input = bytes.as_slice();
        assert_eq!(u8::binprot_read(&mut input)?, 1);
        assert_eq!(u8::binprot_read(&mut input)?, 3);
        assert_eq!(u8::binprot_read(&mut input)?, 1);
        assert_eq!(binprot::Nat0::binprot_read(&mut input)?.0, 2);
        assert_eq!(u8::binprot_read(&mut input)?, 0);
        assert_eq!(
            mina_p2p_messages::string::CharString::binprot_read(&mut input)?.to_string(),
            "Failure"
        );
        assert_eq!(u8::binprot_read(&mut input)?, 0);
        assert!(
            mina_p2p_messages::string::CharString::binprot_read(&mut input)?
                .to_string()
                .starts_with("peer exceeded capacity: /ip4/")
        );
        assert!(input.is_empty());
    }
    for (id, length, child_depth) in [
        (9, 28u64, None),
        (10, 3, None),
        (11, 0, None),
        (12, 34, Some(8u64)),
    ] {
        let addr = v2::MerkleAddressBinableArgStableV1(
            length.into(),
            vec![0; length.div_ceil(8) as usize].into(),
        );
        let (query, reason) = match child_depth {
            Some(depth) => (
                v2::MinaLedgerSyncLedgerQueryStableV1::WhatChildHashes(addr, depth.into()),
                "Invalid address in What_child_hashes request",
            ),
            None => (
                v2::MinaLedgerSyncLedgerQueryStableV1::WhatContents(addr),
                "Requested too big of a subtree at once",
            ),
        };
        cluster
            .libp2p_node_mut(client)
            .swarm_mut()
            .behaviour_mut()
            .rpc
            .query::<AnswerSyncLedgerQueryV2>(
                server_id,
                StreamId::Outgoing(0),
                id,
                (BigInt::zero(), query),
            )?;
        let Some(Received::Response { header, bytes }) =
            next_rpc_received(&mut cluster, client, Duration::from_secs(5)).await
        else {
            panic!("missing ledger rejection")
        };
        assert_eq!(header.id, id);
        let mut input = bytes.as_slice();
        let response =
            ResponsePayload::<<AnswerSyncLedgerQueryV2 as RpcMethod>::Response>::binprot_read(
                &mut input,
            )?
            .0
            .unwrap()
            .0;
        let error = response.0.unwrap_err().to_string();
        assert_eq!(
            error,
            format!(
                "\"Refusing to answer sync ledger query for ledger_hash: 0x{}. Error: {reason}\"",
                "0".repeat(64)
            )
        );
        assert!(input.is_empty());
    }
    let peer = cluster
        .rust_node(server)
        .state()
        .get_ready_peer(&client_id)
        .unwrap();
    assert_eq!(peer.channels.rpc.remote_todo_requests_iter().count(), 0);
    assert_eq!(peer.channels.rpc.remote_pending_requests_iter().count(), 0);
    assert!(cluster
        .rust_node(server)
        .state()
        .network
        .scheduler
        .rpc_incoming_streams[&client_id]
        .values()
        .all(|s| s.pending_incoming.is_empty()));
    assert_peer_is_ready(&cluster, server, client_id);
    Ok(())
}

#[tokio::test]
async fn webrtc_peers_keep_full_answers_without_serving_budgets() -> anyhow::Result<()> {
    use mina_core::Substate;
    use mina_p2p::{
        channels::{
            rpc::{
                BestTipWithProof, P2pChannelsRpcAction as Rpc, P2pChannelsRpcState,
                P2pRpcLocalState, P2pRpcRemoteState, P2pRpcRequest, P2pRpcResponse, RpcChannelMsg,
                ServedRpc, StagedLedgerAuxAndPendingCoinbases,
            },
            streaming_rpc::{
                P2pChannelsStreamingRpcAction as A, P2pChannelsStreamingRpcState as S,
                P2pStreamingRpcLocalState as L, P2pStreamingRpcRemoteState as R,
                P2pStreamingRpcRequest, P2pStreamingRpcResponseFull, StreamingRpcChannelMsg,
            },
            ChannelMsg, P2pChannelsEffectfulAction,
        },
        P2pEffectfulAction, P2pPeerState, P2pPeerStatus, P2pPeerStatusReady, P2pState,
    };
    use mina_p2p_messages::{binprot::BinProtWrite, rpc::GetBestTipV2Response, v2};
    use p2p_testing::redux::{Action, State, Store};
    use redux::{ActionWithMeta, Dispatcher};
    use std::sync::{Arc, Mutex};
    // Capture the actual send effects, including all streaming parts.
    static SENT: Mutex<Vec<(PeerId, ChannelMsg)>> = Mutex::new(Vec::new());
    const PEERS: u8 = 32;
    fn peer(n: u8) -> PeerId {
        PeerId::from_bytes([n; 32])
    }
    fn reducer(
        state: &mut State,
        action: &ActionWithMeta<Action>,
        dispatcher: &mut Dispatcher<Action, State>,
    ) {
        let meta = action.meta().clone();
        let time = meta.time();
        let p2p = state.state_mut();
        if !p2p.peers.contains_key(&peer(1)) {
            for n in 1..=PEERS {
                let mut ready = P2pPeerStatusReady::new(true, time, &Default::default());
                ready.channels.rpc = P2pChannelsRpcState::Ready {
                    time,
                    local: P2pRpcLocalState::WaitingForRequest { time },
                    remote: P2pRpcRemoteState {
                        pending_requests: Default::default(),
                        last_responded: time,
                    },
                };
                ready.channels.streaming_rpc = S::Ready {
                    time,
                    local: L::WaitingForRequest { time },
                    remote: R::WaitingForRequest { time },
                    remote_last_responded: time,
                };
                p2p.peers.insert(
                    peer(n),
                    P2pPeerState {
                        is_libp2p: false,
                        dial_opts: None,
                        status: P2pPeerStatus::Ready(Box::new(ready)),
                        identify: None,
                    },
                );
                // Even an exhausted identity budget left by a libp2p connection
                // must not refuse the same identity on WebRTC.
                for method in [
                    ServedRpc::InitialPeers,
                    ServedRpc::BestTip,
                    ServedRpc::StagedLedger,
                ] {
                    let ip = [10, 0, 0, n].into();
                    assert!(p2p
                        .network
                        .serving_limits
                        .admit(method, peer(n), ip, time, 1));
                    assert!(p2p.network.serving_limits.admit(
                        method,
                        peer(n),
                        ip,
                        time,
                        method.capacity() - 1
                    ));
                }
            }
        }
        if let Action::P2p(action) = action.action() {
            P2pState::reducer(
                Substate::new(state, dispatcher),
                meta.with_action(action.clone()),
            )
            .unwrap();
        }
    }
    fn effects(_store: &mut Store, action: ActionWithMeta<Action>) {
        if let Action::P2pEffectful(P2pEffectfulAction::Channels(
            P2pChannelsEffectfulAction::MessageSend { peer_id, msg, .. },
        )) = action.action()
        {
            SENT.lock().unwrap().push((*peer_id, msg.clone()));
        }
    }
    fn encoded(value: &impl BinProtWrite) -> Vec<u8> {
        let mut bytes = Vec::new();
        value.binprot_write(&mut bytes).unwrap();
        bytes
    }
    SENT.lock().unwrap().clear();
    let mut cluster = ClusterBuilder::default().ports_with_len(10).start().await?;
    let server = cluster.add_rust_node(
        RustNodeConfig::default()
            .with_override_reducer(reducer)
            .with_override(effects),
    )?;
    run_cluster(&mut cluster, Duration::from_millis(100)).await;
    let budget_before =
        serde_json::to_value(&cluster.rust_node(server).state().network.serving_limits)?;
    let bytes = include_bytes!("../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin");
    let tip = ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut &bytes[..])?
        .0?
        .0
        .unwrap();
    let best = BestTipWithProof {
        best_tip: Arc::new(tip.data),
        proof: (tip.proof.0, Arc::new(tip.proof.1)),
    };
    // Small deterministic multi-part answer: this test checks transport/admission,
    // not ledger validity. The best-tip answer above comes from an OCaml capture.
    let staged = Arc::new(StagedLedgerAuxAndPendingCoinbases {
        scan_state: v2::TransactionSnarkScanStateStableV2 {
            scan_state: v2::TransactionSnarkScanStateStableV2ScanState {
                trees: (v2::TransactionSnarkScanStateStableV2ScanStateTreesA::Leaf(vec![(
                    v2::ParallelScanWeightStableV1 { base: 1.into(), merge: 0.into() },
                    v2::TransactionSnarkScanStateStableV2ScanStateTreesABaseT1::Empty,
                )]), mina_p2p_messages::list::List::new()),
                acc: None, curr_job_seq_no: 0.into(), max_base_jobs: 1.into(), delay: 2.into(),
            },
            previous_incomplete_zkapp_updates: (mina_p2p_messages::list::List::new(),
                v2::TransactionSnarkScanStateStableV2PreviousIncompleteZkappUpdates1::Border_block_continued_in_the_next_tree(false)),
        },
        staged_ledger_hash: v2::LedgerHash::zero(),
        pending_coinbase: v2::MinaBasePendingCoinbaseStableV2 {
            tree: v2::MinaBasePendingCoinbaseMerkleTreeVersionedStableV2 {
                indexes: mina_p2p_messages::list::List::new(), depth: 5.into(),
                tree: v2::MinaBasePendingCoinbaseMerkleTreeVersionedStableV2Tree::Hash(v2::MinaBasePendingCoinbaseHashVersionedStableV1(v2::MinaBasePendingCoinbaseHashBuilderStableV1(mina_p2p_messages::bigint::BigInt::zero())).into()),
            }, pos_list: mina_p2p_messages::list::List::new(), new_pos: v2::MinaBasePendingCoinbaseStackIdStableV1(0.into()),
        }, needed_blocks: mina_p2p_messages::list::List::new(),
    });
    for round in 0..32u64 {
        let cases = [
            (
                P2pRpcRequest::InitialPeers,
                P2pRpcResponse::InitialPeers(mina_p2p_messages::list::List::new()),
            ),
            (
                P2pRpcRequest::BestTipWithProof,
                P2pRpcResponse::BestTipWithProof(best.clone()),
            ),
            (
                P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(v2::StateHash::zero()),
                P2pRpcResponse::StagedLedgerAuxAndPendingCoinbasesAtBlock(staged.clone()),
            ),
        ];
        // Keep all 32 peers' requests in flight together, then complete them.
        for n in 1..=PEERS {
            for (offset, (request, _)) in cases.iter().enumerate() {
                assert!(cluster
                    .rust_node_mut(server)
                    .dispatch_action(Rpc::RequestReceived {
                        peer_id: peer(n),
                        id: round * 3 + offset as u64,
                        request: Box::new(request.clone())
                    }));
            }
            assert!(cluster
                .rust_node_mut(server)
                .dispatch_action(A::RequestReceived {
                    peer_id: peer(n),
                    id: round,
                    request: Box::new(P2pStreamingRpcRequest::StagedLedgerParts(
                        v2::StateHash::zero()
                    ))
                }));
        }
        assert!(
            SENT.lock().unwrap().is_empty(),
            "no automatic budget refusals"
        );
        for n in 1..=PEERS {
            for (offset, (_, response)) in cases.iter().enumerate() {
                let id = round * 3 + offset as u64;
                assert!(cluster
                    .rust_node_mut(server)
                    .dispatch_action(Rpc::ResponseSend {
                        peer_id: peer(n),
                        id,
                        response: Some(Box::new(response.clone()))
                    }));
                let sent = SENT.lock().unwrap().pop().unwrap();
                assert_eq!(sent.0, peer(n));
                assert_eq!(
                    encoded(&sent.1),
                    encoded(&ChannelMsg::Rpc(RpcChannelMsg::Response(
                        id,
                        Some(response.clone())
                    )))
                );
            }
            assert!(cluster
                .rust_node_mut(server)
                .dispatch_action(A::ResponsePending {
                    peer_id: peer(n),
                    id: round
                }));
            assert!(cluster
                .rust_node_mut(server)
                .dispatch_action(A::ResponseSendInit {
                    peer_id: peer(n),
                    id: round,
                    response: Some(P2pStreamingRpcResponseFull::StagedLedgerParts(
                        staged.clone()
                    ))
                }));
            for _ in 0..3 {
                assert!(cluster
                    .rust_node_mut(server)
                    .dispatch_action(A::ResponsePartNextSend {
                        peer_id: peer(n),
                        id: round
                    }));
            }
            let sent = std::mem::take(&mut *SENT.lock().unwrap());
            assert_eq!(sent.len(), 4);
            let mut receiver = mina_p2p::channels::streaming_rpc::staged_ledger_parts::StagedLedgerPartsReceiveProgress::default();
            for (index, (target, msg)) in sent.into_iter().enumerate() {
                if index > 0 {
                    assert!(receiver.set_next_pending(redux::Timestamp::ZERO));
                }
                assert_eq!(target, peer(n));
                let ChannelMsg::StreamingRpc(StreamingRpcChannelMsg::Response(id, Some(
                    mina_p2p::channels::streaming_rpc::P2pStreamingRpcResponse::StagedLedgerParts(part)))) = msg else { panic!("expected full streaming answer") };
                assert_eq!(id, round);
                assert!(receiver.update(redux::Timestamp::ZERO, part));
            }
            let mina_p2p::channels::streaming_rpc::staged_ledger_parts::StagedLedgerPartsReceiveProgress::Success { data, .. } = receiver else { panic!("incomplete answer") };
            assert_eq!(encoded(&*data), encoded(&*staged));
            assert!(matches!(
                cluster
                    .rust_node(server)
                    .state()
                    .get_ready_peer(&peer(n))
                    .unwrap()
                    .channels
                    .streaming_rpc,
                S::Ready {
                    remote: R::Responded { .. },
                    ..
                }
            ));
        }
    }
    assert_eq!(
        serde_json::to_value(&cluster.rust_node(server).state().network.serving_limits)?,
        budget_before,
        "WebRTC must never charge or expire OCaml budgets"
    );
    // WebRTC still rejects a sixth outstanding ordinary request per peer.
    for id in 100..105 {
        assert!(cluster
            .rust_node_mut(server)
            .dispatch_action(Rpc::RequestReceived {
                peer_id: peer(1),
                id,
                request: Box::new(P2pRpcRequest::InitialPeers),
            }));
    }
    assert!(!cluster
        .rust_node_mut(server)
        .dispatch_action(Rpc::RequestReceived {
            peer_id: peer(1),
            id: 105,
            request: Box::new(P2pRpcRequest::InitialPeers),
        }));
    for id in 100..105 {
        assert!(cluster
            .rust_node_mut(server)
            .dispatch_action(Rpc::ResponseSend {
                peer_id: peer(1),
                id,
                response: Some(Box::new(P2pRpcResponse::InitialPeers(
                    mina_p2p_messages::list::List::new()
                ))),
            }));
    }
    SENT.lock().unwrap().clear();
    // Preserve the old terminal-None regression for an unavailable ledger,
    // now reached through normal pending work rather than a serving budget.
    assert!(cluster
        .rust_node_mut(server)
        .dispatch_action(A::RequestReceived {
            peer_id: peer(1),
            id: 99,
            request: Box::new(P2pStreamingRpcRequest::StagedLedgerParts(
                v2::StateHash::zero()
            ))
        }));
    assert!(cluster
        .rust_node_mut(server)
        .dispatch_action(A::ResponsePending {
            peer_id: peer(1),
            id: 99
        }));
    assert!(cluster
        .rust_node_mut(server)
        .dispatch_action(A::ResponseSendInit {
            peer_id: peer(1),
            id: 99,
            response: None
        }));
    assert!(matches!(
        SENT.lock().unwrap().pop().unwrap().1,
        ChannelMsg::StreamingRpc(StreamingRpcChannelMsg::Response(99, None))
    ));
    let channel = &cluster
        .rust_node(server)
        .state()
        .get_ready_peer(&peer(1))
        .unwrap()
        .channels
        .streaming_rpc;
    assert!(matches!(
        channel,
        S::Ready {
            remote: R::Responded { id: 99, .. },
            ..
        }
    ));
    assert!(channel.remote_todo_request().is_none());
    assert!(channel.remote_pending_request().is_none());
    Ok(())
}
