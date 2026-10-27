//! A yamux reset or a failed protocol negotiation closes one stream, not the
//! connection, unless the connection can't work without that stream (I16).
//! Our pubsub stream, reset by the peer, is opened again. Bytes written beyond
//! the peer's yamux window keep their order.

use std::{sync::Mutex, time::Duration};

use mina_core::Substate;
use mina_p2p::{
    token::{PingAlgorithm, StreamKind},
    ConnectionAddr, Data, P2pAction, P2pNetworkAction, P2pNetworkPubsubAction,
    P2pNetworkSelectAction, P2pState, P2pTimeouts, PeerId, SelectKind, StreamId, YamuxFlags,
    YamuxStreamKind,
};
use p2p_testing::{
    cluster::{Cluster, ClusterBuilder},
    event::{allow_disconnections, RustNodeEvent},
    redux::{Action, State},
    rust_node::{RustNodeConfig, RustNodeId},
    utils::{try_run_cluster, try_wait_for_all_nodes_with_value},
};
use redux::{ActionWithMeta, Dispatcher};

/// A stream id the dialling node opens: the yamux client's ids are odd.
const EXTRA_STREAM: StreamId = 101;
/// A stream the dialling node opens and never negotiates (see `silent_reducer`).
const SILENT_STREAM: StreamId = 103;

/// Starts two nodes, has the second dial the first, and waits until both have
/// the other's RPC channel ready, so that every stream a connection needs is
/// open. Returns the nodes and each one's address of the connection.
async fn connected_pair(
    cluster: &mut Cluster,
    listener_config: RustNodeConfig,
    dialer_config: RustNodeConfig,
) -> anyhow::Result<[(RustNodeId, ConnectionAddr); 2]> {
    let listener = cluster.add_rust_node(listener_config)?;
    let dialer = cluster.add_rust_node(dialer_config)?;
    let listener_id = cluster.peer_id(listener);
    let dialer_id = cluster.peer_id(dialer);

    // Let the listener start listening before dialling it.
    try_run_cluster(cluster, Duration::from_secs(1))
        .await
        .map_err(|event| anyhow::anyhow!("error event while starting: {event:?}"))?;
    cluster.connect(dialer, listener)?;

    let ready = try_wait_for_all_nodes_with_value(
        cluster,
        [(listener, dialer_id), (dialer, listener_id)],
        Duration::from_secs(10),
        |event| match event {
            RustNodeEvent::RpcChannelReady { peer_id } => Some(peer_id),
            _ => None,
        },
    )
    .await
    .map_err(|event| anyhow::anyhow!("error event while connecting: {event:?}"))?;
    assert!(ready, "the RPC channels should be ready on both nodes");

    Ok([
        (listener, connection_addr(cluster, listener, dialer_id)),
        (dialer, connection_addr(cluster, dialer, listener_id)),
    ])
}

fn connection_addr(cluster: &Cluster, node: RustNodeId, peer_id: PeerId) -> ConnectionAddr {
    let (addr, _) = cluster
        .rust_node(node)
        .state()
        .network
        .scheduler
        .find_peer(&peer_id)
        .expect("the node should have a connection to the peer");
    *addr
}

fn has_stream(cluster: &Cluster, node: RustNodeId, addr: ConnectionAddr, id: StreamId) -> bool {
    let connection = cluster
        .rust_node(node)
        .state()
        .network
        .scheduler
        .connection_state(&addr)
        .expect("the connection should still exist");
    let in_yamux = connection
        .yamux_state()
        .is_some_and(|yamux| yamux.streams.contains_key(&id));
    connection.streams.contains_key(&id) || in_yamux
}

fn assert_peer_ready(cluster: &Cluster, node: RustNodeId, peer_id: PeerId) {
    let state = cluster.rust_node(node).state();
    let peer = state.peers.get(&peer_id).expect("the peer should be known");
    assert!(
        peer.status.as_ready().is_some(),
        "the peer should still be ready, but it is {:?}",
        peer.status
    );
}

