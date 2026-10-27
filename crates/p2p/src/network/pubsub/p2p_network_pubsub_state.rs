use super::{pb, BroadcastMessageId};
use crate::{token::BroadcastAlgorithm, ConnectionAddr, PeerId, StreamId};

use libp2p_identity::ParseError;
use mina_core::{
    p2p::P2pNetworkPubsubMessageCacheId,
    snark::{Snark, SnarkJobId},
    transaction::Transaction,
};
use mina_p2p_messages::gossip::GossipNetMessageV2;
use redux::Timestamp;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::Duration,
};

use malloc_size_of_derive::MallocSizeOf;

pub const IWANT_TIMEOUT_DURATION: Duration = Duration::from_secs(5);

/// State of the P2P Network PubSub system.
///
/// This struct maintains information about connected peers, message sequencing,
/// message caching, and topic subscriptions. It handles incoming and outgoing
/// messages, manages the mesh network topology, and ensures efficient message
/// broadcasting across the network.
#[derive(Default, Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
pub struct P2pNetworkPubsubState {
    /// State of each connected peer.
    #[with_malloc_size_of_func = "measurement::clients"]
    pub clients: BTreeMap<PeerId, P2pNetworkPubsubClientState>,

    /// Current message sequence number.
    ///
    /// Increments with each new message to ensure proper ordering and uniqueness.
    pub seq: u64,

    /// Messages awaiting cryptographic signing.
    pub to_sign: VecDeque<pb::Message>,

    /// Recently seen message identifiers to prevent duplication.
    ///
    /// Keeps a limited history of message signatures to avoid processing
    /// the same message multiple times.
    pub seen: VecDeque<Vec<u8>>,

    /// Cache of published messages for efficient retrieval and broadcasting.
    ///
    /// For quick access and reducing redundant data transmission across peers.
    pub mcache: P2pNetworkPubsubMessageCache,

    /// Incoming transactions from peers along with their nonces.
    pub incoming_transactions: Vec<(Transaction, u32)>,

    /// Incoming snarks from peers along with their nonces.
    pub incoming_snarks: Vec<(Snark, u32)>,

    /// Topics and their subscribed peers.
    #[with_malloc_size_of_func = "measurement::topics"]
    pub topics: BTreeMap<String, BTreeMap<PeerId, P2pNetworkPubsubClientTopicState>>,

    /// `iwant` requests, tracking the number of times peers have expressed interest in specific messages.
    pub iwant: VecDeque<P2pNetworkPubsubIwantRequestCount>,

    /// Which transports recently sent us each block as a peer's best tip.
    /// Decides which blocks [`super::P2pNetworkPubsubAction::WebRtcRebroadcast`]
    /// may gossip.
    pub block_sources: P2pNetworkPubsubBlockSources,

    /// Block-hash deduplication, retained for the entire eligible gossip window.
    #[serde(default)]
    pub block_gossip: super::block_gossip::BlockGossipState,
}

/// Which transports recently sent us each block as a peer's best tip: libp2p
/// (gossip or an RPC answer) or WebRTC.
///
/// We gossip a block to libp2p peers in a message of our own, binprot-encoded
/// again, only if it came from a WebRTC peer and from no libp2p peer. That is
/// how a block from a web node reaches the libp2p network. A block that came
/// as libp2p gossip is forwarded as the original message instead. One that a
/// libp2p peer gave us by RPC is already on the libp2p network, as that peer's
/// best tip, so gossiping it adds nothing, and it would put our encoding in
/// front of OCaml peers: OCaml bans the sender of gossip it can't decode, by
/// peer id and IP, for a day.
#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct P2pNetworkPubsubBlockSources {
    sources: BTreeMap<mina_p2p_messages::v2::StateHash, P2pNetworkPubsubBlockSource>,
    /// Hashes in `sources`, oldest first.
    queue: VecDeque<mina_p2p_messages::v2::StateHash>,
}

/// The transports a block came from, see [`P2pNetworkPubsubBlockSources`].
#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2pNetworkPubsubBlockSource {
    pub libp2p: bool,
    pub webrtc: bool,
}

impl P2pNetworkPubsubBlockSources {
    /// How many blocks to remember. Mainnet makes a block every few minutes,
    /// and a block is gossiped seconds after it arrives, once it is applied.
    pub const CAPACITY: usize = 128;

