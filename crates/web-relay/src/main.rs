//! Relay Mina blocks from libp2p gossip to browsers over WebSocket.
//!
//! The relay joins a network's gossip mesh, keeps the most recent block messages,
//! and streams each raw payload to WebSocket clients, cached blocks first. It
//! verifies nothing: browsers check every block's proof themselves, so the relay is
//! trusted to forward blocks promptly but not for their validity.
//!
//! The gossip loop with Kademlia discovery is adapted from o1-labs/mina-light-node
//! (`crates/mina-relay`, Apache-2.0).

mod gossip;

use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use clap::Parser;
use serde::Serialize;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;

/// A network the relay can join: its chain id and bootstrap peers.
struct Network {
    name: &'static str,
    chain_id: &'static str,
    seeds: &'static [&'static str],
}

/// Mesa networks. Chain ids from `mina internal chain-id` (4.0.2) and, for
/// mainnet, `daemonStatus.chainId` of a live node; seeds from
/// `storage.googleapis.com/seed-lists/<network>_seeds.txt`.
const NETWORKS: &[Network] = &[
    Network {
        name: "mainnet",
        chain_id: "0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886",
        seeds: &[
            "/dns4/mina-mainnet-seed.staketab.com/tcp/10003/p2p/12D3KooWSDTiXcdBVpN12ZqXJ49qCFp8zB1NnovuhZu6A28GLF1J",
            "/dns4/mina-seed.bitcat365.com/tcp/10001/p2p/12D3KooWQzozNTDKL7MqUh6Nh11GMA4pQhRCAsNTRWxCAzAi4VbE",
            "/dns4/production-mainnet-libp2p.minaprotocol.network/tcp/10000/p2p/12D3KooWPywsM191KGGNVGiNqN35nyyJg4W2BhhYukF6hP9YBR8q",
            "/dns4/seed-1.mainnet.gcp.o1test.net/tcp/10003/p2p/12D3KooWCa1d7G3SkRxy846qTvdAFX69NnoYZ32orWVLqJcDVGHW",
            "/dns4/seed-2.mainnet.gcp.o1test.net/tcp/32002/p2p/12D3KooWK4NfthViCTyLgVQa1WvqDC1NccVxGruCXCZUt3GqvFvn",
            "/dns4/seed-3.mainnet.gcp.o1test.net/tcp/32003/p2p/12D3KooWNofeYVAJXA3WGg2qCDhs3GEe71kTmKpFQXRbZmCz1Vr7",
            "/dns4/seed-4.mainnet.gcp.o1test.net/tcp/10003/p2p/12D3KooWEdBiTUQqxp3jeuWaZkwiSNcFxC6d6Tdq7u2Lf2ZD2Q6X",
            "/dns4/seed-5.mainnet.gcp.o1test.net/tcp/32005/p2p/12D3KooWL1DJTigSwuKQRfQE3p7puFUqfbHjXbZJ9YBWtMNpr3GU",
            "/dns4/seed-6.mainnet.gcp.o1test.net/tcp/32006/p2p/12D3KooWHGx4u32n42ub7dJNxAcAhwiA1WDq1Zsjn3k7RsS11pE8",
            "/dns4/seed.minaexplorer.com/tcp/8302/p2p/12D3KooWR7coZtrMHvsgsfiWq2GESYypac3i29LFGp6EpbtjxBiJ",
            "/dns4/seed.minataur.net/tcp/8302/p2p/12D3KooWNyExDzG8T1BYXHpXQS66kaw3zi6qi5Pg9KD3GEyHW5FF",
            "/dns4/seed.piconbello.com/tcp/10001/p2p/12D3KooWRFac2AztcTeen2DYNwnTrmVBvwNDsRiFpDVdTkwdFAHP",
        ],
    },
    Network {
        name: "devnet",
        chain_id: "ebfce0d570bc22eb041e1a7b0a46bbc3ed5d0a1030ef2ab5e36eef908b93eba8",
        seeds: &[
            "/dns4/seed-1.devnet.gcp.o1test.net/tcp/10003/p2p/12D3KooWAdgYL6hv18M3iDBdaK1dRygPivSfAfBNDzie6YqydVbs",
            "/dns4/seed-2.devnet.gcp.o1test.net/tcp/32012/p2p/12D3KooWLjs54xHzVmMmGYb7W5RVibqbwD1co7M2ZMfPgPm7iAag",
            "/dns4/seed-3.devnet.gcp.o1test.net/tcp/32013/p2p/12D3KooWEiGVAFC7curXWXiGZyMWnZK9h8BKr88U8D5PKV3dXciv",
        ],
    },
];

#[derive(Parser)]
#[command(about = "Relay Mina blocks from libp2p gossip to browsers over WebSocket")]
struct Args {
    /// Network to join: mainnet or devnet.
    #[arg(long, default_value = "mainnet")]
    network: String,
    /// Address for the HTTP server (`/blocks` WebSocket, `/status`).
    #[arg(long, default_value = "127.0.0.1:8741")]
    listen: SocketAddr,
    /// Number of recent blocks sent to each client when it connects.
    #[arg(long, default_value_t = 16)]
    cache: usize,
    /// Extra peer multiaddrs to dial, in addition to the network's seeds.
    #[arg(long)]
    peer: Vec<String>,
}