/// A reset of an ordinary stream leaves the connection up and removes only
/// that stream on the side that received it.
#[tokio::test]
async fn peer_reset_closes_only_the_stream() -> anyhow::Result<()> {
    let mut cluster = ClusterBuilder::new()
        .ports_with_len(10)
        .idle_duration(Duration::from_millis(100))
        .total_duration(Duration::from_secs(30))
        .start()
        .await?;
    let [(listener, listener_addr), (dialer, dialer_addr)] = connected_pair(
        &mut cluster,
        RustNodeConfig::default(),
        RustNodeConfig::default(),
    )
    .await?;

    // The dialer opens a ping stream, which the listener accepts.
    cluster
        .rust_node_mut(dialer)
        .dispatch_action(mina_p2p::P2pNetworkYamuxAction::OpenStream {
            addr: dialer_addr,
            stream_id: EXTRA_STREAM,
            stream_kind: StreamKind::Ping(PingAlgorithm::Ping1_0_0),
        });
    try_run_cluster(&mut cluster, Duration::from_secs(1))
        .await
        .map_err(|event| anyhow::anyhow!("error event after opening: {event:?}"))?;
    let listener_stream = cluster
        .rust_node(listener)
        .state()
        .network
        .scheduler
        .connection_state(&listener_addr)
        .and_then(|connection| connection.streams.get(&EXTRA_STREAM))
        .expect("the listener should have accepted the stream");
    assert!(
        listener_stream.select.negotiated.is_some(),
        "the stream should have negotiated"
    );

    // Then resets it. A disconnection on either side fails the test.
    assert!(
        cluster.rust_node_mut(dialer).dispatch_action(
            mina_p2p::P2pNetworkYamuxAction::OutgoingData {
                addr: dialer_addr,
                stream_id: EXTRA_STREAM,
                data: Data::default(),
                flags: YamuxFlags::RST,
            }
        ),
        "the reset should be sent"
    );
    try_run_cluster(&mut cluster, Duration::from_secs(2))
        .await
        .map_err(|event| anyhow::anyhow!("error event after the reset: {event:?}"))?;

    assert!(
        !has_stream(&cluster, listener, listener_addr, EXTRA_STREAM),
        "the listener should have dropped the reset stream"
    );
    assert_peer_ready(&cluster, listener, cluster.peer_id(dialer));
    assert_peer_ready(&cluster, dialer, cluster.peer_id(listener));
    Ok(())
}

/// The dialer's reducer, which never sends the multistream tokens of
/// `SILENT_STREAM`: like a go-libp2p host that opens a stream and waits before
/// it negotiates.
fn silent_reducer(
    state: &mut State,
    action: &ActionWithMeta<Action>,
    dispatcher: &mut Dispatcher<Action, State>,
) {
    let meta = action.meta().clone();
    let time = meta.time();
    let result = match action.action() {
        Action::P2p(P2pAction::Network(P2pNetworkAction::Select(
            P2pNetworkSelectAction::OutgoingTokens {
                kind: SelectKind::Stream(_, SILENT_STREAM),
                ..
            },
        ))) => Ok(()),
        Action::P2p(action) => P2pState::reducer(
            Substate::new(state, dispatcher),
            meta.with_action(action.clone()),
        ),
        Action::Idle(_) => P2pState::p2p_timeout_dispatch(Substate::new(state, dispatcher), &meta),
        Action::P2pEffectful(_) => Ok(()),
    };
    if let Err(error) = result {
        mina_core::warn!(time; "error = {error}");
    }
}

