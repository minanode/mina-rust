use super::{pb, BroadcastMessageId};
use crate::{token::BroadcastAlgorithm, ConnectionAddr, Data, P2pState, PeerId, StreamId};
use mina_core::{p2p::P2pNetworkPubsubMessageCacheId, ActionEvent};
use mina_p2p_messages::gossip::GossipNetMessageV2;
use serde::{Deserialize, Serialize};

/// Actions that can occur within the P2P Network PubSub system.
///
/// Managing pubsub streams, handling incoming and outgoing messages,
/// and maintaining the mesh network topology.
///
/// **Common Fields:**
/// - `peer_id`: The identifier of the peer associated with the action.
/// - `addr`: The connection address of the peer.
/// - `stream_id`: The unique identifier of the stream.
/// - `topic_id`: The identifier of the topic involved in the action.
#[derive(Serialize, Deserialize, Debug, Clone, ActionEvent)]
pub enum P2pNetworkPubsubAction {
    /// Create a new stream, either incoming or outgoing.
    ///
    /// **Fields:**
    /// - `incoming`: Indicates if the stream is incoming (`true`) or outgoing (`false`).
    /// - `protocol`: The broadcast algorithm used for the stream.
    NewStream {
        incoming: bool,
        peer_id: PeerId,
        addr: ConnectionAddr,
        stream_id: StreamId,
        protocol: BroadcastAlgorithm,
    },

    /// Process incoming raw data from a peer.
    ///
    /// **Fields:**
    /// - `data`: The raw data payload received.
    /// - `seen_limit`: The limit for tracking seen messages to prevent duplication.
    IncomingData {
        peer_id: PeerId,
        addr: ConnectionAddr,
        stream_id: StreamId,
        data: Data,
        seen_limit: usize,
    },

    /// Validate a batch of decoded incoming messages.
    ValidateIncomingMessages {
        peer_id: PeerId,
        seen_limit: usize,
        addr: ConnectionAddr,
    },

    /// Handle a fully decoded and validated message received from a peer.
    ///
    /// **Fields:**
    /// - `message`: The decoded protobuf message.
    /// - `seen_limit`: The limit for tracking seen messages to prevent duplication.
    IncomingMessage {
        peer_id: PeerId,
        message: pb::Message,
        seen_limit: usize,
    },

    /// Clean up temporary states after processing an incoming message.
    IncomingMessageCleanup {
        peer_id: PeerId,
    },

    /// Add a peer to the mesh network for a specific topic.
    Graft {
        peer_id: PeerId,
        topic_id: String,
    },

    /// Remove a peer from the mesh network for a specific topic.
    Prune {
        peer_id: PeerId,
        topic_id: String,
    },

    /// Rebroadcast message received from WebRTC connection.
    ///
    /// Expected to be dispatched after the message has been processed,
    /// in spite of whether it was received from libp2p or webrtc network.
    ///
    /// If received from libp2p network, or if we have already broadcasted
    /// this message, the message will be in the `mcache` state,
    /// in which case the action won't be enabled (will be filtered out).
    ///
    /// A block is rebroadcast only if a WebRTC peer sent it to us and no
    /// libp2p peer did, by gossip or RPC (see
    /// [`super::P2pNetworkPubsubBlockSources`]).
    ///
    /// Logged at info, as every message it gossips is in our own encoding.
    #[action_event(level = debug, fields(message_kind = gossip_message_kind(message)))]
    WebRtcRebroadcast {
        message: GossipNetMessageV2,
    },

    /// Initiate the broadcasting of a message to all subscribed peers.
    ///
    /// **Fields:**
    /// - `message`: The gossip network message to broadcast.
    Broadcast {
        message: GossipNetMessageV2,
    },

    /// Prepare a message for signing before broadcasting.
    ///
    /// **Fields:**
    /// - `seqno`: The sequence number of the message.
    /// - `author`: The identifier of the peer authoring the message.
    /// - `data`: The data payload of the message.
    /// - `topic`: The topic under which the message is published.
    Sign {
        seqno: u64,
        author: PeerId,
        data: Data,
        topic: String,
    },

    /// An error occured during the signing process.
    #[action_event(level = warn, fields(display(author), display(topic)))]
    SignError {
        author: PeerId,
        topic: String,
    },

    /// Finalize the broadcasting of a signed message by attaching the signature.
    ///
    /// **Fields:**
    /// - `signature`: The cryptographic signature of the message.
    BroadcastSigned {
        signature: Data,
    },

    /// Prepare an outgoing message to send to a specific peer.
    OutgoingMessage {
        peer_id: PeerId,
    },