    /// Records that a peer sent us `hash` as its best tip, over libp2p or not.
    pub fn record(&mut self, hash: &mina_p2p_messages::v2::StateHash, is_libp2p: bool) {
        let source = match self.sources.get_mut(hash) {
            Some(source) => source,
            None => {
                if self.queue.len() >= Self::CAPACITY {
                    if let Some(oldest) = self.queue.pop_front() {
                        self.sources.remove(&oldest);
                    }
                }
                self.queue.push_back(hash.clone());
                self.sources.entry(hash.clone()).or_default()
            }
        };
        if is_libp2p {
            source.libp2p = true;
        } else {
            source.webrtc = true;
        }
    }

    pub fn get(
        &self,
        hash: &mina_p2p_messages::v2::StateHash,
    ) -> Option<P2pNetworkPubsubBlockSource> {
        self.sources.get(hash).copied()
    }

    /// Whether we may gossip `hash` to libp2p peers in a message of our own:
    /// it came from a WebRTC peer, and from no libp2p peer.
    pub fn came_only_from_webrtc(&self, hash: &mina_p2p_messages::v2::StateHash) -> bool {
        self.get(hash)
            .is_some_and(|source| source.webrtc && !source.libp2p)
    }

    pub fn len(&self) -> usize {
        self.sources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
pub struct P2pNetworkPubsubIwantRequestCount {
    pub message_id: Vec<u8>,
    #[with_malloc_size_of_func = "measurement::timestamps"]
    pub count: Vec<Timestamp>,
}

impl P2pNetworkPubsubState {
    pub fn prune_peer_state(&mut self, peer_id: &PeerId) {
        self.clients.remove(peer_id);
        self.block_gossip.forget_peer(peer_id);
    }

    /// Forgets a reset stream of a peer. If it was our outgoing stream we stop
    /// writing to the peer until the scheduler has reopened it; if it was the
    /// peer's, its unfinished message goes.
    pub fn prune_stream_state(&mut self, peer_id: &PeerId, stream_id: StreamId, incoming: bool) {
        let Some(client) = self.clients.get_mut(peer_id) else {
            return;
        };
        if client.outgoing_stream_id == Some(stream_id) {
            client.outgoing_stream_id = None;
            self.block_gossip.forget_peer(peer_id);
        } else if incoming {
            client.clear_buffer();
        }
    }

    pub fn filter_iwant_message_ids(&mut self, message_id: &[u8], timestamp: Timestamp) -> bool {
        if self
            .mcache
            .get_message_from_raw_message_id(message_id)
            .is_some()
        {
            return false;
        }

        let message_count = self
            .iwant
            .iter_mut()
            .find(|message| message.message_id == message_id);

        match message_count {
            Some(message) => {
                let message_counts = std::mem::take(&mut message.count);

                message.count = message_counts
                    .into_iter()
                    .filter(|time| {
                        timestamp
                            .checked_sub(*time)
                            .is_some_and(|duration| duration < IWANT_TIMEOUT_DURATION)
                    })
                    .collect();

                if message.count.len() < 3 {
                    message.count.push(timestamp);
                    return true;
                }

                false
            }
            None => {
                let message_count = P2pNetworkPubsubIwantRequestCount {
                    message_id: message_id.to_vec(),
                    count: vec![timestamp],
                };

                self.iwant.push_back(message_count);
                if self.iwant.len() > 10 {
                    self.iwant.pop_front();
                }

                true
            }
        }
    }

    pub fn clear_incoming(&mut self) {
        self.incoming_transactions.clear();
        self.incoming_snarks.clear();

        self.incoming_transactions.shrink_to(0x20);
        self.incoming_snarks.shrink_to(0x20);
    }
}

/// State of a pubsub client connected to a peer.
///
/// This struct maintains essential information about the client's protocol,
/// connection details, message buffers, and caching mechanisms. It facilitates
/// efficient message handling and broadcasting within the pubsub system.
#[derive(Serialize, Deserialize, Debug, Clone, MallocSizeOf)]
pub struct P2pNetworkPubsubClientState {
    /// Broadcast algorithm used for this client.
    pub protocol: BroadcastAlgorithm,

    /// Connection address of the peer.
    pub addr: ConnectionAddr,

    /// Outgoing stream identifier, if any.
    ///
    /// - `Some(StreamId)`: Indicates an active outgoing stream.
    /// - `None`: No outgoing stream is currently established.
    pub outgoing_stream_id: Option<StreamId>,

    /// Current RPC message being constructed or processed.
    ///
    /// - `subscriptions`: List of subscription options for various topics.
    /// - `publish`: Messages queued for publishing.
    /// - `control`: Control commands for managing the mesh network.
    pub message: pb::Rpc,

    /// Cache of recently published messages.
    pub cache: P2pNetworkPubsubRecentlyPublishCache,

    /// Explicit block requests queued for this RPC, allowed to bypass the
    /// unsolicited publication cache. Consumed at the final send-time check.
    #[serde(default)]
    #[with_malloc_size_of_func = "measurement::message_ids"]
    pub iwant_replies: BTreeSet<P2pNetworkPubsubMessageCacheId>,

    /// Buffer for incoming data fragments.
    ///
    /// Stores partial data received from peers, facilitating the assembly of complete
    /// messages when all fragments are received.
    pub buffer: Vec<u8>,

    /// Collection of incoming messages from the peer.
    ///
    /// Holds fully decoded `pb::Message` instances received from the peer,
    /// ready for further handling such as validation, caching, and broadcasting.
    pub incoming_messages: Vec<pb::Message>,
}

impl P2pNetworkPubsubClientState {
    pub fn reply_to_iwant(&mut self, message: &pb::Message) {
        if !super::block_gossip::BlockGossipState::is_block(message) {
            self.publish(message);
            return;
        }
        let Ok(id) = compute_message_id(message) else {
            return;
        };
        // Coalesce repeated IDs, including a copy already queued by broadcast.
        if self.iwant_replies.insert(id)
            && !self
                .message
                .publish
                .iter()
                .any(|msg| compute_message_id(msg).ok() == Some(id))
        {
            self.message.publish.push(message.clone());
        }
    }

    pub fn publish(&mut self, message: &pb::Message) {
        let Ok(id) = compute_message_id(message) else {
            self.message.publish.push(message.clone());
            return;
        };
        if self.cache.map.insert(id) {
            self.message.publish.push(message.clone());
        }
        self.cache.queue.push_back(id);
        if self.cache.queue.len() > 50 {
            if let Some(id) = self.cache.queue.pop_front() {
                self.cache.map.remove(&id);
            }
        }
    }

    pub fn clear_buffer(&mut self) {
        self.buffer.clear();
        self.buffer.shrink_to(0x2000);
    }

    pub fn clear_incoming(&mut self) {
        self.incoming_messages.clear();
        self.incoming_messages.shrink_to(0x20)
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct P2pNetworkPubsubRecentlyPublishCache {
    pub map: BTreeSet<P2pNetworkPubsubMessageCacheId>,
    pub queue: VecDeque<P2pNetworkPubsubMessageCacheId>,
}

// TODO: store blocks, snarks and txs separately
#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct P2pNetworkPubsubMessageCache {
    pub map: BTreeMap<P2pNetworkPubsubMessageCacheId, P2pNetworkPubsubMessageCacheMessage>,
    pub queue: VecDeque<P2pNetworkPubsubMessageCacheId>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum P2pNetworkPubsubMessageCacheMessage {
    Init {
        message: pb::Message,
        content: GossipNetMessageV2,
        peer_id: PeerId,
        time: Timestamp,
    },
    PreValidatedBlockMessage {
        block_hash: mina_p2p_messages::v2::StateHash,
        message: pb::Message,
        peer_id: PeerId,
        time: Timestamp,
    },
    PreValidatedSnark {
        job_id: SnarkJobId,
        snark: Box<mina_core::snark::Snark>,
        message: pb::Message,
        peer_id: PeerId,
        time: Timestamp,
    },
    PreValidated {
        message: pb::Message,
        peer_id: PeerId,
        time: Timestamp,
    },
    Validated {
        message: pb::Message,
        peer_id: PeerId,
        time: Timestamp,
    },
}

// TODO: what if wasm32?
// How to test it?
pub fn compute_message_id(
    message: &pb::Message,
) -> Result<P2pNetworkPubsubMessageCacheId, ParseError> {
    let source = source_from_message(message)?;

    let seqno = message
        .seqno
        .as_ref()
        .and_then(|b| <[u8; 8]>::try_from(b.as_slice()).ok())
        .map(u64::from_be_bytes)
        .unwrap_or_default();

    Ok(P2pNetworkPubsubMessageCacheId { source, seqno })
}

macro_rules! enum_field {
    ($field:ident: $field_type:ty) => {
        pub fn $field(&self) -> &$field_type {
            match self {
                Self::Init { $field, .. }
                | Self::PreValidated { $field, .. }
                | Self::PreValidatedBlockMessage { $field, .. }
                | Self::PreValidatedSnark { $field, .. }
                | Self::Validated { $field, .. } => $field,
            }
        }
    };
}

impl P2pNetworkPubsubMessageCacheMessage {
    enum_field!(message: pb::Message);
    enum_field!(time: Timestamp);
    enum_field!(peer_id: PeerId);
}

impl P2pNetworkPubsubMessageCache {
    const CAPACITY: usize = 100;

    pub fn put(
        &mut self,
        message: pb::Message,
        content: GossipNetMessageV2,
        peer_id: PeerId,
        time: Timestamp,
    ) -> Result<P2pNetworkPubsubMessageCacheId, ParseError> {
        let id = compute_message_id(&message)?;
        self.map.insert(
            id,
            P2pNetworkPubsubMessageCacheMessage::Init {
                message,
                content,
                time,
                peer_id,
            },
        );

        self.queue.push_back(id);
        if self.queue.len() > Self::CAPACITY {
            if let Some(id) = self.queue.pop_front() {
                self.map.remove(&id);
            }
        }
        Ok(id)
    }

    pub fn get_message(&self, id: &P2pNetworkPubsubMessageCacheId) -> Option<&GossipNetMessageV2> {
        let message = self.map.get(id)?;
        match message {
            P2pNetworkPubsubMessageCacheMessage::Init { content, .. } => Some(content),
            _ => None,
        }
    }

    pub fn contains_broadcast_id(&self, message_id: &BroadcastMessageId) -> bool {
        match message_id {
            BroadcastMessageId::BlockHash { hash } => self
                .map
                .values()
                .any(|message| matches!(message, P2pNetworkPubsubMessageCacheMessage::PreValidatedBlockMessage { block_hash, .. } if block_hash == hash)),
            BroadcastMessageId::MessageId { message_id } => {
                self.map.contains_key(message_id)
            },
            BroadcastMessageId::Snark { job_id: snark_job_id } => {
                self
                    .map
                    .values()
                    .any(|message| matches!(message, P2pNetworkPubsubMessageCacheMessage::PreValidatedSnark { job_id,.. } if job_id == snark_job_id))
            },
        }
    }

    /// Select the exact proof and fee that passed validation. Endpoint job IDs
    /// alone can identify different statements, provers, fees, and proofs.
    pub fn snark_message_id(
        &self,
        checked: &mina_core::snark::Snark,
    ) -> Option<P2pNetworkPubsubMessageCacheId> {
        self.map.iter().find_map(|(id, message)| match message {
            P2pNetworkPubsubMessageCacheMessage::PreValidatedSnark { snark, .. }
                if snark.fee == checked.fee
                    && snark.snarker == checked.snarker
                    && snark.proofs == checked.proofs =>
            {
                Some(*id)
            }
            _ => None,
        })
    }

    pub fn get_message_id_and_message(
        &mut self,
        message_id: &BroadcastMessageId,
    ) -> Option<(
        P2pNetworkPubsubMessageCacheId,
        &mut P2pNetworkPubsubMessageCacheMessage,
    )> {
        match message_id {
            BroadcastMessageId::BlockHash { hash } => {
                self.map
                    .iter_mut()
                    .find_map(|(message_id, message)| match message {
                        P2pNetworkPubsubMessageCacheMessage::PreValidatedBlockMessage {
                            block_hash,
                            ..
                        } if block_hash == hash => Some((*message_id, message)),
                        _ => None,
                    })
            }
            BroadcastMessageId::MessageId { message_id } => self
                .map
                .get_mut(message_id)
                .map(|content| (*message_id, content)),
            BroadcastMessageId::Snark {
                job_id: snark_job_id,
            } => {
                self.map
                    .iter_mut()
                    .find_map(|(message_id, message)| match message {
                        P2pNetworkPubsubMessageCacheMessage::PreValidatedSnark {
                            job_id, ..
                        } if job_id == snark_job_id => Some((*message_id, message)),
                        _ => None,
                    })
            }
        }
    }

    pub fn remove_message(
        &mut self,
        message_id: P2pNetworkPubsubMessageCacheId,
    ) -> Option<P2pNetworkPubsubMessageCacheMessage> {
        let message = self.map.remove(&message_id);
        if let Some(position) = self.queue.iter().position(|id| id == &message_id) {
            self.queue.remove(position);
        }
        message
    }

    pub fn get_message_from_raw_message_id(
        &self,
        message_id: &[u8],
    ) -> Option<&P2pNetworkPubsubMessageCacheMessage> {
        self.map.iter().find_map(|(key, value)| {
            if key.to_raw_bytes() == message_id {
                Some(value)
            } else {
                None
            }
        })
    }
}

pub fn source_from_message(message: &pb::Message) -> Result<libp2p_identity::PeerId, ParseError> {
    let source_bytes = message
        .from
        .as_ref()
        .map(AsRef::as_ref)
        .unwrap_or(&[0, 1, 0][..]);

    libp2p_identity::PeerId::from_bytes(source_bytes)
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct P2pNetworkPubsubClientTopicState {
    pub mesh: P2pNetworkPubsubClientMeshAddingState,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum P2pNetworkPubsubClientMeshAddingState {
    #[default]
    Initial,
    TheyRefused,
    WeRefused,
    Added,
}

impl P2pNetworkPubsubClientState {
    pub fn message_is_empty(&self) -> bool {
        self.message.subscriptions.is_empty()
            && self.message.publish.is_empty()
            && self.message.control.is_none()
    }
}

impl P2pNetworkPubsubClientTopicState {
    pub fn on_mesh(&self) -> bool {
        matches!(&self.mesh, P2pNetworkPubsubClientMeshAddingState::Added)
    }
}

mod measurement {
    use malloc_size_of::{MallocSizeOf, MallocSizeOfOps};
    use std::mem;

    use super::*;

    pub fn clients(
        val: &BTreeMap<PeerId, P2pNetworkPubsubClientState>,
        ops: &mut MallocSizeOfOps,
    ) -> usize {
        val.values().map(|v| v.size_of(ops)).sum()
    }

    pub fn topics(
        val: &BTreeMap<String, BTreeMap<PeerId, P2pNetworkPubsubClientTopicState>>,
        ops: &mut MallocSizeOfOps,
    ) -> usize {
        val.iter()
            .map(|(k, v)| k.size_of(ops) + v.size_of(ops))
            .sum()
    }

    pub fn timestamps(val: &Vec<Timestamp>, _ops: &mut MallocSizeOfOps) -> usize {
        val.capacity() * mem::size_of::<Timestamp>()
    }

    pub fn message_ids(
        val: &BTreeSet<P2pNetworkPubsubMessageCacheId>,
        _ops: &mut MallocSizeOfOps,
    ) -> usize {
        val.len() * size_of::<P2pNetworkPubsubMessageCacheId>()
    }

    impl MallocSizeOf for P2pNetworkPubsubRecentlyPublishCache {
        fn size_of(&self, _ops: &mut malloc_size_of::MallocSizeOfOps) -> usize {
            let map_size = self.map.len() * size_of::<P2pNetworkPubsubMessageCacheId>();
            let queue_size = self.queue.capacity() * size_of::<P2pNetworkPubsubMessageCacheId>();
            map_size + queue_size
        }
    }

    impl MallocSizeOf for P2pNetworkPubsubMessageCache {
        fn size_of(&self, _ops: &mut malloc_size_of::MallocSizeOfOps) -> usize {
            let map_size = self.map.len()
                * (size_of::<P2pNetworkPubsubMessageCacheId>()
                    + size_of::<P2pNetworkPubsubMessageCacheMessage>());
            let queue_size = self.queue.capacity() * size_of::<P2pNetworkPubsubMessageCacheId>();
            map_size + queue_size
        }
    }

    impl MallocSizeOf for P2pNetworkPubsubClientTopicState {
        fn size_of(&self, _ops: &mut MallocSizeOfOps) -> usize {
            0
        }
    }

    impl MallocSizeOf for P2pNetworkPubsubBlockSources {
        fn size_of(&self, _ops: &mut MallocSizeOfOps) -> usize {
            let hash = size_of::<mina_p2p_messages::v2::StateHash>();
            self.sources.len() * (hash + size_of::<P2pNetworkPubsubBlockSource>())
                + self.queue.capacity() * hash
        }
    }
}

#[cfg(test)]
mod block_source_tests {
    use mina_p2p_messages::{
        bigint::BigInt,
        v2::{DataHashLibStateHashStableV1, StateHash},
    };

    use super::{P2pNetworkPubsubBlockSource, P2pNetworkPubsubBlockSources};

    fn hash(n: u32) -> StateHash {
        let mut bytes = [0; 32];
        bytes[..4].copy_from_slice(&n.to_le_bytes());
        DataHashLibStateHashStableV1(BigInt::from_bytes(bytes)).into()
    }

    #[test]
    fn only_a_block_from_webrtc_alone_may_be_gossiped() {
        let mut sources = P2pNetworkPubsubBlockSources::default();
        let (webrtc, libp2p, both, unknown) = (hash(1), hash(2), hash(3), hash(4));
        sources.record(&webrtc, false);
        sources.record(&libp2p, true);
        sources.record(&both, false);
        sources.record(&both, true);

        assert!(sources.came_only_from_webrtc(&webrtc));
        // Gossip or an RPC answer from a libp2p peer: libp2p has it already.
        assert!(!sources.came_only_from_webrtc(&libp2p));
        // Seen from a libp2p peer too, in either order.
        assert!(!sources.came_only_from_webrtc(&both));
        sources.record(&webrtc, true);
        assert!(!sources.came_only_from_webrtc(&webrtc));
        // Our own blocks, and blocks we no longer remember.
        assert!(!sources.came_only_from_webrtc(&unknown));
        assert_eq!(
            sources.get(&both),
            Some(P2pNetworkPubsubBlockSource {
                libp2p: true,
                webrtc: true
            })
        );
    }

    #[test]
    fn remembers_the_latest_blocks_only() {
        let mut sources = P2pNetworkPubsubBlockSources::default();
        let capacity = P2pNetworkPubsubBlockSources::CAPACITY as u32;
        for n in 0..capacity {
            sources.record(&hash(n), false);
        }
        // A block seen again doesn't take another place.
        sources.record(&hash(0), false);
        assert_eq!(sources.len(), capacity as usize);
        assert!(sources.came_only_from_webrtc(&hash(0)));

        sources.record(&hash(capacity), false);
        assert_eq!(sources.len(), capacity as usize);
        assert!(sources.get(&hash(0)).is_none());
        assert!(sources.came_only_from_webrtc(&hash(1)));
        assert!(sources.came_only_from_webrtc(&hash(capacity)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(outgoing_stream_id: Option<StreamId>) -> P2pNetworkPubsubClientState {
        P2pNetworkPubsubClientState {
            protocol: BroadcastAlgorithm::Meshsub1_1_0,
            addr: ConnectionAddr {
                sock_addr: "127.0.0.1:8302".parse().unwrap(),
                incoming: false,
            },
            outgoing_stream_id,
            message: pb::Rpc {
                subscriptions: vec![],
                publish: vec![],
                control: None,
            },
            cache: Default::default(),
            iwant_replies: Default::default(),
            buffer: vec![1, 2, 3],
            incoming_messages: vec![],
        }
    }

    #[test]
    fn a_reset_stream_is_forgotten_but_not_the_peer() {
        let peer = PeerId::from_bytes([1; 32]);
        let mut state = P2pNetworkPubsubState::default();
        state.clients.insert(peer, client(Some(3)));

        // A peer's stream that isn't pubsub: our stream and the buffer stay.
        state.prune_stream_state(&peer, 4, false);
        assert_eq!(state.clients[&peer].outgoing_stream_id, Some(3));
        assert_eq!(state.clients[&peer].buffer, [1, 2, 3]);

        // The peer's pubsub stream: its unfinished message goes.
        state.prune_stream_state(&peer, 4, true);
        assert!(state.clients[&peer].buffer.is_empty());

        // Our pubsub stream: we stop writing, but keep the peer.
        state.prune_stream_state(&peer, 3, false);
        assert_eq!(state.clients[&peer].outgoing_stream_id, None);
        assert!(state.clients.contains_key(&peer));
    }
}