/// A stream the peer opened and doesn't negotiate in time is reset on its own:
/// both sides keep the connection, and neither keeps the stream.
#[tokio::test]
async fn select_timeout_resets_only_the_stream() -> anyhow::Result<()> {
    let mut cluster = ClusterBuilder::new()
        .ports_with_len(10)
        .idle_duration(Duration::from_millis(100))
        .total_duration(Duration::from_secs(30))
        .start()
        .await?;
    let timeouts = P2pTimeouts {
        select_incoming_stream: Some(Duration::from_secs(1)),
        ..Default::default()
    };
    let [(listener, listener_addr), (dialer, dialer_addr)] = connected_pair(
        &mut cluster,
        RustNodeConfig::default().with_timeouts(timeouts),
        RustNodeConfig::default().with_override_reducer(silent_reducer),
    )
    .await?;

    // Open the stream, then send the SYN alone.
    let dialer_node = cluster.rust_node_mut(dialer);
    dialer_node.dispatch_action(mina_p2p::P2pNetworkYamuxAction::OpenStream {
        addr: dialer_addr,
        stream_id: SILENT_STREAM,
        stream_kind: StreamKind::Ping(PingAlgorithm::Ping1_0_0),
    });
    assert!(
        dialer_node.dispatch_action(mina_p2p::P2pNetworkYamuxAction::OutgoingData {
            addr: dialer_addr,
            stream_id: SILENT_STREAM,
            data: Data::default(),
            flags: YamuxFlags::empty(),
        }),
        "the SYN should be sent"
    );
    try_run_cluster(&mut cluster, Duration::from_millis(300))
        .await
        .map_err(|event| anyhow::anyhow!("error event after opening: {event:?}"))?;
    assert!(
        has_stream(&cluster, listener, listener_addr, SILENT_STREAM),
        "the listener should be waiting for the stream to negotiate"
    );

    // The listener's 1 s timeout resets the stream; nobody disconnects.
    try_run_cluster(&mut cluster, Duration::from_secs(3))
        .await
        .map_err(|event| anyhow::anyhow!("error event after the timeout: {event:?}"))?;
    assert!(
        !has_stream(&cluster, listener, listener_addr, SILENT_STREAM),
        "the listener should have reset the stream"
    );
    assert!(
        !has_stream(&cluster, dialer, dialer_addr, SILENT_STREAM),
        "the dialer should have dropped the stream the listener reset"
    );
    assert_peer_ready(&cluster, listener, cluster.peer_id(dialer));
    assert_peer_ready(&cluster, dialer, cluster.peer_id(listener));
    Ok(())
}

/// A reset of the stream our RPC channel uses still closes the connection.
#[tokio::test]
async fn reset_of_our_rpc_stream_is_fatal() -> anyhow::Result<()> {
    let mut cluster = ClusterBuilder::new()
        .ports_with_len(10)
        .idle_duration(Duration::from_millis(100))
        .total_duration(Duration::from_secs(30))
        .is_error(allow_disconnections)
        .start()
        .await?;
    let [(listener, listener_addr), (dialer, dialer_addr)] = connected_pair(
        &mut cluster,
        RustNodeConfig::default(),
        RustNodeConfig::default(),
    )
    .await?;
    let dialer_id = cluster.peer_id(dialer);

    // The listener's RPC stream has the server's id; the dialer resets it.
    let rpc_stream = YamuxStreamKind::Rpc.stream_id(listener_addr.incoming);
    assert!(has_stream(&cluster, listener, listener_addr, rpc_stream));
    assert!(
        cluster.rust_node_mut(dialer).dispatch_action(
            mina_p2p::P2pNetworkYamuxAction::OutgoingData {
                addr: dialer_addr,
                stream_id: rpc_stream,
                data: Data::default(),
                flags: YamuxFlags::RST,
            }
        ),
        "the reset should be sent"
    );

    // A connection error emits no cluster event: read the listener's state.
    try_run_cluster(&mut cluster, Duration::from_secs(2))
        .await
        .map_err(|event| anyhow::anyhow!("error event after the reset: {event:?}"))?;
    let state = cluster.rust_node(listener).state();
    assert!(
        state
            .network
            .scheduler
            .connection_state(&listener_addr)
            .is_none_or(|connection| connection.closed.is_some()),
        "the listener should have closed the connection"
    );
    let peer = state
        .peers
        .get(&dialer_id)
        .expect("the peer should be known");
    assert!(
        peer.status.as_ready().is_none(),
        "the peer should no longer be ready, but it is {:?}",
        peer.status
    );
    Ok(())
}