    /// Clear the outgoing message state for a specific peer after sending.
    OutgoingMessageClear {
        peer_id: PeerId,
    },

    /// An error occured during the sending of an outgoing message.
    ///
    /// **Fields:**
    /// - `msg`: The protobuf message that failed to send.
    #[action_event(level = warn, fields(display(peer_id), debug(msg)))]
    OutgoingMessageError {
        msg: pb::Rpc,
        peer_id: PeerId,
    },

    /// Send encoded data over an outgoing stream to a specific peer.
    ///
    /// **Fields:**
    /// - `data`: The encoded data to be sent.
    OutgoingData {
        data: Data,
        peer_id: PeerId,
    },

    HandleIncomingMessage {
        message: pb::Message,
        message_content: GossipNetMessageV2,
        peer_id: PeerId,
    },

    ValidateIncomingMessage {
        message_id: P2pNetworkPubsubMessageCacheId,
    },

    /// Delete expired messages from state
    PruneMessages {},

    RejectMessage {
        message_id: Option<BroadcastMessageId>,
        peer_id: Option<PeerId>,
        reason: String,
    },
    IgnoreMessage {
        message_id: Option<BroadcastMessageId>,
        reason: String,
    },

    // After message is fully validated, broadcast it to other peers
    BroadcastValidatedMessage {
        message_id: BroadcastMessageId,
    },
}

/// The kind of a gossip message, for logs.
fn gossip_message_kind(message: &GossipNetMessageV2) -> &'static str {
    match message {
        GossipNetMessageV2::NewState(_) => "block",
        GossipNetMessageV2::SnarkPoolDiff { .. } => "snark_pool_diff",
        GossipNetMessageV2::TransactionPoolDiff { .. } => "transaction_pool_diff",
    }
}

impl From<P2pNetworkPubsubAction> for crate::P2pAction {
    fn from(value: P2pNetworkPubsubAction) -> Self {
        crate::P2pAction::Network(value.into())
    }
}

impl redux::EnablingCondition<P2pState> for P2pNetworkPubsubAction {
    fn is_enabled(&self, state: &P2pState, time: redux::Timestamp) -> bool {
        let pubsub = &state.network.scheduler.broadcast_state;
        match self {
            P2pNetworkPubsubAction::OutgoingMessage { peer_id } => pubsub
                .clients
                .get(peer_id)
                .is_some_and(|s| !s.message_is_empty()),
            P2pNetworkPubsubAction::Prune { peer_id, topic_id } => pubsub
                .topics
                .get(topic_id)
                .is_some_and(|topics| topics.contains_key(peer_id)),
            P2pNetworkPubsubAction::WebRtcRebroadcast { message } => {
                if !pubsub.block_gossip.can_announce(message, time) {
                    return false;
                }
                // A block only if a WebRTC peer sent it and no libp2p peer
                // did (`P2pNetworkPubsubBlockSources`). This is checked first:
                // the check below encodes the whole block.
                if let GossipNetMessageV2::NewState(block) = message {
                    let came_only_from_webrtc = block
                        .try_hash()
                        .is_ok_and(|hash| pubsub.block_sources.came_only_from_webrtc(&hash));
                    if !came_only_from_webrtc {
                        return false;
                    }
                }
                let source = super::webrtc_source_sk(message)
                    .public_key()
                    .peer_id()
                    .try_into()
                    .unwrap();
                pubsub
                    .mcache
                    .get_message(&P2pNetworkPubsubMessageCacheId { source, seqno: 0 })
                    .is_none()
            }
            P2pNetworkPubsubAction::Broadcast { message } => {
                if let Some(reason) = pubsub.block_gossip.announce_rejection(message, time) {
                    if let GossipNetMessageV2::NewState(block) = message {
                        mina_core::debug!(time;
                            kind = "BlockGossipSuppressed",
                            state_hash = block.header.protocol_state.try_hash().ok().map(|hash| hash.to_string()),
                            slot = block.header.protocol_state.body.consensus_state.curr_global_slot_since_hard_fork.slot_number.as_u32(),
                            reason = reason,
                            phase = "initial",
                        );
                    }
                    false
                } else {
                    true
                }
            }
            P2pNetworkPubsubAction::BroadcastValidatedMessage { message_id }
            | P2pNetworkPubsubAction::RejectMessage {
                message_id: Some(message_id),
                ..
            } => pubsub.mcache.contains_broadcast_id(message_id),
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use binprot::BinProtRead;
    use mina_core::MAINNET_CHAIN_ID;
    use mina_p2p_messages::{
        gossip::GossipNetMessageV2, list::List, rpc_kernel::ResponsePayload,
        v2::MinaBlockBlockStableV2,
    };
    use redux::{EnablingCondition, Timestamp};

    use super::P2pNetworkPubsubAction;
    use crate::{
        channels::ChannelId, identity::SecretKey, P2pCallbacks, P2pConfig, P2pMeshsubConfig,
        P2pState,
    };

    /// Block 563900 as a Mesa mainnet node served it (`mina-rpc-capture`).
    fn mesa_block() -> Arc<MinaBlockBlockStableV2> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../p2p-messages/tests/files/mesa/get_transition_chain_v2_563900_zkapp.bin"
        );
        let bytes = std::fs::read(path).unwrap();
        let payload =
            ResponsePayload::<Option<List<MinaBlockBlockStableV2>>>::binprot_read(&mut &bytes[..])
                .unwrap();
        let blocks = payload.0.unwrap().0.unwrap();
        Arc::new(blocks.iter().next().unwrap().clone())
    }

