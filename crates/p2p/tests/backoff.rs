//! Dial backoff with real connections (I18): a peer id whose address answers
//! as another peer id is dialled once, and failed dials wait longer and longer.
//! Before I18 both were redialled every `outgoing_error_reconnect_timeout`.

use std::{
    net::{Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};

use mina_p2p::{identity::SecretKey, P2pTimeouts, PeerId};
use p2p_testing::{
    cluster::{ClusterBuilder, Listener},
    event::RustNodeEvent,
    futures::StreamExt,
    rust_node::RustNodeConfig,
    stream::ClusterStreamExt,
    test_node::TestNode,
    utils::wait_for_all_nodes_to_listen,
};

/// One second waits instead of 30, so that the test takes seconds.
fn short_timeouts() -> P2pTimeouts {
    P2pTimeouts {
        reconnect_timeout: Some(Duration::from_secs(1)),
        outgoing_error_reconnect_timeout: Some(Duration::from_secs(1)),
        ..Default::default()
    }
}

/// Runs the cluster for `time` and returns when the dialer's dials to
/// `peer_id` failed, after the start, with their errors.
async fn dial_errors(
    cluster: &mut p2p_testing::cluster::Cluster,
    dialer: p2p_testing::rust_node::RustNodeId,
    peer_id: PeerId,
    time: Duration,
) -> Vec<(Duration, String)> {
    let start = Instant::now();
    let mut errors = Vec::new();
    let mut stream = cluster.stream().take_during(time);
    while let Some(event) = stream.next().await {
        if let Some((
            id,
            RustNodeEvent::PeerConnectionError {
                peer_id: Some(failed),
                incoming: false,
                error,
            },
        )) = event.rust()
        {
            if *id == dialer && *failed == peer_id {
                errors.push((start.elapsed(), error.clone()));
            }
        }
    }
    errors
}

#[tokio::test]
async fn a_stale_peer_id_is_dialled_once() -> anyhow::Result<()> {
    let mut cluster = ClusterBuilder::default()
        .ports_with_len(10)
        .total_duration(Duration::from_secs(30))
        .start()
        .await?;
    let listener = cluster.add_rust_node(RustNodeConfig::default())?;
    let listening =
        wait_for_all_nodes_to_listen(&mut cluster, [listener], Duration::from_secs(2)).await;
    assert!(listening);

    // The listener's address, with a peer id that isn't the listener's.
    let addr = SocketAddr::from((
        Ipv4Addr::LOCALHOST,
        cluster.rust_node(listener).libp2p_port(),
    ));
    let stale = SecretKey::from_bytes([0xab; 32]).public_key().peer_id();
    let dialer = cluster.add_rust_node(
        RustNodeConfig::default()
            .with_timeouts(short_timeouts())
            .with_initial_peers([Listener::SocketPeerId(addr, stale)]),
    )?;

    let errors = dial_errors(&mut cluster, dialer, stale, Duration::from_secs(8)).await;
    assert_eq!(errors.len(), 1, "dialled the stale peer id: {errors:?}");
    assert!(
        errors[0].1.contains("doesn't match expected peer id"),
        "{errors:?}"
    );

    let listener_id = cluster.peer_id(listener);
    let state = cluster.rust_node(dialer).state();
    let record = state.dial_backoff.stale(&stale).expect("a stale record");
    assert_eq!(record.addr, addr);
    assert_eq!(record.answered_by, listener_id);
    assert!(state
        .peers
        .get(&stale)
        .is_none_or(|peer| peer.dial_opts.is_none()));
    Ok(())
}

#[tokio::test]
async fn failed_dials_wait_longer_and_longer() -> anyhow::Result<()> {
    let mut cluster = ClusterBuilder::default()
        .ports_with_len(10)
        .total_duration(Duration::from_secs(30))
        .start()
        .await?;
    // A port of the cluster's that nothing listens on: every dial is refused.
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, cluster.next_port()?));
    let peer_id = SecretKey::from_bytes([0xcd; 32]).public_key().peer_id();
    let dialer = cluster.add_rust_node(
        RustNodeConfig::default()
            .with_timeouts(short_timeouts())
            .with_initial_peers([Listener::SocketPeerId(addr, peer_id)]),
    )?;

    let errors = dial_errors(&mut cluster, dialer, peer_id, Duration::from_secs(11)).await;
    let times = errors
        .iter()
        .map(|(t, _)| t.as_secs_f64())
        .collect::<Vec<_>>();
    // Waits of about 1, 2 and 4 s (each 20% either way, and at least the 1 s
    // timeout): dials at about 0, 1, 3 and 7 s. A dial every second would
    // be 11.
    assert!(
        (3..=5).contains(&times.len()),
        "failed dials at {times:?} s"
    );
    let gaps = times.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>();
    assert!(
        gaps.windows(2).all(|g| g[1] > g[0]),
        "gaps between dials should grow: {gaps:?}"
    );

    let state = cluster.rust_node(dialer).state();
    let entry = state.dial_backoff.peer(&peer_id).expect("a backoff entry");
    assert_eq!(entry.failures as usize, times.len());
    assert_eq!(
        state.dial_backoff.addr(&addr).map(|e| e.failures),
        Some(entry.failures)
    );
    Ok(())
}

