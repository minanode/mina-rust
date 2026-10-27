//! Join a Mina network's gossip mesh and publish every block message to the relay.
//!
//! Adapted from o1-labs/mina-light-node (`crates/mina-relay/src/lib.rs`,
//! Apache-2.0). Kademlia on Mina's `/coda/kad/1.0.0` discovers peers beyond the
//! seeds, which matters: with the seeds alone, blocks arrive rarely.

use std::{collections::HashSet, sync::Arc, time::Duration};

use binprot::BinProtRead;
use libp2p::{
    futures::StreamExt,
    gossipsub,
    identity::Keypair,
    kad::{self, store::MemoryStore},
    multiaddr::Protocol,
    swarm::{NetworkBehaviour, SwarmEvent},
    Multiaddr, PeerId, StreamProtocol,
};
use mina_p2p_messages::v2::MinaBlockHeaderStableV2;

use crate::Relay;

/// Blocks, snark-pool diffs and transaction-pool diffs share this topic.
const CONSENSUS_TOPIC: &str = "coda/consensus-messages/0.0.1";
const KAD_PROTOCOL: StreamProtocol = StreamProtocol::new("/coda/kad/1.0.0");
/// Binprot tag of `GossipNetMessageV2::NewState`, after the 8-byte length prefix.
const NEW_STATE_TAG: u8 = 0;

#[derive(NetworkBehaviour)]
struct Behaviour {
    gossipsub: gossipsub::Behaviour,
    kad: kad::Behaviour<MemoryStore>,
}

fn peer_id_of(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().find_map(|p| match p {
        Protocol::P2p(id) => Some(id),
        _ => None,
    })
}

/// Block height from a gossip payload, for logging only; the relay forwards
/// payloads whether or not this decodes.
fn block_height(payload: &[u8]) -> Option<u32> {
    let header = MinaBlockHeaderStableV2::binprot_read(&mut payload.get(9..)?).ok()?;
    Some(
        header
            .protocol_state
            .body
            .consensus_state
            .blockchain_length
            .as_u32(),
    )
}

pub async fn run(chain_id: &str, peers: Vec<String>, relay: Arc<Relay>) {
    let peers: Vec<Multiaddr> = peers
        .iter()
        .map(|s| s.parse().unwrap_or_else(|e| panic!("peer {s:?}: {e}")))
        .collect();

    let local_key = Keypair::generate_ed25519();
    let local_peer_id = local_key.public().to_peer_id();
    log::info!("local peer id: {local_peer_id}");

    let gossipsub = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        gossipsub::ConfigBuilder::default()
            .max_transmit_size(32 * 1024 * 1024)
            .build()
            .expect("gossipsub config"),
    )
    .expect("gossipsub behaviour");

    let mut kad_config = kad::Config::default();
    kad_config.set_protocol_names(vec![KAD_PROTOCOL]);
    let mut kad =
        kad::Behaviour::with_config(local_peer_id, MemoryStore::new(local_peer_id), kad_config);
    for addr in &peers {
        if let Some(peer) = peer_id_of(addr) {
            kad.add_address(&peer, addr.clone());
        }
    }

    // The pnet pre-shared key is Blake2b-256 of "/coda/0.0.1/" ++ chain id.
    let pnet_input = format!("/coda/0.0.1/{chain_id}");
    let mut swarm = mina_transport::swarm(
        local_key,
        pnet_input.as_bytes(),
        [],
        peers.iter().cloned(),
        Behaviour { gossipsub, kad },
    );

    let topic = gossipsub::IdentTopic::new(CONSENSUS_TOPIC);
    swarm
        .behaviour_mut()
        .gossipsub
        .subscribe(&topic)
        .expect("subscribe to consensus topic");
    for addr in &peers {
        if let Some(peer) = peer_id_of(addr) {
            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer);
        }
    }
    let _ = swarm.behaviour_mut().kad.bootstrap();

    let mut connected = HashSet::new();
    let mut discover = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            _ = discover.tick() => {
                log::info!("peers connected: {}", connected.len());
                let _ = swarm.behaviour_mut().kad.get_closest_peers(PeerId::random());
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    message,
                    ..
                })) => {
                    if message.data.get(8) == Some(&NEW_STATE_TAG) && relay.publish(&message.data) {
                        match block_height(&message.data) {
                            Some(height) => log::info!("block {height} ({} bytes)", message.data.len()),
                            None => log::info!("block ({} bytes, header not decoded)", message.data.len()),
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Kad(kad::Event::RoutingUpdated {
                    peer, ..
                })) => {
                    if !connected.contains(&peer) {
                        let _ = swarm.dial(peer);
                    }
                }
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    connected.insert(peer_id);
                    relay.set_peers(connected.len());
                }
                SwarmEvent::ConnectionClosed { peer_id, num_established, .. } => {
                    if num_established == 0 {
                        connected.remove(&peer_id);
                        relay.set_peers(connected.len());
                    }
                    if connected.is_empty() {
                        for addr in &peers {
                            let _ = swarm.dial(addr.clone());
                        }
                    }
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    log::debug!("dial error to {peer_id:?}: {error}");
                }
                _ => {}
            },
        }
    }
}