    fn p2p_state() -> P2pState {
        let config = P2pConfig {
            libp2p_port: None,
            listen_port: None,
            identity_pub_key: SecretKey::from_bytes([3; 32]).public_key(),
            initial_peers: vec![],
            external_addrs: vec![],
            enabled_channels: ChannelId::iter_all().collect(),
            timeouts: Default::default(),
            limits: Default::default(),
            peer_discovery: false,
            meshsub: P2pMeshsubConfig::default(),
            catchup_rpc: Default::default(),
        };
        P2pState::new(config, P2pCallbacks::default(), &MAINNET_CHAIN_ID)
    }

    fn rebroadcast_enabled(state: &P2pState, block: &Arc<MinaBlockBlockStableV2>) -> bool {
        P2pNetworkPubsubAction::WebRtcRebroadcast {
            message: GossipNetMessageV2::NewState(block.clone()),
        }
        .is_enabled(state, super::super::block_gossip::tests::slot_start(block))
    }

    /// A block a libp2p peer gave us, by gossip or RPC, is never gossiped in
    /// our own encoding; one from a WebRTC peer alone is.
    #[test]
    fn rebroadcasts_only_blocks_from_webrtc_peers() {
        let block = mesa_block();
        let hash = block.try_hash().unwrap();

        let mut state = p2p_state();
        // Not received from any peer: our own block, or one we forgot.
        assert!(!rebroadcast_enabled(&state, &block));

        let sources = &mut state.network.scheduler.broadcast_state.block_sources;
        sources.record(&hash, true);
        assert!(!rebroadcast_enabled(&state, &block));

        let mut state = p2p_state();
        let sources = &mut state.network.scheduler.broadcast_state.block_sources;
        sources.record(&hash, false);
        assert!(rebroadcast_enabled(&state, &block));

        // Then from a libp2p peer too, before it became our best tip.
        let sources = &mut state.network.scheduler.broadcast_state.block_sources;
        sources.record(&hash, true);
        assert!(!rebroadcast_enabled(&state, &block));
    }
    #[test]
    fn producer_and_best_tip_bridge_obey_age_genesis_and_duplicate_guards() {
        use super::super::block_gossip::tests::{message, slot_start};
        let mut block = mesa_block();
        let time = slot_start(&block);
        let mut state = p2p_state();
        state
            .network
            .scheduler
            .broadcast_state
            .block_sources
            .record(&block.try_hash().unwrap(), false);
        for action in [
            P2pNetworkPubsubAction::Broadcast {
                message: GossipNetMessageV2::NewState(block.clone()),
            },
            P2pNetworkPubsubAction::WebRtcRebroadcast {
                message: GossipNetMessageV2::NewState(block.clone()),
            },
        ] {
            assert!(action.is_enabled(&state, time));
            assert!(!action.is_enabled(&state, time + 90_000_000_000));
            assert!(!action.is_enabled(&state, Timestamp::ZERO));
        }
        assert!(state
            .network
            .scheduler
            .broadcast_state
            .block_gossip
            .announce(&message(block.clone(), 1), time));
        assert!(!P2pNetworkPubsubAction::Broadcast {
            message: GossipNetMessageV2::NewState(block.clone())
        }
        .is_enabled(&state, time));
        assert!(!rebroadcast_enabled(&state, &block));
        Arc::make_mut(&mut block)
            .header
            .protocol_state
            .body
            .consensus_state
            .curr_global_slot_since_hard_fork
            .slot_number =
            mina_p2p_messages::v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(
                0.into(),
            );
        let time = slot_start(&block);
        assert!(!P2pNetworkPubsubAction::Broadcast {
            message: GossipNetMessageV2::NewState(block)
        }
        .is_enabled(&state, time));
    }
}
