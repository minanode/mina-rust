//! Send one Mina RPC to a live peer and save the raw response.
//!
//! Captures real responses from OCaml nodes as test fixtures, so wire types can be
//! ported against what the network actually sends:
//!
//!   mina-rpc-capture --rpc get_best_tip:3 --out best_tip_v3.bin
//!
//! The saved file is the response payload exactly as received after the RPC
//! message header: `RpcResult<NeedsLength<Response>, Error>` in bin_prot.
//!
//! The RPC substream handler and framing are adapted from MinaProtocol/mina-verify
//! (`crates/mina-verify-capture`, Apache-2.0).

use std::{
    collections::VecDeque,
    path::PathBuf,
    task::{Context, Poll},
    time::Duration,
};

use binprot::{BinProtRead, BinProtWrite};
use clap::Parser;
use libp2p::{
    core::{upgrade::ReadyUpgrade, Endpoint},
    futures::{AsyncReadExt, AsyncWriteExt, StreamExt},
    identity::Keypair,
    swarm::{
        handler::{ConnectionEvent, FullyNegotiatedOutbound},
        ConnectionDenied, ConnectionHandler, ConnectionHandlerEvent, ConnectionId, FromSwarm,
        KeepAlive, NetworkBehaviour, PollParameters, Stream, StreamProtocol, SubstreamProtocol,
        SwarmEvent, THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
    },
    Multiaddr, PeerId,
};
use mina_p2p_messages::{
    rpc_kernel::{MessageHeader, QueryHeader, ResponseHeader},
    string::CharString,
};
use void::Void;

const RPC_PROTOCOL: StreamProtocol = StreamProtocol::new("coda/rpcs/0.0.1");
/// The id of the handshake both sides send when an RPC stream opens.
const HANDSHAKE_ID: u64 = u64::from_le_bytes(*b"RPC\x00\x00\x00\x00\x00");
const QUERY_ID: u64 = 1;

const MAINNET_CHAIN_ID: &str = "0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886";
const MAINNET_SEEDS: &[&str] = &[
    "/dns4/seed-1.mainnet.gcp.o1test.net/tcp/10003/p2p/12D3KooWCa1d7G3SkRxy846qTvdAFX69NnoYZ32orWVLqJcDVGHW",
    "/dns4/seed-4.mainnet.gcp.o1test.net/tcp/10003/p2p/12D3KooWEdBiTUQqxp3jeuWaZkwiSNcFxC6d6Tdq7u2Lf2ZD2Q6X",
    "/dns4/mina-mainnet-seed.staketab.com/tcp/10003/p2p/12D3KooWSDTiXcdBVpN12ZqXJ49qCFp8zB1NnovuhZu6A28GLF1J",
    "/dns4/seed.minaexplorer.com/tcp/8302/p2p/12D3KooWR7coZtrMHvsgsfiWq2GESYypac3i29LFGp6EpbtjxBiJ",
];

#[derive(Parser)]
#[command(about = "Send one Mina RPC to a live peer and save the raw response")]
struct Args {
    /// RPC as `name:version`, e.g. `get_best_tip:3`.
    #[arg(long)]
    rpc: String,
    /// Query as hex-encoded bin_prot (without the length prefix). Defaults to
    /// `()`, the query of `get_best_tip`.
    #[arg(long, default_value = "00")]
    query_hex: String,
    /// Where to write the raw response payload.
    #[arg(long)]
    out: PathBuf,
    /// Chain id; defaults to Mesa mainnet.
    #[arg(long, default_value = MAINNET_CHAIN_ID)]
    chain_id: String,
    /// Peers to dial; defaults to mainnet seeds.
    #[arg(long)]
    peer: Vec<String>,
    /// Give up after this many seconds.
    #[arg(long, default_value_t = 120)]
    timeout: u64,
}

/// Opens one outbound RPC substream per connection and hands it up.
#[derive(Default)]
struct RpcHandler {
    requested: bool,
    ready: VecDeque<Stream>,
}

impl ConnectionHandler for RpcHandler {
    type FromBehaviour = Void;
    type ToBehaviour = Stream;
    type Error = Void;
    type InboundProtocol = ReadyUpgrade<StreamProtocol>;
    type OutboundProtocol = ReadyUpgrade<StreamProtocol>;
    type InboundOpenInfo = ();
    type OutboundOpenInfo = ();

    fn listen_protocol(&self) -> SubstreamProtocol<ReadyUpgrade<StreamProtocol>, ()> {
        SubstreamProtocol::new(ReadyUpgrade::new(RPC_PROTOCOL), ())
    }

    fn connection_keep_alive(&self) -> KeepAlive {
        KeepAlive::Yes
    }

    fn on_behaviour_event(&mut self, _: Void) {}

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<ConnectionHandlerEvent<ReadyUpgrade<StreamProtocol>, (), Stream, Void>> {
        if !self.requested {
            self.requested = true;
            return Poll::Ready(ConnectionHandlerEvent::OutboundSubstreamRequest {
                protocol: SubstreamProtocol::new(ReadyUpgrade::new(RPC_PROTOCOL), ()),
            });
        }
        match self.ready.pop_front() {
            Some(stream) => Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(stream)),
            None => Poll::Pending,
        }
    }

    fn on_connection_event(
        &mut self,
        event: ConnectionEvent<
            Self::InboundProtocol,
            Self::OutboundProtocol,
            Self::InboundOpenInfo,
            Self::OutboundOpenInfo,
        >,
    ) {
        if let ConnectionEvent::FullyNegotiatedOutbound(FullyNegotiatedOutbound {
            protocol: stream,
            ..
        }) = event
        {
            self.ready.push_back(stream);
        }
    }
}

