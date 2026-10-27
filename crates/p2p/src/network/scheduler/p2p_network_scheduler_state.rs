use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr},
    ops::{Deref, DerefMut},
};

use malloc_size_of_derive::MallocSizeOf;
use redux::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{disconnection::P2pDisconnectionReason, identity::PublicKey, PeerId};

use super::super::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StreamState<T>(pub BTreeMap<PeerId, BTreeMap<StreamId, T>>);

impl<T> Default for StreamState<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<T> Deref for StreamState<T> {
    type Target = BTreeMap<PeerId, BTreeMap<StreamId, T>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for StreamState<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Serialize, Deserialize, PartialEq, PartialOrd, Eq, Ord, Debug, Clone, Copy)]
pub struct ConnectionAddr {
    pub sock_addr: SocketAddr,
    pub incoming: bool,
}

impl std::fmt::Display for ConnectionAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} (incoming: {})", self.sock_addr, self.incoming)
    }
}

#[serde_with::serde_as]
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pNetworkSchedulerState {
    pub interfaces: BTreeSet<IpAddr>,
    pub listeners: BTreeSet<SocketAddr>,
    pub local_pk: PublicKey,
    #[serde_as(as = "serde_with::hex::Hex")]
    pub pnet_key: [u8; 32],
    pub connections: BTreeMap<ConnectionAddr, P2pNetworkConnectionState>,
    pub broadcast_state: P2pNetworkPubsubState,
    pub identify_state: identify::P2pNetworkIdentifyState,
    pub discovery_state: Option<P2pNetworkKadState>,
    pub rpc_incoming_streams: StreamState<P2pNetworkRpcState>,
    pub rpc_outgoing_streams: StreamState<P2pNetworkRpcState>,
}

impl P2pNetworkSchedulerState {
    /// Bytes actually retained behind remote Yamux windows, across all
    /// connections. Scan live streams so resets and disconnects release their
    /// share without a separate counter that can become stale.
    pub fn queued_outgoing_bytes(&self, peer_id: &PeerId) -> (usize, usize) {
        self.connections
            .values()
            .fold((0, 0), |(total, peer), connection| {
                let queued = match &connection.mux {
                    Some(P2pNetworkConnectionMuxState::Yamux(yamux)) => yamux
                        .streams
                        .values()
                        .map(|stream| stream.pending_len())
                        .sum::<usize>(),
                    _ => 0,
                };
                (
                    total.saturating_add(queued),
                    peer.saturating_add(if connection.peer_id() == Some(peer_id) {
                        queued
                    } else {
                        0
                    }),
                )
            })
    }

    /// Expected output for channel-dispatched RPC work. The stream is the
    /// owner, so resets and disconnects release this without counter updates.
    pub fn reserved_rpc_response_bytes(&self, peer_id: &PeerId) -> (usize, usize) {
        self.rpc_incoming_streams.iter().fold(
            (0usize, 0usize),
            |(total, peer), (owner, streams)| {
                let reserved = streams
                    .values()
                    .flat_map(|stream| stream.pending_incoming.values())
                    .map(|query| query.reserved_bytes)
                    .sum::<usize>();
                (
                    total.saturating_add(reserved),
                    peer.saturating_add(if owner == peer_id { reserved } else { 0 }),
                )
            },
        )
    }

    pub fn discovery_state(&self) -> Option<&P2pNetworkKadState> {
        self.discovery_state.as_ref()
    }

    pub fn find_peer(
        &self,
        peer_id: &PeerId,
    ) -> Option<(&ConnectionAddr, &P2pNetworkConnectionState)> {
        self.connections
            .iter()
            .find(|(_, conn_state)| conn_state.peer_id() == Some(peer_id))
    }

    pub fn prune_peer_state(&mut self, peer_id: &PeerId) {
        self.broadcast_state.prune_peer_state(peer_id);
        self.identify_state.prune_peer_state(peer_id);

        if let Some(discovery_state) = self.discovery_state.as_mut() {
            discovery_state.streams.remove(peer_id);
        }

        self.rpc_incoming_streams.remove(peer_id);
        self.rpc_outgoing_streams.remove(peer_id);
    }