fn pubsub_stream_of(cluster: &Cluster, node: RustNodeId, peer_id: PeerId) -> Option<StreamId> {
    cluster
        .rust_node(node)
        .state()
        .network
        .scheduler
        .broadcast_state
        .clients
        .get(&peer_id)
        .and_then(|client| client.outgoing_stream_id)
}

/// The pubsub bytes the dialer received, with the stream they came on.
static PUBSUB_RECEIVED: Mutex<Vec<(StreamId, Vec<u8>)>> = Mutex::new(Vec::new());
/// Mina's one pubsub topic, which a node subscribes to in the first message it
/// writes on a new pubsub stream.
const PUBSUB_TOPIC: &[u8] = b"coda/consensus-messages/0.0.1";

/// The dialer's reducer, which records what reaches its pubsub layer and then
/// handles it as usual.
fn pubsub_recording_reducer(
    state: &mut State,
    action: &ActionWithMeta<Action>,
    dispatcher: &mut Dispatcher<Action, State>,
) {
    let meta = action.meta().clone();
    let time = meta.time();
    if let Action::P2p(P2pAction::Network(P2pNetworkAction::Pubsub(
        P2pNetworkPubsubAction::IncomingData {
            stream_id, data, ..
        },
    ))) = action.action()
    {
        PUBSUB_RECEIVED
            .lock()
            .unwrap()
            .push((*stream_id, data.0.to_vec()));
    }
    let result = match action.action() {
        Action::P2p(action) => P2pState::reducer(
            Substate::new(state, dispatcher),
            meta.with_action(action.clone()),
        ),
        Action::Idle(_) => P2pState::p2p_timeout_dispatch(Substate::new(state, dispatcher), &meta),
        Action::P2pEffectful(_) => Ok(()),
    };
    if let Err(error) = result {
        mina_core::warn!(time; "error = {error}");
    }
}

/// When the peer resets our pubsub stream, we open it again, as
/// go-libp2p-pubsub does, so that the peer keeps getting our gossip.
#[tokio::test]
async fn peer_reset_of_our_pubsub_stream_reopens_it() -> anyhow::Result<()> {
    let mut cluster = ClusterBuilder::new()
        .ports_with_len(10)
        .idle_duration(Duration::from_millis(100))
        .total_duration(Duration::from_secs(30))
        .start()
        .await?;
    let [(listener, listener_addr), (dialer, dialer_addr)] = connected_pair(
        &mut cluster,
        RustNodeConfig::default(),
        RustNodeConfig::default().with_override_reducer(pubsub_recording_reducer),
    )
    .await?;
    let dialer_id = cluster.peer_id(dialer);

    // The listener's pubsub stream has the server's id.
    let pubsub_stream = YamuxStreamKind::Gossipsub.stream_id(listener_addr.incoming);
    assert_eq!(
        pubsub_stream_of(&cluster, listener, dialer_id),
        Some(pubsub_stream),
        "the listener should have opened its pubsub stream"
    );

    // The dialer resets it, as a go-libp2p-pubsub peer does with a stream it
    // can't read.
    assert!(
        cluster.rust_node_mut(dialer).dispatch_action(
            mina_p2p::P2pNetworkYamuxAction::OutgoingData {
                addr: dialer_addr,
                stream_id: pubsub_stream,
                data: Data::default(),
                flags: YamuxFlags::RST,
            }
        ),
        "the reset should be sent"
    );
    // Count only what the dialer receives from here on.
    PUBSUB_RECEIVED.lock().unwrap().clear();
    try_run_cluster(&mut cluster, Duration::from_secs(2))
        .await
        .map_err(|event| anyhow::anyhow!("error event after the reset: {event:?}"))?;

    // The listener opened it again, negotiated it, and writes to it.
    let reopened = cluster
        .rust_node(listener)
        .state()
        .network
        .scheduler
        .connection_state(&listener_addr)
        .expect("the connection should still exist")
        .streams
        .get(&pubsub_stream)
        .expect("the listener should have opened the stream again");
    assert!(
        reopened.select.negotiated.is_some(),
        "the reopened stream should have negotiated"
    );
    assert_eq!(
        pubsub_stream_of(&cluster, listener, dialer_id),
        Some(pubsub_stream),
        "the listener should write its gossip to the reopened stream"
    );
    // And the dialer reads it: the listener's first message on the new stream
    // subscribes it to the topic again.
    let received: Vec<u8> = PUBSUB_RECEIVED
        .lock()
        .unwrap()
        .iter()
        .filter(|(stream_id, _)| *stream_id == pubsub_stream)
        .flat_map(|(_, data)| data.clone())
        .collect();
    assert!(
        received
            .windows(PUBSUB_TOPIC.len())
            .any(|window| window == PUBSUB_TOPIC),
        "the dialer should have read the listener's subscription on the reopened stream, \
         got {} bytes",
        received.len()
    );
    assert_peer_ready(&cluster, listener, dialer_id);
    assert_peer_ready(&cluster, dialer, cluster.peer_id(listener));
    Ok(())
}