/// Surfaces each opened RPC stream, with its peer, as a swarm event.
#[derive(Default)]
struct RpcBehaviour {
    streams: VecDeque<(PeerId, Stream)>,
}

impl NetworkBehaviour for RpcBehaviour {
    type ConnectionHandler = RpcHandler;
    type ToSwarm = (PeerId, Stream);

    fn handle_established_inbound_connection(
        &mut self,
        _: ConnectionId,
        _: PeerId,
        _: &Multiaddr,
        _: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        Ok(RpcHandler::default())
    }

    fn handle_established_outbound_connection(
        &mut self,
        _: ConnectionId,
        _: PeerId,
        _: &Multiaddr,
        _: Endpoint,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        Ok(RpcHandler::default())
    }

    fn on_connection_handler_event(
        &mut self,
        peer: PeerId,
        _: ConnectionId,
        stream: THandlerOutEvent<Self>,
    ) {
        self.streams.push_back((peer, stream));
    }

    fn poll(
        &mut self,
        _: &mut Context<'_>,
        _: &mut impl PollParameters,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        match self.streams.pop_front() {
            Some(event) => Poll::Ready(ToSwarm::GenerateEvent(event)),
            None => Poll::Pending,
        }
    }

    fn on_swarm_event(&mut self, _: FromSwarm<Self::ConnectionHandler>) {}
}

/// `[8-byte little-endian length][MessageHeader][payload]`.
fn frame(header: &MessageHeader, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0u8; 8];
    header.binprot_write(&mut bytes).expect("write header");
    bytes.extend_from_slice(payload);
    let len = ((bytes.len() - 8) as u64).to_le_bytes();
    bytes[..8].copy_from_slice(&len);
    bytes
}

/// Send the handshake and one query, and return the raw payload of its response.
async fn call(
    stream: &mut Stream,
    name: &str,
    version: u32,
    query: &[u8],
) -> Result<Vec<u8>, String> {
    let handshake = frame(
        &MessageHeader::Response(ResponseHeader { id: HANDSHAKE_ID }),
        b"\x01",
    );
    let mut payload = Vec::new();
    binprot::Nat0(query.len() as u64)
        .binprot_write(&mut payload)
        .expect("write length");
    payload.extend_from_slice(query);
    let query = frame(
        &MessageHeader::Query(QueryHeader {
            tag: CharString::from(name),
            version,
            id: QUERY_ID,
        }),
        &payload,
    );
    let io = |e: std::io::Error| e.to_string();
    stream.write_all(&handshake).await.map_err(io)?;
    stream.write_all(&query).await.map_err(io)?;
    stream.flush().await.map_err(io)?;

    loop {
        let mut len = [0u8; 8];
        stream.read_exact(&mut len).await.map_err(io)?;
        let mut message = vec![0u8; u64::from_le_bytes(len) as usize];
        stream.read_exact(&mut message).await.map_err(io)?;
        let mut cursor = &message[..];
        match MessageHeader::binprot_read(&mut cursor).map_err(|e| e.to_string())? {
            MessageHeader::Response(ResponseHeader { id }) if id == QUERY_ID => {
                return Ok(cursor.to_vec());
            }
            // The peer's handshake, heartbeats, or its own queries.
            other => log::debug!("skipping {other:?}"),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    let (name, version) = args
        .rpc
        .split_once(':')
        .and_then(|(n, v)| Some((n.to_string(), v.parse::<u32>().ok()?)))
        .ok_or("--rpc must be name:version")?;
    let query = hex::decode(&args.query_hex).map_err(|e| format!("--query-hex: {e}"))?;
    let peers: Vec<Multiaddr> = if args.peer.is_empty() {
        MAINNET_SEEDS.iter().map(|s| s.parse().unwrap()).collect()
    } else {
        args.peer
            .iter()
            .map(|s| s.parse().map_err(|e| format!("peer {s}: {e}")))
            .collect::<Result<_, _>>()?
    };

    let pnet_input = format!("/coda/0.0.1/{}", args.chain_id);
    let mut swarm = mina_transport::swarm(
        Keypair::generate_ed25519(),
        pnet_input.as_bytes(),
        Vec::<Multiaddr>::new(),
        peers,
        RpcBehaviour::default(),
    );

    let run = async {
        loop {
            let (peer, mut stream) = loop {
                if let Some(SwarmEvent::Behaviour(opened)) = swarm.next().await {
                    break opened;
                }
            };
            log::info!("RPC stream open to {peer}, sending {name}:{version}");
            let mut rpc = std::pin::pin!(call(&mut stream, &name, version, &query));
            let result = loop {
                tokio::select! {
                    _ = swarm.next() => {}
                    r = &mut rpc => break r,
                }
            };
            match result {
                Ok(payload) => return Ok((peer, payload)),
                Err(e) => log::warn!("{peer}: {e}; trying the next peer"),
            }
        }
    };
    let (peer, payload) = tokio::select! {
        r = run => r,
        _ = tokio::time::sleep(Duration::from_secs(args.timeout)) => Err::<_, String>("timed out".into()),
    }?;

    std::fs::write(&args.out, &payload).map_err(|e| e.to_string())?;
    let status = match payload.first() {
        Some(0) => "ok".to_string(),
        Some(1) => format!("error: {}", String::from_utf8_lossy(&payload[1..]).trim()),
        _ => "empty".to_string(),
    };
    println!(
        "{name}:{version} from {peer}: {} bytes, {status} -> {}",
        payload.len(),
        args.out.display()
    );
    Ok(())
}
