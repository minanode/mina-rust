//! Publishes given gossip messages to the peers that connect to it, and to
//! no one else (P6 Level 2).
//!
//! It dials nobody and has no default peers or chain id: start an isolated
//! OCaml daemon with `--peer` set to this probe's address, and the probe
//! publishes each `--message` file (pubsub data as our node sends it: an
//! 8-byte little-endian length, then the bin_prot `GossipNetMessageV2`) on
//! `coda/consensus-messages/0.0.1` once that peer subscribes.
//!
//! Adapted from `tools/gossipsub-sandbox`, without its built-in seed list.

use std::{path::PathBuf, time::Duration};

use libp2p::{futures::StreamExt, gossipsub, swarm::SwarmEvent, Multiaddr};
use structopt::StructOpt;

#[derive(StructOpt)]
struct Args {
    /// Chain id (hex) of the network the peer runs: the libp2p pnet key.
    #[structopt(long)]
    chain_id: String,
    /// Address to listen on, e.g. /ip4/127.0.0.1/tcp/8662.
    #[structopt(long)]
    listen: Multiaddr,
    /// Hex-encoded ed25519 secret key (32 bytes) for a fixed peer id; a new
    /// one is generated when absent. This is a throwaway test identity.
    #[structopt(long)]
    identity: Option<String>,
    /// Peers to dial, and the only ones: for example the isolated daemon's
    /// own address, when it fails to dial in.
    #[structopt(long)]
    dial: Vec<Multiaddr>,
    /// Files whose bytes are published, one message each, in order.
    #[structopt(long)]
    message: Vec<PathBuf>,
    /// Seconds between messages.
    #[structopt(long, default_value = "5")]
    interval: u64,
    /// Seconds to stay connected after the last message.
    #[structopt(long, default_value = "30")]
    linger: u64,
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let args = Args::from_args();

    let messages: Vec<Vec<u8>> = args
        .message
        .iter()
        .map(|path| std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .collect();

    let secret = match &args.identity {
        Some(hex) => {
            let mut bytes = [0u8; 32];
            for (i, byte) in bytes.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex identity");
            }
            mina_transport::ed25519::SecretKey::try_from_bytes(bytes).expect("ed25519 key")
        }
        None => mina_transport::ed25519::SecretKey::generate(),
    };
    let local_key: libp2p::identity::Keypair =
        mina_transport::ed25519::Keypair::from(secret).into();
    let local_peer_id = local_key.public().to_peer_id();
    println!("probe peer id {local_peer_id}");
    println!("probe address {}/p2p/{local_peer_id}", args.listen);

    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .max_transmit_size(1024 * 1024 * 32)
        .build()
        .expect("valid gossipsub config");
    let behaviour: gossipsub::Behaviour = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        gossipsub_config,
    )
    .expect("valid gossipsub behaviour");

    let mut swarm = mina_transport::swarm(
        local_key,
        // The pnet key is Blake2b-256 of the rendezvous string, as in
        // `codanet.go` and `mina_core::preshared_key`.
        format!("/coda/0.0.1/{}", args.chain_id).as_bytes(),
        [args.listen.clone()],
        args.dial.iter().cloned(),
        behaviour,
    );
    let topic = gossipsub::IdentTopic::new("coda/consensus-messages/0.0.1");
    swarm.behaviour_mut().subscribe(&topic).unwrap();
    for address in &args.dial {
        for protocol in address.iter() {
            if let libp2p::multiaddr::Protocol::P2p(peer_id) = protocol {
                swarm.behaviour_mut().add_explicit_peer(&peer_id);
            }
        }
    }

    let mut subscribed = false;
    let mut next = 0;
    let mut tick = tokio::time::interval(Duration::from_secs(args.interval));
    let mut done_at: Option<tokio::time::Instant> = None;

    loop {
        tokio::select! {
            event = swarm.select_next_some() => match event {
                SwarmEvent::Behaviour(gossipsub::Event::Subscribed { peer_id, topic }) => {
                    println!("subscribed {peer_id} {topic}");
                    subscribed = true;
                }
                SwarmEvent::Behaviour(gossipsub::Event::Message { message, .. }) => {
                    println!("received {} bytes from {:?}", message.data.len(), message.source);
                }
                SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                    println!("connected {peer_id} {}", endpoint.get_remote_address());
                }
                SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                    println!("disconnected {peer_id} {cause:?}");
                }
                SwarmEvent::NewListenAddr { address, .. } => {
                    println!("listening {address}");
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    println!("dial {peer_id:?} failed: {error}");
                }
                SwarmEvent::IncomingConnectionError { error, .. } => {
                    println!("incoming connection failed: {error}");
                }
                _ => {}
            },
            _ = tick.tick() => {
                if let Some(at) = done_at {
                    if tokio::time::Instant::now() >= at {
                        println!("done");
                        return;
                    }
                } else if subscribed && next < messages.len() {
                    match swarm.behaviour_mut().publish(topic.clone(), messages[next].clone()) {
                        Ok(id) => {
                            println!("published {} ({} bytes) as {id}", args.message[next].display(), messages[next].len());
                            next += 1;
                            if next == messages.len() {
                                done_at = Some(tokio::time::Instant::now() + Duration::from_secs(args.linger));
                            }
                        }
                        Err(e) => println!("publish {} failed: {e:?}", args.message[next].display()),
                    }
                }
            }
        }
    }
}