    /// Forgets one stream of a peer in every protocol's state, after the
    /// stream was reset. `pubsub_incoming` drops the peer's partial pubsub
    /// message as well, which would otherwise be prefixed to the next
    /// stream's data.
    pub fn prune_stream_state(
        &mut self,
        peer_id: &PeerId,
        stream_id: StreamId,
        pubsub_incoming: bool,
    ) {
        self.broadcast_state
            .prune_stream_state(peer_id, stream_id, pubsub_incoming);
        self.identify_state
            .remove_identify_stream_state(peer_id, &stream_id);

        if let Some(discovery_state) = self.discovery_state.as_mut() {
            discovery_state.remove_kad_stream_state(peer_id, &stream_id);
        }

        for streams in [
            &mut self.rpc_incoming_streams,
            &mut self.rpc_outgoing_streams,
        ] {
            if let Some(streams) = streams.get_mut(peer_id) {
                streams.remove(&stream_id);
            }
        }
    }

    pub fn connection_state_mut(
        &mut self,
        addr: &ConnectionAddr,
    ) -> Option<&mut P2pNetworkConnectionState> {
        self.connections.get_mut(addr)
    }

    pub fn connection_state(&self, addr: &ConnectionAddr) -> Option<&P2pNetworkConnectionState> {
        self.connections.get(addr)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
pub struct P2pNetworkConnectionState {
    pub incoming: bool,
    pub pnet: P2pNetworkPnetState,
    pub select_auth: P2pNetworkSelectState,
    pub auth: Option<P2pNetworkAuthState>,
    pub select_mux: P2pNetworkSelectState,
    pub mux: Option<P2pNetworkConnectionMuxState>,
    #[with_malloc_size_of_func = "measurement::streams_map"]
    pub streams: BTreeMap<StreamId, P2pNetworkStreamState>,
    #[ignore_malloc_size_of = "error"]
    pub closed: Option<P2pNetworkConnectionCloseReason>,
    // the number of bytes that peer allowed to send us before yamux is negotiated
    pub limit: usize,
    /// Reopens of our pubsub stream after the peer reset it: how many since
    /// the window last expired, and when the last one was
    /// (`P2pNetworkConnectionState::may_reopen_pubsub`).
    #[serde(default)]
    #[ignore_malloc_size_of = "small"]
    pub pubsub_reopens: Option<(u8, Timestamp)>,
}

impl P2pNetworkConnectionState {
    pub const INITIAL_LIMIT: usize = 1024;
    /// At most this many reopens of our pubsub stream...
    pub const PUBSUB_MAX_REOPENS: u8 = 4;
    /// ...until this long has passed since the last one.
    pub const PUBSUB_REOPEN_WINDOW: std::time::Duration = std::time::Duration::from_secs(600);

    /// Whether to reopen our pubsub stream, which the peer reset while the
    /// connection stays up, and if so counts the reopen. Without the stream the
    /// peer gets none of our gossip. go-libp2p-pubsub, which OCaml nodes run,
    /// respawns its writer in this case, at most `MaxBackoffAttempts` (4) times
    /// until `TimeToLive` (10 minutes) passes since the last respawn
    /// (`handleDeadPeers` in `pubsub.go` and `backoff.go`, v0.9.3, the version
    /// OCaml 4.0.2's helper pins). We reopen at once, without its backoff delay.
    pub fn may_reopen_pubsub(&mut self, now: Timestamp) -> bool {
        let attempts = match self.pubsub_reopens {
            Some((attempts, last))
                if now
                    .checked_sub(last)
                    .is_some_and(|since| since <= Self::PUBSUB_REOPEN_WINDOW) =>
            {
                attempts
            }
            _ => 0,
        };
        if attempts >= Self::PUBSUB_MAX_REOPENS {
            return false;
        }
        self.pubsub_reopens = Some((attempts + 1, now));
        true
    }

    pub fn peer_id(&self) -> Option<&PeerId> {
        self.auth.as_ref().and_then(P2pNetworkAuthState::peer_id)
    }

    pub fn limit(&self) -> usize {
        if let Some(mux) = &self.mux {
            mux.limit()
        } else {
            self.limit
        }
    }

    pub fn consume(&mut self, len: usize) {
        if let Some(mux) = &mut self.mux {
            mux.consume(len);
        } else {
            self.limit = self.limit.saturating_sub(len);
        }
    }

    pub fn noise_state(&self) -> Option<&P2pNetworkNoiseState> {
        self.auth
            .as_ref()
            .map(|P2pNetworkAuthState::Noise(state)| state)
    }

    pub fn noise_state_mut(&mut self) -> Option<&mut P2pNetworkNoiseState> {
        self.auth
            .as_mut()
            .map(|P2pNetworkAuthState::Noise(state)| state)
    }

    pub fn yamux_state_mut(&mut self) -> Option<&mut P2pNetworkYamuxState> {
        self.mux
            .as_mut()
            .map(|P2pNetworkConnectionMuxState::Yamux(state)| state)
    }

    pub fn yamux_state(&self) -> Option<&P2pNetworkYamuxState> {
        self.mux
            .as_ref()
            .map(|P2pNetworkConnectionMuxState::Yamux(state)| state)
    }

    /// Whether the peer, rather than we, opened the stream. The yamux client,
    /// the side that dialled, opens odd-numbered streams and the server
    /// even-numbered ones.
    pub fn stream_opened_by_peer(addr: &ConnectionAddr, stream_id: StreamId) -> bool {
        (stream_id % 2 == 1) == addr.incoming
    }

    /// Whether a failure of this stream must close the connection: our RPC
    /// stream, which the RPC channel needs for every request we send, and our
    /// identify stream while it is open, without whose answer we never open
    /// the RPC stream. Both use fixed ids (`YamuxStreamKind::stream_id`).
    pub fn is_essential_stream(&self, addr: &ConnectionAddr, stream_id: StreamId) -> bool {
        let essential = [YamuxStreamKind::Rpc, YamuxStreamKind::Identify]
            .into_iter()
            .any(|kind| kind.stream_id(addr.incoming) == stream_id);
        essential && self.streams.contains_key(&stream_id)
    }

    /// The protocol negotiated on a stream, if any.
    pub fn stream_protocol(&self, stream_id: StreamId) -> Option<token::StreamKind> {
        match self.streams.get(&stream_id)?.select.negotiated {
            Some(Some(token::Protocol::Stream(kind))) => Some(kind),
            _ => None,
        }
    }

    /// Removes a stream from the connection and from its multiplexer.
    pub fn remove_stream(&mut self, stream_id: StreamId) {
        self.streams.remove(&stream_id);
        if let Some(yamux) = self.yamux_state_mut() {
            yamux.streams.remove(&stream_id);
        }
    }

    pub fn select_state_mut(&mut self, kind: &SelectKind) -> Option<&mut P2pNetworkSelectState> {
        match kind {
            SelectKind::Authentication => Some(&mut self.select_auth),
            SelectKind::MultiplexingNoPeerId | SelectKind::Multiplexing(_) => {
                Some(&mut self.select_mux)
            }
            SelectKind::Stream(_, stream_id) => Some(&mut self.streams.get_mut(stream_id)?.select),
        }
    }

    pub fn select_state(&self, kind: &SelectKind) -> Option<&P2pNetworkSelectState> {
        match kind {
            SelectKind::Authentication => Some(&self.select_auth),
            SelectKind::MultiplexingNoPeerId | SelectKind::Multiplexing(_) => {
                Some(&self.select_mux)
            }
            SelectKind::Stream(_, stream_id) => Some(&self.streams.get(stream_id)?.select),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, thiserror::Error)]
pub enum P2pNetworkConnectionCloseReason {
    #[error("peer is disconnected: {0}")]
    Disconnect(#[from] P2pDisconnectionReason),
    #[error("connection error: {0}")]
    Error(#[from] P2pNetworkConnectionError),
}

impl P2pNetworkConnectionCloseReason {
    /// Returns true if the reason for disconnection is the statemachine
    /// behaviour, not external error.
    pub fn is_disconnected(&self) -> bool {
        matches!(self, P2pNetworkConnectionCloseReason::Disconnect(_))
    }
}

/// P2p connection error.
#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum P2pNetworkConnectionError {
    #[error("mio error: {0}")]
    MioError(String),
    #[error("noise handshake error: {0}")]
    Noise(#[from] NoiseError),
    #[error("remote peer closed connection")]
    RemoteClosed,
    #[error("select protocol error")]
    SelectError,
    #[error(transparent)]
    IdentifyStreamError(#[from] P2pNetworkIdentifyStreamError),
    #[error(transparent)]
    KademliaIncomingStreamError(#[from] P2pNetworkKadIncomingStreamError),
    #[error(transparent)]
    KademliaOutgoingStreamError(#[from] P2pNetworkKadOutgoingStreamError),
    #[error("peer reset yamux stream {0}")]
    StreamReset(StreamId),
    #[error("pubsub error: {0}")]
    PubSubError(String),
    #[error("peer make us keep too much data at stream {0}")]
    YamuxOverflow(StreamId),
    #[error("peer should not decrease window size at stream {0}")]
    YamuxBadWindowUpdate(StreamId),
}

/// Why a single stream was closed (`P2pNetworkSchedulerAction::StreamReset`).
#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum P2pNetworkStreamResetReason {
    #[error("peer reset the stream")]
    RemoteReset,
    #[error("peer closed the RPC connection")]
    RemoteRpcClose,
    #[error("incoming RPC heartbeat timed out")]
    RpcTimeout,
    #[error("outgoing stream queue exceeded its memory budget or stalled")]
    OutgoingQueueLimit,
    #[error("incoming stream limit exceeded")]
    StreamLimit,
    #[error("protocol negotiation failed: {0}")]
    SelectError(String),
    #[error("Kademlia stream error: {0}")]
    Kademlia(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
pub enum P2pNetworkAuthState {
    Noise(P2pNetworkNoiseState),
}

impl P2pNetworkAuthState {
    fn peer_id(&self) -> Option<&PeerId> {
        match self {
            P2pNetworkAuthState::Noise(v) => v.peer_id(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
pub enum P2pNetworkConnectionMuxState {
    Yamux(P2pNetworkYamuxState),
}

impl P2pNetworkConnectionMuxState {
    pub fn consume(&mut self, len: usize) {
        match self {
            Self::Yamux(state) => state.consume(len),
        }
    }

    fn limit(&self) -> usize {
        match self {
            Self::Yamux(state) => state.limit(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pNetworkStreamState {
    pub select: P2pNetworkSelectState,
}

impl P2pNetworkStreamState {
    pub fn new(stream_kind: token::StreamKind, time: Timestamp) -> Self {
        P2pNetworkStreamState {
            select: P2pNetworkSelectState::initiator_stream(stream_kind, time),
        }
    }

    pub fn new_incoming(time: Timestamp) -> Self {
        P2pNetworkStreamState {
            select: P2pNetworkSelectState::default_timed(time),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum P2pNetworkStreamHandlerState {
    Broadcast,
    Discovery,
}
mod measurement {
    use std::mem;

    use malloc_size_of::{MallocSizeOf, MallocSizeOfOps};

    use super::*;

    pub fn streams_map(
        val: &BTreeMap<StreamId, P2pNetworkStreamState>,
        ops: &mut MallocSizeOfOps,
    ) -> usize {
        val.iter()
            .map(|(k, v)| mem::size_of_val(k) + mem::size_of_val(v) + v.size_of(ops))
            .sum()
    }

    impl<T> MallocSizeOf for StreamState<T>
    where
        T: MallocSizeOf,
    {
        fn size_of(&self, ops: &mut MallocSizeOfOps) -> usize {
            self.0
                .iter()
                .map(|(k, v)| {
                    mem::size_of_val(k)
                        + mem::size_of_val(v)
                        + v.iter()
                            .map(|(k, v)| {
                                mem::size_of_val(k) + mem::size_of_val(v) + v.size_of(ops)
                            })
                            .sum::<usize>()
                })
                .sum()
        }
    }

    impl MallocSizeOf for ConnectionAddr {
        fn size_of(&self, _ops: &mut MallocSizeOfOps) -> usize {
            0
        }
    }

    impl MallocSizeOf for P2pNetworkSchedulerState {
        fn size_of(&self, ops: &mut MallocSizeOfOps) -> usize {
            self.interfaces.len() * mem::size_of::<IpAddr>()
                + self.listeners.len() * mem::size_of::<SocketAddr>()
                + self
                    .connections
                    .iter()
                    .map(|(k, v)| mem::size_of_val(k) + mem::size_of_val(v) + v.size_of(ops))
                    .sum::<usize>()
                + self.broadcast_state.size_of(ops)
                + self.identify_state.size_of(ops)
                + self.discovery_state.size_of(ops)
                + self.rpc_incoming_streams.size_of(ops)
                + self.rpc_outgoing_streams.size_of(ops)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::P2pTimeouts;

    fn addr(incoming: bool) -> ConnectionAddr {
        ConnectionAddr {
            sock_addr: "127.0.0.1:8302".parse().unwrap(),
            incoming,
        }
    }

    fn connection(incoming: bool, streams: &[StreamId]) -> P2pNetworkConnectionState {
        let time = Timestamp::ZERO;
        let mut yamux = P2pNetworkYamuxState::default();
        let mut stream_states = BTreeMap::new();
        for id in streams {
            yamux.streams.insert(*id, Default::default());
            stream_states.insert(*id, P2pNetworkStreamState::new_incoming(time));
        }
        P2pNetworkConnectionState {
            incoming,
            pnet: P2pNetworkPnetState::new([0; 32], time),
            select_auth: Default::default(),
            auth: None,
            select_mux: Default::default(),
            mux: Some(P2pNetworkConnectionMuxState::Yamux(yamux)),
            streams: stream_states,
            closed: None,
            limit: P2pNetworkConnectionState::INITIAL_LIMIT,
            pubsub_reopens: None,
        }
    }

    #[test]
    fn who_opened_a_stream() {
        // We dialled: we are the yamux client, with odd ids.
        assert!(!P2pNetworkConnectionState::stream_opened_by_peer(
            &addr(false),
            1
        ));
        assert!(P2pNetworkConnectionState::stream_opened_by_peer(
            &addr(false),
            4
        ));
        // The peer dialled: we are the server, with even ids.
        assert!(P2pNetworkConnectionState::stream_opened_by_peer(
            &addr(true),
            1
        ));
        assert!(!P2pNetworkConnectionState::stream_opened_by_peer(
            &addr(true),
            2
        ));
    }

    #[test]
    fn only_our_rpc_and_identify_streams_are_essential() {
        // Outgoing: our RPC 1, pubsub 3, Kademlia 5, identify 7; the peer's 2, 4.
        let ids = [1, 2, 3, 4, 5, 7];
        let conn = connection(false, &ids);
        let essential: Vec<_> = ids
            .into_iter()
            .filter(|id| conn.is_essential_stream(&addr(false), *id))
            .collect();
        assert_eq!(essential, [1, 7]);

        // Incoming: ours are even; the peer's odd ones never are.
        let ids = [1, 2, 7, 8];
        let conn = connection(true, &ids);
        let essential: Vec<_> = ids
            .into_iter()
            .filter(|id| conn.is_essential_stream(&addr(true), *id))
            .collect();
        assert_eq!(essential, [2, 8]);

        // A finished identify stream is gone, so a late reset of it is not
        // fatal.
        let conn = connection(false, &[1]);
        assert!(!conn.is_essential_stream(&addr(false), 7));
    }

    #[test]
    fn pubsub_reopens_are_bounded_like_go_libp2p_pubsub() {
        let mut conn = connection(false, &[1, 3]);
        let start = Timestamp::ZERO + Duration::from_secs(1);
        let at = |secs| start + Duration::from_secs(secs);

        // Four reopens in quick succession, then none.
        for secs in 0..4 {
            assert!(conn.may_reopen_pubsub(at(secs)), "reopen {secs}");
        }
        assert!(!conn.may_reopen_pubsub(at(4)));
        // A refusal doesn't extend the window: it ends 10 minutes after the
        // last reopen.
        assert!(!conn.may_reopen_pubsub(at(3 + 600)));
        assert!(conn.may_reopen_pubsub(at(3 + 601)));
        // And the count starts again.
        for secs in 602..605 {
            assert!(conn.may_reopen_pubsub(at(3 + secs)));
        }
        assert!(!conn.may_reopen_pubsub(at(3 + 605)));
    }

    #[test]
    fn remove_stream_forgets_it_in_yamux_too() {
        let mut conn = connection(false, &[1, 4]);
        conn.remove_stream(4);
        assert!(!conn.streams.contains_key(&4));
        assert!(!conn.yamux_state().unwrap().streams.contains_key(&4));
        assert!(conn.streams.contains_key(&1));
        // Removing a stream that is already gone is harmless.
        conn.remove_stream(4);
    }

    #[test]
    fn peer_opened_streams_get_the_longer_negotiation_timeout() {
        let timeouts = P2pTimeouts {
            select: Some(Duration::from_secs(5)),
            select_incoming_stream: Some(Duration::from_secs(10)),
            ..Default::default()
        };
        let start = Timestamp::ZERO;
        let at = |secs| start + Duration::from_secs(secs);

        let theirs = P2pNetworkStreamState::new_incoming(start).select;
        assert!(!theirs.is_stream_timed_out(at(9), &timeouts));
        assert!(theirs.is_stream_timed_out(at(10), &timeouts));

        let kind = token::StreamKind::Ping(token::PingAlgorithm::Ping1_0_0);
        let ours = P2pNetworkStreamState::new(kind, start).select;
        assert!(!ours.is_stream_timed_out(at(4), &timeouts));
        assert!(ours.is_stream_timed_out(at(5), &timeouts));
    }
}