/// The only configured seed changes identity at the same socket. No discovery
/// peers exist to repair the dial options: the timer must recover on its own.
#[tokio::test]
async fn seed_only_timer_reconnects_when_expected_identity_returns() -> anyhow::Result<()> {
    use p2p_testing::{cluster::PeerIdConfig, libp2p_node::Libp2pNodeConfig};

    let mut cluster = ClusterBuilder::default()
        .ports_with_len(10)
        .total_duration(Duration::from_secs(30))
        .start()
        .await?;
    let wrong = cluster.add_libp2p_node(Libp2pNodeConfig::default())?;
    let expected = cluster.add_libp2p_node(Libp2pNodeConfig {
        peer_id: PeerIdConfig::Bytes([0xac; 32]),
        ..Default::default()
    })?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, cluster.next_port()?));
    let multiaddr = format!("/ip4/127.0.0.1/tcp/{}", addr.port()).parse()?;
    let listener = cluster
        .libp2p_node_mut(wrong)
        .swarm_mut()
        .listen_on(multiaddr)?;
    let peer_id = cluster.peer_id(expected);
    let timeouts = P2pTimeouts {
        stale_peer_id: Some(Duration::from_secs(8)),
        ..short_timeouts()
    };
    let dialer = cluster.add_rust_node(
        RustNodeConfig::default()
            .with_timeouts(timeouts)
            .with_initial_peers([Listener::SocketPeerId(addr, peer_id)]),
    )?;
    let errors = dial_errors(&mut cluster, dialer, peer_id, Duration::from_secs(3)).await;
    assert!(
        errors
            .iter()
            .any(|(_, e)| e.contains("doesn't match expected peer id")),
        "{errors:?}"
    );
    let state = cluster.rust_node(dialer).state();
    assert_eq!(state.ready_peers_iter().count(), 0);
    assert!(state.peers[&peer_id].dial_opts.is_none());
    let stale_time = state.dial_backoff.stale(&peer_id).unwrap().time;

    assert!(cluster
        .libp2p_node_mut(wrong)
        .swarm_mut()
        .remove_listener(listener));
    dial_errors(&mut cluster, dialer, peer_id, Duration::from_secs(1)).await;
    cluster
        .libp2p_node_mut(expected)
        .swarm_mut()
        .listen_on(format!("/ip4/127.0.0.1/tcp/{}", addr.port()).parse()?)?;
    let errors = dial_errors(&mut cluster, dialer, peer_id, Duration::from_secs(10)).await;
    assert!(errors.is_empty(), "{errors:?}");
    let state = cluster.rust_node(dialer).state();
    assert_eq!(state.ready_peers_iter().count(), 1);
    let ready = state
        .dial_backoff
        .peer(&peer_id)
        .unwrap()
        .ready_since
        .unwrap();
    assert!(ready.checked_sub(stale_time).unwrap() >= Duration::from_secs(8));
    assert_eq!(
        state.peers[&peer_id]
            .dial_opts
            .as_ref()
            .unwrap()
            .libp2p_socket_addr(),
        Some(addr)
    );
    Ok(())
}
