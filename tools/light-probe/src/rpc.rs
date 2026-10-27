//! A Mina RPC client over one libp2p substream.
//!
//! The substream handler and framing are the ones in `tools/rpc-capture`
//! (adapted there from MinaProtocol/mina-verify, Apache-2.0). This version keeps
//! the stream open and sends several typed queries on it, counting the bytes it
//! reads and writes.

use std::{
    collections::VecDeque,
    future::Future,
    task::{Context, Poll},
};

use binprot::{BinProtRead, BinProtWrite};
use libp2p::{
    core::{upgrade::ReadyUpgrade, Endpoint},
    futures::{AsyncReadExt, AsyncWriteExt, StreamExt},
    swarm::{
        handler::{ConnectionEvent, FullyNegotiatedOutbound},
        ConnectionDenied, ConnectionHandler, ConnectionHandlerEvent, ConnectionId, FromSwarm,
        KeepAlive, NetworkBehaviour, PollParameters, Stream, StreamProtocol, SubstreamProtocol,
        Swarm, THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
    },
    Multiaddr, PeerId,
};
use mina_p2p_messages::{
    rpc_kernel::{MessageHeader, QueryHeader, ResponseHeader, ResponsePayload, RpcMethod},
    string::CharString,
};
use void::Void;

const RPC_PROTOCOL: StreamProtocol = StreamProtocol::new("coda/rpcs/0.0.1");
/// The id of the handshake both sides send when an RPC stream opens.
const HANDSHAKE_ID: u64 = u64::from_le_bytes(*b"RPC\x00\x00\x00\x00\x00");

/// Opens one outbound RPC substream per connection and hands it up.
#[derive(Default)]
pub struct RpcHandler {
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
pub struct RpcBehaviour {
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

/// Run `fut` while polling the swarm, so the connection keeps working.
pub async fn drive<B: NetworkBehaviour, F: Future>(swarm: &mut Swarm<B>, fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    loop {
        tokio::select! {
            _ = swarm.next() => {}
            r = &mut fut => return r,
        }
    }
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

/// One RPC stream to a peer, with byte counters.
pub struct RpcConn {
    pub peer: PeerId,
    stream: Stream,
    next_id: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub calls: u64,
}

impl RpcConn {
    pub async fn open(peer: PeerId, mut stream: Stream) -> Result<Self, String> {
        let handshake = frame(
            &MessageHeader::Response(ResponseHeader { id: HANDSHAKE_ID }),
            b"\x01",
        );
        stream
            .write_all(&handshake)
            .await
            .map_err(|e| e.to_string())?;
        Ok(Self {
            peer,
            stream,
            next_id: 1,
            bytes_in: 0,
            bytes_out: handshake.len() as u64,
            calls: 0,
        })
    }

    /// Send one query and decode its response.
    pub async fn call<M: RpcMethod>(&mut self, query: &M::Query) -> Result<M::Response, String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut body = Vec::new();
        query.binprot_write(&mut body).expect("write query");
        let mut payload = Vec::new();
        binprot::Nat0(body.len() as u64)
            .binprot_write(&mut payload)
            .expect("write length");
        payload.extend_from_slice(&body);
        let message = frame(
            &MessageHeader::Query(QueryHeader {
                tag: CharString::from(M::NAME_STR),
                version: M::VERSION,
                id,
            }),
            &payload,
        );
        let io = |e: std::io::Error| e.to_string();
        self.stream.write_all(&message).await.map_err(io)?;
        self.stream.flush().await.map_err(io)?;
        self.bytes_out += message.len() as u64;
        self.calls += 1;

        loop {
            let mut len = [0u8; 8];
            self.stream.read_exact(&mut len).await.map_err(io)?;
            let mut message = vec![0u8; u64::from_le_bytes(len) as usize];
            self.stream.read_exact(&mut message).await.map_err(io)?;
            self.bytes_in += 8 + message.len() as u64;
            let mut cursor = &message[..];
            match MessageHeader::binprot_read(&mut cursor).map_err(|e| e.to_string())? {
                MessageHeader::Response(ResponseHeader { id: got }) if got == id => {
                    let payload = ResponsePayload::<M::Response>::binprot_read(&mut cursor)
                        .map_err(|e| format!("decoding {} response: {e}", M::NAME_STR))?;
                    return match payload.0 {
                        Ok(v) => Ok(v.0),
                        Err(e) => Err(format!("{} answered an error: {e:?}", M::NAME_STR)),
                    };
                }
                // The peer's handshake, heartbeats, or its own queries.
                other => log::debug!("skipping {other:?}"),
            }
        }
    }
}