/// State shared between the gossip loop and the HTTP server.
pub struct Relay {
    recent: Mutex<VecDeque<Arc<[u8]>>>,
    cache_size: usize,
    blocks: broadcast::Sender<Arc<[u8]>>,
    peers: AtomicUsize,
    clients: AtomicUsize,
    blocks_relayed: AtomicU64,
    last_block_unix_ms: AtomicU64,
}

impl Relay {
    fn new(cache_size: usize) -> Self {
        Self {
            recent: Mutex::new(VecDeque::with_capacity(cache_size)),
            cache_size,
            blocks: broadcast::channel(cache_size.max(16)).0,
            peers: AtomicUsize::new(0),
            clients: AtomicUsize::new(0),
            blocks_relayed: AtomicU64::new(0),
            last_block_unix_ms: AtomicU64::new(0),
        }
    }

    /// Cache a block payload and send it to connected clients, unless it is a
    /// repeat of a cached one.
    pub fn publish(&self, payload: &[u8]) -> bool {
        let payload: Arc<[u8]> = {
            let mut recent = self.recent.lock().unwrap();
            if recent.iter().any(|cached| **cached == *payload) {
                return false;
            }
            if recent.len() == self.cache_size {
                recent.pop_front();
            }
            let payload: Arc<[u8]> = payload.into();
            recent.push_back(payload.clone());
            payload
        };
        self.blocks_relayed.fetch_add(1, Ordering::Relaxed);
        self.last_block_unix_ms.store(unix_ms(), Ordering::Relaxed);
        let _ = self.blocks.send(payload);
        true
    }

    pub fn set_peers(&self, peers: usize) {
        self.peers.store(peers, Ordering::Relaxed);
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Serialize)]
struct Status {
    network: &'static str,
    chain_id: &'static str,
    peers: usize,
    clients: usize,
    cached_blocks: usize,
    blocks_relayed: u64,
    last_block_unix_ms: Option<u64>,
}

#[derive(Clone)]
struct AppState {
    relay: Arc<Relay>,
    network: &'static Network,
}

async fn status(State(app): State<AppState>) -> Json<Status> {
    let relay = &app.relay;
    let last = relay.last_block_unix_ms.load(Ordering::Relaxed);
    Json(Status {
        network: app.network.name,
        chain_id: app.network.chain_id,
        peers: relay.peers.load(Ordering::Relaxed),
        clients: relay.clients.load(Ordering::Relaxed),
        cached_blocks: relay.recent.lock().unwrap().len(),
        blocks_relayed: relay.blocks_relayed.load(Ordering::Relaxed),
        last_block_unix_ms: (last != 0).then_some(last),
    })
}

async fn blocks(ws: WebSocketUpgrade, State(app): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| stream_blocks(socket, app.relay))
}

/// Send the cached blocks, then each new one, as binary messages.
async fn stream_blocks(mut socket: WebSocket, relay: Arc<Relay>) {
    // Subscribe before reading the cache so no block falls between the two.
    let mut live = relay.blocks.subscribe();
    let cached: Vec<Arc<[u8]>> = relay.recent.lock().unwrap().iter().cloned().collect();
    relay.clients.fetch_add(1, Ordering::Relaxed);

    let mut sent = cached.clone();
    for payload in cached {
        if socket
            .send(Message::Binary(payload.to_vec().into()))
            .await
            .is_err()
        {
            relay.clients.fetch_sub(1, Ordering::Relaxed);
            return;
        }
    }
    loop {
        tokio::select! {
            block = live.recv() => match block {
                Ok(payload) => {
                    if sent.contains(&payload) {
                        continue;
                    }
                    sent.push(payload.clone());
                    if sent.len() > relay.cache_size {
                        sent.remove(0);
                    }
                    if socket.send(Message::Binary(payload.to_vec().into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    log::warn!("client lagged, skipped {missed} block(s)");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    relay.clients.fetch_sub(1, Ordering::Relaxed);
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let network: &'static Network = NETWORKS
        .iter()
        .find(|n| n.name == args.network)
        .unwrap_or_else(|| panic!("unknown network {:?}; use mainnet or devnet", args.network));
    let mut peers: Vec<String> = network.seeds.iter().map(|s| s.to_string()).collect();
    peers.extend(args.peer);

    let relay = Arc::new(Relay::new(args.cache.max(1)));
    tokio::spawn(gossip::run(network.chain_id, peers, relay.clone()));

    let app = Router::new()
        .route("/blocks", get(blocks))
        .route("/status", get(status))
        .layer(CorsLayer::permissive())
        .with_state(AppState { relay, network });

    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .unwrap_or_else(|e| panic!("binding {}: {e}", args.listen));
    log::info!(
        "{} relay: ws://{}/blocks, status at http://{}/status",
        network.name,
        args.listen,
        args.listen
    );
    axum::serve(listener, app).await.expect("http server");
}