/// A stream the dialler writes more to than the listener's yamux window holds.
const BULK_STREAM: StreamId = 105;
/// What the listener received on `BULK_STREAM`, after negotiation.
static BULK_RECEIVED: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// The listener's reducer, which records the payload of `BULK_STREAM`.
fn recording_reducer(
    state: &mut State,
    action: &ActionWithMeta<Action>,
    dispatcher: &mut Dispatcher<Action, State>,
) {
    let meta = action.meta().clone();
    let time = meta.time();
    if let Action::P2p(P2pAction::Network(P2pNetworkAction::Select(
        P2pNetworkSelectAction::IncomingPayload {
            stream_id: BULK_STREAM,
            data,
            ..
        },
    ))) = action.action()
    {
        // Record it instead of handing it on: the node has no ping handler
        // and logs an error for every payload, which fails the cluster.
        BULK_RECEIVED.lock().unwrap().extend_from_slice(&data.0);
        return;
    }
    let result = match action.action() {
        Action::P2p(action) => P2pState::reducer(
            Substate::new(state, dispatcher),
            meta.with_action(action.clone()),
        ),
        Action::Idle(_) => P2pState::p2p_timeout_dispatch(Substate::new(state, dispatcher), &meta),
        Action::P2pEffectful(_) => Ok(()),
    };
    if let Err(error) = result {
        mina_core::warn!(time; "error = {error}");
    }
}

/// Bytes written faster than the peer's yamux window takes them wait and
/// arrive in order. (They used to be reordered once the window was full, and
/// a go-libp2p peer can only reset a stream whose bytes make no sense.)
#[tokio::test]
async fn writes_beyond_the_window_arrive_in_order() -> anyhow::Result<()> {
    let mut cluster = ClusterBuilder::new()
        .ports_with_len(10)
        .idle_duration(Duration::from_millis(100))
        .total_duration(Duration::from_secs(30))
        .start()
        .await?;
    let [(listener, listener_addr), (dialer, dialer_addr)] = connected_pair(
        &mut cluster,
        RustNodeConfig::default().with_override_reducer(recording_reducer),
        RustNodeConfig::default(),
    )
    .await?;

    cluster
        .rust_node_mut(dialer)
        .dispatch_action(mina_p2p::P2pNetworkYamuxAction::OpenStream {
            addr: dialer_addr,
            stream_id: BULK_STREAM,
            stream_kind: StreamKind::Ping(PingAlgorithm::Ping1_0_0),
        });
    try_run_cluster(&mut cluster, Duration::from_secs(1))
        .await
        .map_err(|event| anyhow::anyhow!("error event after opening: {event:?}"))?;
    assert!(
        has_stream(&cluster, listener, listener_addr, BULK_STREAM),
        "the listener should have accepted the stream"
    );

    // 600 KiB in 4 KiB chunks, all written before the listener's first window
    // update can arrive: its window is 256 KiB.
    let expected: Vec<u8> = (0u32..150)
        .flat_map(|chunk| std::iter::repeat_n(chunk.to_le_bytes(), 1024).flatten())
        .collect();
    for chunk in expected.chunks(4096) {
        assert!(
            cluster.rust_node_mut(dialer).dispatch_action(
                mina_p2p::P2pNetworkYamuxAction::OutgoingData {
                    addr: dialer_addr,
                    stream_id: BULK_STREAM,
                    data: chunk.to_vec().into(),
                    flags: YamuxFlags::empty(),
                }
            ),
            "the write should be accepted"
        );
    }

    for _ in 0..20 {
        if BULK_RECEIVED.lock().unwrap().len() >= expected.len() {
            break;
        }
        try_run_cluster(&mut cluster, Duration::from_millis(500))
            .await
            .map_err(|event| anyhow::anyhow!("error event while sending: {event:?}"))?;
    }
    let received = BULK_RECEIVED.lock().unwrap().clone();
    assert_eq!(received.len(), expected.len(), "every byte should arrive");
    let first_difference = received.iter().zip(&expected).position(|(a, b)| a != b);
    assert_eq!(
        first_difference, None,
        "the bytes should arrive in the order they were written"
    );
    assert_peer_ready(&cluster, listener, cluster.peer_id(dialer));
    Ok(())
}

/// Rejected incoming streams must be forgotten in both maps. FIN alone left
/// an entry in yamux for every refusal, even after the scheduler forgot it.
#[tokio::test]
async fn stream_limit_refusals_do_not_accumulate_mux_state() -> anyhow::Result<()> {
    let mut cluster = ClusterBuilder::new()
        .ports_with_len(10)
        .total_duration(Duration::from_secs(30))
        .start()
        .await?;
    let [(listener, addr), (dialer, _)] = connected_pair(
        &mut cluster,
        RustNodeConfig::default(),
        RustNodeConfig::default(),
    )
    .await?;
    let peer = cluster.peer_id(dialer);
    let counts = |cluster: &Cluster| {
        let connection = cluster
            .rust_node(listener)
            .state()
            .network
            .scheduler
            .connection_state(&addr)
            .unwrap();
        (
            connection
                .streams
                .values()
                .filter(|s| s.select.is_incoming())
                .count(),
            connection.yamux_state().unwrap().streams.len(),
        )
    };
    let (initial_incoming, initial_mux) = counts(&cluster);
    for n in 0u32..150 {
        let id = 101 + n * 2;
        // Yamux WindowUpdate/SYN, zero delta, an unused client stream ID.
        let mut syn = vec![0, 1, 0, 1];
        syn.extend_from_slice(&id.to_be_bytes());
        syn.extend_from_slice(&[0; 4]);
        assert!(cluster.rust_node_mut(listener).dispatch_action(
            mina_p2p::P2pNetworkYamuxAction::IncomingData {
                addr,
                data: syn.into()
            }
        ));
        let (incoming, mux) = counts(&cluster);
        assert_eq!(incoming, (initial_incoming + n as usize + 1).min(128));
        assert_eq!(
            mux,
            initial_mux + incoming - initial_incoming,
            "rejected streams must not leave mux state behind"
        );
    }
    assert_peer_ready(&cluster, listener, peer);
    Ok(())
}
