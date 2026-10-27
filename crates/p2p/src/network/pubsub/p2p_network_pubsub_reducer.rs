use std::{collections::btree_map::Entry, time::Duration};

use mina_core::{
    block::BlockWithHash, bug_condition, fuzz_maybe, fuzzed_maybe, snark::Snark, Substate,
};
use mina_p2p_messages::{
    gossip::{self, GossipNetMessageV2},
    v2::NetworkPoolSnarkPoolDiffVersionedStableV2,
};
use redux::{Dispatcher, Timestamp};

use crate::{
    channels::{snark::P2pChannelsSnarkAction, transaction::P2pChannelsTransactionAction},
    disconnection::{P2pDisconnectionAction, P2pDisconnectionReason},
    peer::P2pPeerAction,
    Data, P2pConfig, P2pNetworkYamuxAction, P2pState, PeerId,
};

use super::{
    p2p_network_pubsub_state::{
        source_from_message, P2pNetworkPubsubClientMeshAddingState,
        P2pNetworkPubsubMessageCacheMessage,
    },
    pb::{self, Message},
    P2pNetworkPubsubAction, P2pNetworkPubsubClientState, P2pNetworkPubsubEffectfulAction,
    P2pNetworkPubsubMessageCacheId, P2pNetworkPubsubState, TOPIC,
};

const MAX_MESSAGE_KEEP_DURATION: Duration = Duration::from_secs(300);

/// The largest gossipsub RPC we take from a peer, OCaml's largest message.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/app/libp2p_helper/src/libp2p_helper/config_msg.go#L199>
const MAX_RPC_SIZE: usize = 32 * 1024 * 1024;

impl P2pNetworkPubsubState {
    pub fn reducer<Action, State>(
        mut state_context: Substate<Action, State, Self>,
        action: redux::ActionWithMeta<P2pNetworkPubsubAction>,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let pubsub_state = state_context.get_substate_mut()?;
        let (action, meta) = action.split();
        let time = meta.time();

        match action {
            P2pNetworkPubsubAction::NewStream {
                incoming: true,
                peer_id,
                addr,
                protocol,
                ..
            } => {
                let entry = pubsub_state.clients.entry(peer_id);
                // preserve it
                let outgoing_stream_id = match &entry {
                    Entry::Occupied(v) => v.get().outgoing_stream_id,
                    Entry::Vacant(_) => None,
                };
                let state = entry.or_insert_with(|| P2pNetworkPubsubClientState {
                    protocol,
                    addr,
                    outgoing_stream_id,
                    message: pb::Rpc {
                        subscriptions: vec![],
                        publish: vec![],
                        control: None,
                    },
                    cache: Default::default(),
                    iwant_replies: Default::default(),
                    buffer: vec![],
                    incoming_messages: vec![],
                });
                state.protocol = protocol;
                state.addr = addr;
                // A peer sends gossip on one stream, and opens another only
                // once the last one failed, so the start of an RPC buffered
                // from that one will never be completed. Kept, it would be
                // taken for the start of this stream's first RPC.
                state.clear_buffer();

                pubsub_state
                    .topics
                    .entry(super::TOPIC.to_owned())
                    .or_default()
                    .insert(peer_id, Default::default());

                Ok(())
            }
            P2pNetworkPubsubAction::NewStream {
                incoming: false,
                peer_id,
                stream_id,
                addr,
                protocol,
            } => {
                let state = pubsub_state.clients.entry(peer_id).or_insert_with(|| {
                    P2pNetworkPubsubClientState {
                        protocol,
                        addr,
                        outgoing_stream_id: Some(stream_id),
                        message: pb::Rpc {
                            subscriptions: vec![],
                            publish: vec![],
                            control: None,
                        },
                        cache: Default::default(),
                        iwant_replies: Default::default(),
                        buffer: vec![],
                        incoming_messages: vec![],
                    }
                });
                state.outgoing_stream_id = Some(stream_id);
                state.protocol = protocol;
                state.addr = addr;

                pubsub_state
                    .topics
                    .entry(TOPIC.to_owned())
                    .or_default()
                    .insert(peer_id, Default::default());

                if let Some(state) = pubsub_state.clients.get_mut(&peer_id) {
                    state.message.subscriptions.push(pb::rpc::SubOpts {
                        subscribe: Some(true),
                        topic_id: Some(TOPIC.to_owned()),
                    });
                }

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let config: &P2pConfig = state.substate()?;
                let state: &P2pNetworkPubsubState = state.substate()?;

                let Some(map) = state.topics.get(TOPIC) else {
                    // must have this topic already
                    return Ok(());
                };
                dispatcher.push(P2pNetworkPubsubAction::OutgoingMessage { peer_id });
                let mesh_size = map.values().filter(|s| s.on_mesh()).count();
                if mesh_size < config.meshsub.outbound_degree_desired {
                    dispatcher.push(P2pNetworkPubsubAction::Graft {
                        peer_id,
                        topic_id: TOPIC.to_owned(),
                    });
                }

                Ok(())
            }
            P2pNetworkPubsubAction::IncomingData {
                peer_id,
                data,
                seen_limit,
                addr,
                ..
            } => {
                pubsub_state.reduce_incoming_data(&peer_id, data, meta.time())?;

                let dispatcher = state_context.into_dispatcher();

                dispatcher.push(P2pNetworkPubsubAction::ValidateIncomingMessages {
                    peer_id,
                    seen_limit,
                    addr,
                });
                // Control-only RPCs (notably IWANT) have no validation callback
                // to flush their replies. Do not leave them until the next block.
                dispatcher.push(P2pNetworkPubsubAction::OutgoingMessage { peer_id });

                Ok(())
            }
            P2pNetworkPubsubAction::ValidateIncomingMessages {
                peer_id,
                seen_limit,
                addr,
            } => {
                let Some(state) = pubsub_state.clients.get_mut(&peer_id) else {
                    // TODO: investigate, cannot reproduce this
                    // bug_condition!("{:?} not found in state.clients", peer_id);
                    return Ok(());
                };
                let messages = std::mem::take(&mut state.incoming_messages);

                let dispatcher = state_context.into_dispatcher();

                dispatcher.push(P2pNetworkPubsubEffectfulAction::ValidateIncomingMessages {
                    peer_id,
                    seen_limit,
                    addr,
                    messages,
                });

                Ok(())
            }
            P2pNetworkPubsubAction::IncomingMessage {
                peer_id,
                message,
                seen_limit,
            } => {
                // Check that if we can extract source from message, this is pre check
                if source_from_message(&message).is_err() {
                    let dispatcher = state_context.into_dispatcher();
                    dispatcher.push(P2pNetworkPubsubAction::RejectMessage {
                        message_id: None,
                        peer_id: Some(peer_id),
                        reason: "Invalid originator in message".to_owned(),
                    });
                    return Ok(());
                }

                // Check result later to ensure we always dispatch the cleanup action
                let reduce_incoming_result =
                    pubsub_state.reduce_incoming_message(&message, seen_limit);

                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let p2p_state: &P2pState = global_state.substate()?;
                let state: &Self = global_state.substate()?;

                dispatcher.push(P2pNetworkPubsubAction::IncomingMessageCleanup { peer_id });

                let message_content = reduce_incoming_result?;

                for (topic_id, map) in &state.topics {
                    let mesh_size = map.values().filter(|s| s.on_mesh()).count();
                    let could_accept = mesh_size < p2p_state.config.meshsub.outbound_degree_high;

                    if !could_accept {
                        if let Some(topic_state) = map.get(&peer_id) {
                            if topic_state.on_mesh() {
                                let topic_id = topic_id.clone();
                                dispatcher.push(P2pNetworkPubsubAction::Prune { peer_id, topic_id })
                            }
                        }
                    }
                }

                // This happens if message was already seen
                if let Some(message_content) = message_content {
                    dispatcher.push(P2pNetworkPubsubAction::HandleIncomingMessage {
                        message,
                        message_content,
                        peer_id,
                    });
                } else {
                    dispatcher.push(P2pNetworkPubsubAction::IgnoreMessage {
                        message_id: None,
                        reason: "Message already seen".to_owned(),
                    });
                };

                Ok(())
            }
            P2pNetworkPubsubAction::HandleIncomingMessage {
                message,
                message_content,
                peer_id,
            } => {
                let Ok(message_id) =
                    pubsub_state
                        .mcache
                        .put(message, message_content, peer_id, time)
                else {
                    bug_condition!("Unable to add message to `mcache`");
                    return Ok(());
                };

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let p2p_state: &P2pState = state.substate()?;

                if let Some(callback) = p2p_state.callbacks.on_p2p_pubsub_message_received.clone() {
                    dispatcher.push_callback(callback, message_id);
                } else {
                    dispatcher.push(P2pNetworkPubsubAction::ValidateIncomingMessage { message_id });
                }
                Ok(())
            }
            P2pNetworkPubsubAction::IncomingMessageCleanup { peer_id } => {
                pubsub_state.clear_incoming();

                let Some(client_state) = pubsub_state.clients.get_mut(&peer_id) else {
                    bug_condition!(
                        "State not found for action P2pNetworkPubsubAction::IncomingMessageCleanup"
                    );
                    return Ok(());
                };

                client_state.clear_incoming();

                Ok(())
            }
            // we want to add peer to our mesh
            P2pNetworkPubsubAction::Graft { peer_id, topic_id } => {
                let Some(state) = pubsub_state
                    .topics
                    .get_mut(&topic_id)
                    .and_then(|m| m.get_mut(&peer_id))
                else {
                    return Ok(());
                };
                state.mesh = P2pNetworkPubsubClientMeshAddingState::Added;

                if let Some(state) = pubsub_state.clients.get_mut(&peer_id) {
                    let control = state
                        .message
                        .control
                        .get_or_insert_with(|| pb::ControlMessage {
                            ihave: vec![],
                            iwant: vec![],
                            graft: vec![],
                            prune: vec![],
                        });
                    control.graft.push(pb::ControlGraft {
                        topic_id: Some(topic_id),
                    });
                }

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkPubsubAction::OutgoingMessage { peer_id });
                Ok(())
            }
            P2pNetworkPubsubAction::Prune { peer_id, topic_id } => {
                let Some(state) = pubsub_state
                    .topics
                    .get_mut(&topic_id)
                    .and_then(|m| m.get_mut(&peer_id))
                else {
                    bug_condition!("State not found for action: `P2pNetworkPubsubAction::Prune`");
                    return Ok(());
                };
                state.mesh = P2pNetworkPubsubClientMeshAddingState::WeRefused;

                if let Some(state) = pubsub_state.clients.get_mut(&peer_id) {
                    let control = state
                        .message
                        .control
                        .get_or_insert_with(|| pb::ControlMessage {
                            ihave: vec![],
                            iwant: vec![],
                            graft: vec![],
                            prune: vec![],
                        });
                    control.prune.push(pb::ControlPrune {
                        topic_id: Some(topic_id),
                        peers: vec![pb::PeerInfo {
                            peer_id: None,
                            signed_peer_record: None,
                        }],
                        backoff: None,
                    });
                }

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkPubsubAction::OutgoingMessage { peer_id });
                Ok(())
            }
            P2pNetworkPubsubAction::OutgoingMessage { peer_id } => {
                let msg = if let Some(v) = pubsub_state.clients.get_mut(&peer_id) {
                    &v.message
                } else {
                    bug_condition!(
                        "Invalid state for action: `P2pNetworkPubsubAction::OutgoingMessage`"
                    );
                    return Ok(());
                };

                let mut data = vec![];
                let result = prost::Message::encode_length_delimited(msg, &mut data)
                    .map(|_| data)
                    .map_err(|_| msg.clone());

                let dispatcher = state_context.into_dispatcher();

                match result {
                    Err(msg) => {
                        dispatcher
                            .push(P2pNetworkPubsubAction::OutgoingMessageError { msg, peer_id });
                    }
                    Ok(data) => {
                        dispatcher.push(P2pNetworkPubsubAction::OutgoingData {
                            data: Data::from(data),
                            peer_id,
                        });
                    }
                }

                // Important to avoid leaking state
                dispatcher.push(P2pNetworkPubsubAction::OutgoingMessageClear { peer_id });

                Ok(())
            }
            P2pNetworkPubsubAction::OutgoingMessageClear { peer_id } => {
                if let Some(v) = pubsub_state.clients.get_mut(&peer_id) {
                    v.iwant_replies.clear();
                    v.message = pb::Rpc {
                        subscriptions: vec![],
                        publish: vec![],
                        control: None,
                    };
                } else {
                    bug_condition!(
                        "Invalid state for action: `P2pNetworkPubsubAction::OutgoingMessageClear`"
                    );
                };
                Ok(())
            }
            P2pNetworkPubsubAction::OutgoingMessageError { .. } => Ok(()),
            P2pNetworkPubsubAction::WebRtcRebroadcast { message } => {
                let data = match super::encode_message(&message) {
                    Err(err) => {
                        bug_condition!("binprot serialization error: {err}");
                        return Ok(());
                    }
                    Ok(data) => data,
                };

                let mut source_sk = super::webrtc_source_sk_from_bytes(&data[8..]);
                let source_peer_id = source_sk.public_key().peer_id();
                let message_id = P2pNetworkPubsubMessageCacheId {
                    source: libp2p_identity::PeerId::try_from(source_peer_id).unwrap(),
                    seqno: 0,
                };
                let mut msg = pb::Message {
                    from: Some(message_id.source.to_bytes().to_vec()),
                    data: Some(data),
                    seqno: Some(message_id.seqno.to_be_bytes().to_vec()),
                    topic: super::TOPIC.to_owned(),
                    signature: None,
                    key: None,
                };

                msg.signature = match source_sk.libp2p_pubsub_pb_message_sign(&msg) {
                    Err(err) => {
                        bug_condition!("pubsub prost encode error: {err}");
                        return Ok(());
                    }
                    Ok(sig) => Some(sig.to_bytes().to_vec()),
                };

                let message_state = P2pNetworkPubsubMessageCacheMessage::Validated {
                    message: msg,
                    peer_id: source_peer_id,
                    time,
                };

                pubsub_state.mcache.map.insert(message_id, message_state);

                let dispatcher = state_context.into_dispatcher();

                dispatcher.push(P2pNetworkPubsubAction::BroadcastValidatedMessage {
                    message_id: super::BroadcastMessageId::MessageId { message_id },
                });

                Ok(())
            }
            P2pNetworkPubsubAction::Broadcast { message } => {
                let data = match super::encode_message(&message) {
                    Err(err) => {
                        bug_condition!("binprot serialization error: {err}");
                        return Ok(());
                    }
                    Ok(data) => data,
                };

                Self::prepare_to_sign(state_context, data)
            }
            P2pNetworkPubsubAction::Sign {
                seqno,
                author,
                data,
                topic,
            } => {
                pubsub_state.seq += 1;

                let libp2p_peer_id =
                    libp2p_identity::PeerId::try_from(author).expect("valid peer_id"); // This can't happen unless something is broken in the configuration
                pubsub_state.to_sign.push_back(pb::Message {
                    from: Some(libp2p_peer_id.to_bytes()),
                    data: Some(data.0.into_vec()),
                    seqno: Some(seqno.to_be_bytes().to_vec()),
                    topic: topic.clone(),
                    signature: None,
                    key: None,
                });

                let to_sign = pubsub_state.to_sign.front().cloned();
                let Some(message) = to_sign else {
                    bug_condition!("Message not found");
                    return Ok(());
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkPubsubEffectfulAction::Sign { author, message });
                Ok(())
            }
            P2pNetworkPubsubAction::SignError { .. } => {
                let _ = pubsub_state.to_sign.pop_front();
                Ok(())
            }
            P2pNetworkPubsubAction::BroadcastSigned { signature } => {
                pubsub_state.broadcast_signed(signature, time);

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                Self::broadcast(dispatcher, state)
            }
            P2pNetworkPubsubAction::OutgoingData { mut data, peer_id } => {
                let Some(client) = pubsub_state.clients.get(&peer_id) else {
                    bug_condition!(
                        "Missing state for action: `P2pNetworkPubsubAction::OutgoingData`"
                    );
                    return Ok(());
                };
                let Some(stream_id) = client.outgoing_stream_id else {
                    if let Ok(rpc) =
                        <pb::Rpc as prost::Message>::decode_length_delimited(data.as_ref())
                    {
                        for message in &rpc.publish {
                            if let Some((hash, slot)) =
                                super::block_gossip::BlockGossipState::message_identity(message)
                            {
                                mina_core::debug!(time; kind = "BlockGossipSuppressed",
                                    state_hash = hash.to_string(), slot = slot,
                                    reason = "no_outgoing_stream", phase = "transport",
                                    peer_id = display(peer_id));
                            }
                        }
                    }
                    return Ok(());
                };
                let addr = client.addr;
                // Signing, validation, IWANT, and stream setup can all queue a
                // publication. Recheck using send time at the transport boundary.
                let mut rpc = <pb::Rpc as prost::Message>::decode_length_delimited(data.as_ref())
                    .map_err(|err| format!("outgoing pubsub RPC: {err}"))?;
                pubsub_state.filter_outgoing_rpc(&mut rpc, peer_id, time);
                let (dispatcher, global) = state_context.into_dispatcher_and_state();
                let current: &Self = global.substate()?;
                let before = rpc.publish.len();
                super::filter_snark_gossip(&mut rpc, &current.mcache, |work| {
                    global.snark_work_is_eligible(work)
                });
                if rpc.publish.len() < before {
                    mina_core::debug!(time; kind = "SnarkPoolSendSuppressed", publications = before - rpc.publish.len());
                }
                for message in &rpc.publish {
                    if let Some((hash, slot)) =
                        super::block_gossip::BlockGossipState::message_identity(message)
                    {
                        mina_core::debug!(time; kind = "BlockGossipTransportHandoff",
                            state_hash = hash.to_string(), slot = slot,
                            peer_id = display(peer_id));
                    }
                }
                if rpc.publish.is_empty() && rpc.subscriptions.is_empty() && rpc.control.is_none() {
                    return Ok(());
                }
                data = prost::Message::encode_length_delimited_to_vec(&rpc).into();
                fuzz_maybe!(&mut data, crate::fuzzer::mutate_pubsub);
                let flags = fuzzed_maybe!(Default::default(), crate::fuzzer::mutate_yamux_flags);

                dispatcher.push(P2pNetworkYamuxAction::OutgoingData {
                    addr,
                    stream_id,
                    data,
                    flags,
                });
                Ok(())
            }
            P2pNetworkPubsubAction::ValidateIncomingMessage { message_id } => {
                let Some(message) = pubsub_state.mcache.map.remove(&message_id) else {
                    bug_condition!("Message with id: {:?} not found", message_id);
                    return Ok(());
                };

                let P2pNetworkPubsubMessageCacheMessage::Init {
                    message,
                    content,
                    time,
                    peer_id,
                } = message
                else {
                    bug_condition!(
                        "`P2pNetworkPubsubAction::ValidateIncomingMessage` called on invalid state"
                    );
                    return Ok(());
                };

                let new_message_state = match &content {
                    GossipNetMessageV2::NewState(block) => {
                        let block_hash = block.try_hash()?;
                        P2pNetworkPubsubMessageCacheMessage::PreValidatedBlockMessage {
                            block_hash,
                            message,
                            peer_id,
                            time,
                        }
                    }
                    GossipNetMessageV2::SnarkPoolDiff {
                        message: NetworkPoolSnarkPoolDiffVersionedStableV2::AddSolvedWork(snark),
                        ..
                    } => {
                        let statement = &snark.0;
                        let snark: Snark = snark.1.clone().into();
                        super::checked_snark_gossip(statement, &snark)?;
                        let job_id = snark.job_id();
                        P2pNetworkPubsubMessageCacheMessage::PreValidatedSnark {
                            job_id,
                            snark: Box::new(snark),
                            message,
                            peer_id,
                            time,
                        }
                    }
                    _ => P2pNetworkPubsubMessageCacheMessage::PreValidated {
                        message,
                        peer_id,
                        time,
                    },
                };
                pubsub_state
                    .mcache
                    .map
                    .insert(message_id, new_message_state);

                let dispatcher = state_context.into_dispatcher();

                // TODO: for transaction proof we track source, we should do that for `BestTipUpdate` and for `SnarkPoolDiff`
                match content {
                    GossipNetMessageV2::NewState(block) => {
                        let best_tip = BlockWithHash::try_new(block.clone())?;
                        dispatcher.push(P2pPeerAction::BestTipUpdate { peer_id, best_tip });
                    }
                    GossipNetMessageV2::TransactionPoolDiff { message, nonce } => {
                        // Received nonces are not used, so truncating is fine.
                        let nonce = nonce.as_u64() as u32;
                        dispatcher.push(P2pChannelsTransactionAction::Libp2pReceived {
                            peer_id,
                            transactions: message.0.into_iter().collect(),
                            nonce,
                            message_id,
                        });
                    }
                    GossipNetMessageV2::SnarkPoolDiff {
                        message: NetworkPoolSnarkPoolDiffVersionedStableV2::AddSolvedWork(work),
                        nonce,
                    } => {
                        let snark: Snark = work.1.into();
                        dispatcher.push(P2pChannelsSnarkAction::Libp2pReceived {
                            peer_id,
                            snark: Box::new(snark),
                            nonce: nonce.as_u64() as u32,
                        });
                    }
                    _ => {}
                }
                Ok(())
            }
            P2pNetworkPubsubAction::BroadcastValidatedMessage { message_id } => {
                let Some((message_id, _)) =
                    pubsub_state.mcache.get_message_id_and_message(&message_id)
                else {
                    bug_condition!("Message with id: {:?} not found", message_id);
                    return Ok(());
                };

                let Some(message) = pubsub_state.mcache.map.get(&message_id) else {
                    bug_condition!("Message with id: {:?} not found", message_id);
                    return Ok(());
                };

                let raw_message = message.message().clone();
                let peer_id = *message.peer_id();

                if !pubsub_state.block_gossip.announce(&raw_message, time) {
                    return Ok(());
                }
                pubsub_state.reduce_incoming_validated_message(message_id, peer_id, &raw_message);

                let Some(message) = pubsub_state.mcache.map.get_mut(&message_id) else {
                    bug_condition!("Message with id: {:?} not found", message_id);
                    return Ok(());
                };

                *message = P2pNetworkPubsubMessageCacheMessage::Validated {
                    message: raw_message,
                    peer_id,
                    time: *message.time(),
                };

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                Self::broadcast(dispatcher, state)
            }
            P2pNetworkPubsubAction::PruneMessages {} => {
                pubsub_state.block_gossip.prune(time);
                let messages = pubsub_state
                    .mcache
                    .map
                    .iter()
                    .filter_map(|(message_id, message)| {
                        if (*message.time() + MAX_MESSAGE_KEEP_DURATION) <= time {
                            Some(message_id.to_owned())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();

                for message_id in messages {
                    pubsub_state.mcache.remove_message(message_id);
                }
                Ok(())
            }
            P2pNetworkPubsubAction::RejectMessage {
                message_id,
                peer_id,
                ..
            } => {
                let mut involved_peers = peer_id.into_iter().collect::<Vec<_>>();
                let mut add_peer = |peer: &PeerId| {
                    if !involved_peers.contains(peer) {
                        involved_peers.push(*peer);
                    }
                };

                if let Some(message_id) = message_id {
                    let Some((message_id, message)) =
                        pubsub_state.mcache.get_message_id_and_message(&message_id)
                    else {
                        bug_condition!("Message not found for id: {:?}", message_id);
                        return Ok(());
                    };

                    add_peer(message.peer_id());
                    pubsub_state.mcache.remove_message(message_id);
                }

                let dispatcher = state_context.into_dispatcher();

                for peer_id in involved_peers {
                    dispatcher.push(P2pDisconnectionAction::Init {
                        peer_id,
                        reason: P2pDisconnectionReason::InvalidMessage,
                    });
                }

                Ok(())
            }
            P2pNetworkPubsubAction::IgnoreMessage { .. } => Ok(()),
        }
    }

    fn prepare_to_sign<Action, State>(
        mut state_context: Substate<Action, State, Self>,
        buffer: Vec<u8>,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let pubsub_state = state_context.get_substate_mut()?;

        let mut seqno = pubsub_state.seq;
        let (dispatcher, state) = state_context.into_dispatcher_and_state();
        let config: &P2pConfig = state.substate()?;
        seqno += config.meshsub.initial_time.as_nanos() as u64;

        dispatcher.push(P2pNetworkPubsubAction::Sign {
            seqno,
            author: config.identity_pub_key.peer_id(),
            data: buffer.into(),
            topic: super::TOPIC.to_owned(),
        });

        Ok(())
    }

    /// Queues a validated message for propagation to other peers in the pubsub network.
    /// For peers that are "on mesh" for the message's topic, queues the full message.
    /// For other peers, queues an IHAVE control message to notify about message availability.
    /// The original sender is excluded from propagation.
    fn reduce_incoming_validated_message(
        &mut self,
        message_id: P2pNetworkPubsubMessageCacheId,
        peer_id: PeerId,
        message: &Message,
    ) {
        let topic = self.topics.entry(message.topic.clone()).or_default();

        self.clients
            .iter_mut()
            .filter(|(c, _)| {
                // don't send back to who sent this
                *c != &peer_id
            })
            .for_each(|(c, state)| {
                let Some(topic_state) = topic.get(c) else {
                    return;
                };
                if topic_state.on_mesh() {
                    state.publish(message)
                } else {
                    let ctr = state.message.control.get_or_insert_with(Default::default);
                    ctr.ihave.push(pb::ControlIHave {
                        topic_id: Some(message.topic.clone()),
                        message_ids: vec![message_id.to_raw_bytes()],
                    })
                }
            });
    }

    /// Processes an incoming message by checking for duplicates and deserializing its contents.
    ///
    /// This function performs two main operations:
    /// 1. Deduplication: Tracks recently seen messages using their signatures to avoid processing duplicates
    /// 2. Deserialization: Converts valid message data into a `GossipNetMessageV2` structure
    ///
    /// # Arguments
    ///
    /// * `message` - The incoming message to process
    /// * `seen_limit` - Maximum number of message signatures to keep in the deduplication cache
    ///
    /// # Returns
    ///
    /// * `Ok(Some(GossipNetMessageV2))` - Successfully processed and deserialized message
    /// * `Ok(None)` - Message was a duplicate (already seen)
    /// * `Err(String)` - Error during processing (invalid message format or deserialization failure)
    ///
    #[inline(never)]
    fn reduce_incoming_message(
        &mut self,
        message: &Message,
        seen_limit: usize,
    ) -> Result<Option<GossipNetMessageV2>, String> {
        let Some(signature) = &message.signature else {
            bug_condition!("Validation failed: missing signature");
            return Ok(None);
        };

        // skip recently seen message
        if !self.seen.contains(signature) {
            self.seen.push_back(signature.clone());
            // keep only last `n` to avoid memory leak
            if self.seen.len() > seen_limit {
                self.seen.pop_front();
            }
        } else {
            return Ok(None);
        }

        // A message we can't decode exactly as OCaml does is dropped here, so
        // it is never validated or forwarded: OCaml bans the peer that
        // forwards it. See `gossip::decode_pubsub_data`.
        match &message.data {
            Some(data) => Ok(Some(gossip::decode_pubsub_data(data).map_err(|e| {
                format!("Invalid `GossipNetMessageV2` message, error: {e}")
            })?)),
            None => Err("Invalid message".to_owned()),
        }
    }

    /// Takes the complete length-delimited RPCs off the front of a peer's
    /// stream, which is what is left in `buffer` followed by `data`, and keeps
    /// the bytes after the last complete one in `buffer`.
    ///
    /// An RPC can arrive in any number of pieces. go-yamux, which OCaml's
    /// libp2p helper uses, sends at most 64 KiB in a frame, and a block is 0.1
    /// to 2 MB, so a block from an OCaml peer arrives in 2 to 35 frames. An RPC
    /// that doesn't decode is skipped. An invalid length, or one over
    /// `MAX_RPC_SIZE`, drops what is buffered, as the stream can't be framed
    /// from there.
    fn take_complete_rpcs(buffer: &mut Vec<u8>, data: &[u8]) -> Vec<pb::Rpc> {
        let buffered = !buffer.is_empty();
        if buffered {
            buffer.extend_from_slice(data);
        }
        let bytes = if buffered { buffer.as_slice() } else { data };

        let mut rpcs = vec![];
        let mut consumed = 0;
        while consumed < bytes.len() {
            let remaining = &bytes[consumed..];
            let mut rest = remaining;
            let len = match prost::decode_length_delimiter(&mut rest) {
                Ok(len) if len <= MAX_RPC_SIZE => len,
                // A varint takes at most 10 bytes: fewer may be its start.
                Err(_) if remaining.len() < 10 => break,
                _ => {
                    consumed = bytes.len();
                    break;
                }
            };
            let Some(rpc) = rest.get(..len) else {
                break;
            };
            if let Ok(rpc) = <pb::Rpc as prost::Message>::decode(rpc) {
                rpcs.push(rpc);
            }
            consumed = bytes.len() - rest.len() + len;
        }

        if buffered {
            buffer.drain(..consumed);
        } else {
            buffer.extend_from_slice(&data[consumed..]);
        }
        if buffer.is_empty() {
            // Don't hold on to a block-sized allocation for every peer.
            buffer.shrink_to(0x2000);
        }
        rpcs
    }

    /// Processes incoming data from a peer, handling subscriptions, control messages,
    /// and message broadcasting within the P2P pubsub system.
    fn reduce_incoming_data(
        &mut self,
        peer_id: &PeerId,
        data: Data,
        timestamp: Timestamp,
    ) -> Result<(), String> {
        let Some(client_state) = self.clients.get_mut(peer_id) else {
            // TODO: investigate, cannot reproduce this
            // bug_condition!("State not found for action: P2pNetworkPubsubAction::IncomingData");
            return Ok(());
        };

        // Data may complete a message we received part of before.
        for decoded in Self::take_complete_rpcs(&mut client_state.buffer, &data) {
            if let Some(client_state) = self.clients.get_mut(peer_id) {
                client_state.incoming_messages.extend(decoded.publish);
            }

            let subscriptions = decoded.subscriptions;
            let control = decoded.control.unwrap_or_default();

            self.update_subscriptions(peer_id, subscriptions);
            self.apply_control_commands(peer_id, &control);
            self.respond_to_iwant_requests(peer_id, &control.iwant, timestamp);
            self.process_ihave_messages(peer_id, control.ihave, timestamp);
        }

        Ok(())
    }

    fn update_subscriptions(&mut self, peer_id: &PeerId, subscriptions: Vec<pb::rpc::SubOpts>) {
        // Update subscription status based on incoming subscription requests.
        for subscription in &subscriptions {
            let topic_id = subscription.topic_id().to_owned();
            let topic = self.topics.entry(topic_id).or_default();

            if subscription.subscribe() {
                topic.entry(*peer_id).or_default();
            } else {
                topic.remove(peer_id);
            }
        }
    }

    /// Applies control commands (`graft` and `prune`) to manage the peer's mesh states within topics.
    fn apply_control_commands(&mut self, peer_id: &PeerId, control: &pb::ControlMessage) {
        // Apply graft commands to add the peer to specific topic meshes.
        for graft in &control.graft {
            if let Some(mesh_state) = self
                .topics
                .get_mut(graft.topic_id())
                .and_then(|m| m.get_mut(peer_id))
            {
                mesh_state.mesh = P2pNetworkPubsubClientMeshAddingState::Added;
            }
        }

        // Apply prune commands to remove the peer from specific topic meshes.
        for prune in &control.prune {
            if let Some(mesh_state) = self
                .topics
                .get_mut(prune.topic_id())
                .and_then(|m| m.get_mut(peer_id))
            {
                mesh_state.mesh = P2pNetworkPubsubClientMeshAddingState::TheyRefused;
            }
        }
    }

    fn respond_to_iwant_requests(
        &mut self,
        peer_id: &PeerId,
        iwant_requests: &[pb::ControlIWant],
        time: Timestamp,
    ) {
        // Respond to iwant requests by publishing available messages from the cache.
        for iwant in iwant_requests {
            for msg_id in &iwant.message_ids {
                if let Some(msg) = self.mcache.get_message_from_raw_message_id(msg_id) {
                    // Signature validation alone does not authorize forwarding a
                    // block, transaction or SNARK that is still being validated.
                    if !matches!(msg, P2pNetworkPubsubMessageCacheMessage::Validated { .. })
                        || !super::block_gossip::BlockGossipState::is_fresh(msg.message(), time)
                    {
                        continue;
                    }
                    if let Some(client) = self.clients.get_mut(peer_id) {
                        client.reply_to_iwant(msg.message());
                    }
                }
            }
        }
    }

    fn broadcast_signed(&mut self, signature: Data, time: Timestamp) {
        if let Some(mut message) = self.to_sign.pop_front() {
            message.signature = Some(signature.0.to_vec());
            if self.block_gossip.announce(&message, time) {
                let mut queued = 0;
                for client in self.clients.values_mut() {
                    let before = client.message.publish.len();
                    client.publish(&message);
                    queued += usize::from(client.message.publish.len() > before);
                }
                if queued == 0 {
                    if let Some((hash, slot)) =
                        super::block_gossip::BlockGossipState::message_identity(&message)
                    {
                        mina_core::debug!(time; kind = "BlockGossipSuppressed",
                            state_hash = hash.to_string(), slot = slot,
                            reason = "no_new_client_queue", phase = "signed");
                    }
                }
            }
        }
    }

    fn filter_outgoing_rpc(&mut self, rpc: &mut pb::Rpc, peer_id: PeerId, time: Timestamp) {
        self.block_gossip.prune(time);
        rpc.publish.retain(|message| {
            let requested = super::p2p_network_pubsub_state::compute_message_id(message)
                .ok()
                .is_some_and(|id| {
                    self.clients
                        .get_mut(&peer_id)
                        .is_some_and(|client| client.iwant_replies.remove(&id))
                });
            if requested {
                self.block_gossip.send_iwant(message, peer_id, time)
            } else {
                self.block_gossip.send(message, peer_id, time)
            }
        });
        if let Some(control) = &mut rpc.control {
            for ihave in &mut control.ihave {
                ihave.message_ids.retain(|id| {
                    self.mcache
                        .get_message_from_raw_message_id(id)
                        .is_some_and(|message| {
                            matches!(
                                message,
                                P2pNetworkPubsubMessageCacheMessage::Validated { .. }
                            ) && super::block_gossip::BlockGossipState::is_fresh(
                                message.message(),
                                time,
                            )
                        })
                });
            }
            control.ihave.retain(|ihave| !ihave.message_ids.is_empty());
            if control.ihave.is_empty()
                && control.iwant.is_empty()
                && control.graft.is_empty()
                && control.prune.is_empty()
            {
                rpc.control = None;
            }
        }
    }

    fn process_ihave_messages(
        &mut self,
        peer_id: &PeerId,
        ihave_messages: Vec<pb::ControlIHave>,
        timestamp: Timestamp,
    ) {
        // Process ihave messages by determining which available messages the client wants.
        for ihave in ihave_messages {
            if self.clients.contains_key(peer_id) {
                let message_ids = ihave
                    .message_ids
                    .into_iter()
                    .filter(|message_id| self.filter_iwant_message_ids(message_id, timestamp))
                    .collect::<Vec<_>>();

                if message_ids.is_empty() {
                    continue;
                }

                let Some(client) = self.clients.get_mut(peer_id) else {
                    bug_condition!("process_ihave_messages: State not found for {}", peer_id);
                    return;
                };

                // Queue the desired message IDs for the client to request.
                let ctr = client.message.control.get_or_insert_with(Default::default);
                ctr.iwant.push(pb::ControlIWant { message_ids })
            }
        }
    }

    fn broadcast<Action, State>(
        dispatcher: &mut Dispatcher<Action, State>,
        state: &State,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let state: &P2pNetworkPubsubState = state.substate()?;

        for peer_id in state
            .clients
            .iter()
            .filter(|(_, s)| !s.message_is_empty())
            .map(|(peer_id, _)| *peer_id)
        {
            dispatcher.push(P2pNetworkPubsubAction::OutgoingMessage { peer_id });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::{
            block_gossip::tests::{mesa_block, message, slot_start},
            p2p_network_pubsub_state::compute_message_id,
        },
        *,
    };
    use crate::{token::BroadcastAlgorithm, ConnectionAddr};
    use binprot::BinProtRead;

    fn cache_validated(
        state: &mut P2pNetworkPubsubState,
        message: pb::Message,
        time: Timestamp,
    ) -> P2pNetworkPubsubMessageCacheId {
        let id = compute_message_id(&message).unwrap();
        state.mcache.map.insert(
            id,
            P2pNetworkPubsubMessageCacheMessage::Validated {
                message,
                peer_id: PeerId::from_bytes([2; 32]),
                time,
            },
        );
        id
    }

    #[test]
    fn delayed_producer_signature_and_duplicate_injection_do_not_publish() {
        let block = mesa_block();
        let time = slot_start(&block);
        let msg = message(block.clone(), 1);
        let mut state = state_with_client();
        state.to_sign.push_back(msg.clone());
        state.broadcast_signed(vec![3; 64].into(), time + 90_000_000_000);
        assert!(state.to_sign.is_empty());
        assert!(state.clients[&peer()].message.publish.is_empty());
        state.to_sign.push_back(msg.clone());
        state.broadcast_signed(vec![3; 64].into(), time);
        assert_eq!(state.clients[&peer()].message.publish, vec![msg]);
        state.to_sign.push_back(message(block, 2));
        state.broadcast_signed(vec![3; 64].into(), time);
        assert_eq!(state.clients[&peer()].message.publish.len(), 1);
    }

    #[test]
    fn delayed_iwant_reply_is_filtered_at_send_and_fresh_reply_keeps_original_bytes() {
        let block = mesa_block();
        let time = slot_start(&block);
        let msg = message(block, 1);
        let mut state = state_with_client();
        let id = cache_validated(&mut state, msg.clone(), time);
        let request = pb::ControlIWant {
            message_ids: vec![id.to_raw_bytes()],
        };
        state.respond_to_iwant_requests(&peer(), std::slice::from_ref(&request), time);
        let mut rpc = std::mem::take(&mut state.clients.get_mut(&peer()).unwrap().message);
        assert_eq!(rpc.publish, vec![msg.clone()]);
        state.filter_outgoing_rpc(&mut rpc, peer(), time + 90_000_000_000);
        assert!(rpc.publish.is_empty());

        let mut state = state_with_client();
        cache_validated(&mut state, msg.clone(), time);
        state.respond_to_iwant_requests(&peer(), std::slice::from_ref(&request), time);
        let mut rpc = std::mem::take(&mut state.clients.get_mut(&peer()).unwrap().message);
        state.filter_outgoing_rpc(&mut rpc, peer(), time);
        assert_eq!(rpc.publish, vec![msg]);
        state.respond_to_iwant_requests(&peer(), &[request], time + 90_000_000_000);
        assert!(state.clients[&peer()].message.publish.is_empty());
    }

    fn take_reply(state: &mut P2pNetworkPubsubState, time: Timestamp) -> pb::Rpc {
        let mut rpc = std::mem::take(&mut state.clients.get_mut(&peer()).unwrap().message);
        state.filter_outgoing_rpc(&mut rpc, peer(), time);
        rpc
    }

    fn reset_or_reconnect(state: &mut P2pNetworkPubsubState, reconnect: bool) {
        if reconnect {
            state.prune_peer_state(&peer());
            assert!(!state.clients.contains_key(&peer()));
            state.clients = state_with_client().clients;
        } else {
            state.prune_stream_state(&peer(), 3, false);
            assert_eq!(state.clients[&peer()].outgoing_stream_id, None);
        }
        state.clients.get_mut(&peer()).unwrap().outgoing_stream_id = Some(5);
    }

    #[test]
    fn failed_iwant_send_can_retry_after_reset_or_disconnect_but_not_after_expiry() {
        let block = mesa_block();
        let time = slot_start(&block);
        let msg = message(block, 1);
        for reconnect in [false, true] {
            for expired in [false, true] {
                let mut state = state_with_client();
                state.clients.get_mut(&peer()).unwrap().outgoing_stream_id = Some(3);
                let id = cache_validated(&mut state, msg.clone(), time);
                let request = pb::ControlIWant {
                    message_ids: vec![id.to_raw_bytes()],
                };
                assert!(state.block_gossip.announce(&msg, time));
                state.respond_to_iwant_requests(&peer(), std::slice::from_ref(&request), time);
                let lost = take_reply(&mut state, time);
                assert_eq!(lost.publish, vec![msg.clone()]);
                // Model a transport failure: the first RPC never reaches the
                // receiving parser. Exercise the scheduler's actual cleanup.
                reset_or_reconnect(&mut state, reconnect);
                assert!(!state.block_gossip.announce(&msg, time));
                // Its stream send mark was cleared, independently of retries.
                assert!(state.block_gossip.send(&msg, peer(), time));
                assert!(!state.block_gossip.send(&msg, peer(), time));
                let retry_time = if expired {
                    time + 90_000_000_000
                } else {
                    time + 1_000_000
                };
                state.respond_to_iwant_requests(&peer(), &[request], retry_time);
                let retry = take_reply(&mut state, retry_time);
                let mut receiver = state_with_client();
                let bytes = prost::Message::encode_length_delimited_to_vec(&retry);
                receiver
                    .reduce_incoming_data(&peer(), bytes.into(), retry_time)
                    .unwrap();
                if expired {
                    assert!(receiver.clients[&peer()].incoming_messages.is_empty());
                } else {
                    assert_eq!(
                        receiver.clients[&peer()].incoming_messages,
                        vec![msg.clone()]
                    );
                }
            }
        }
    }

    #[test]
    fn explicit_iwant_is_bounded_across_ids_and_reconnects() {
        let block = mesa_block();
        let time = slot_start(&block);
        let msg = message(block.clone(), 1);
        let mut state = state_with_client();
        let id = cache_validated(&mut state, msg.clone(), time);
        let alternate = message(block, 2);
        let alternate_id = cache_validated(&mut state, alternate.clone(), time);
        // An unsolicited copy may have been lost too. It must not consume the
        // explicit request budget, nor may its client-cache entry block IWANT.
        assert!(state.block_gossip.announce(&msg, time));
        state.clients.get_mut(&peer()).unwrap().publish(&msg);
        assert_eq!(take_reply(&mut state, time).publish, vec![msg.clone()]);
        // go-libp2p-pubsub's bound: three explicit responses, then refusal.
        for attempt in 0..4 {
            let (id, expected) = if attempt % 2 == 0 {
                (id, &msg)
            } else {
                (alternate_id, &alternate)
            };
            let request = pb::ControlIWant {
                message_ids: vec![id.to_raw_bytes(); 2],
            };
            state.respond_to_iwant_requests(&peer(), &[request], time);
            let rpc = take_reply(&mut state, time);
            if attempt < 3 {
                assert_eq!(rpc.publish, vec![expected.clone()]);
            } else {
                assert!(rpc.publish.is_empty());
            }
            if attempt == 1 {
                reset_or_reconnect(&mut state, true);
            }
        }
        // Peer-local budget: another peer still has its own first response.
        assert!(state
            .block_gossip
            .send_iwant(&msg, PeerId::from_bytes([4; 32]), time));
    }

    #[test]
    fn ihave_with_no_missing_ids_does_not_queue_an_empty_iwant() {
        let block = mesa_block();
        let time = slot_start(&block);
        let mut state = state_with_client();
        let id = cache_validated(&mut state, message(block, 1), time);
        for ids in [vec![], vec![id.to_raw_bytes()]] {
            state.process_ihave_messages(
                &peer(),
                vec![pb::ControlIHave {
                    topic_id: Some(TOPIC.to_owned()),
                    message_ids: ids,
                }],
                time,
            );
            assert!(state.clients[&peer()].message_is_empty());
        }
        let unknown = b"missing".to_vec();
        state.process_ihave_messages(
            &peer(),
            vec![pb::ControlIHave {
                topic_id: Some(TOPIC.to_owned()),
                message_ids: vec![id.to_raw_bytes(), unknown.clone()],
            }],
            time,
        );
        let rpc = take_reply(&mut state, time);
        assert_eq!(
            rpc.control.unwrap().iwant,
            vec![pb::ControlIWant {
                message_ids: vec![unknown]
            }]
        );
    }

    #[test]
    fn iwant_never_bypasses_block_validation() {
        let block = mesa_block();
        let time = slot_start(&block);
        let msg = message(block.clone(), 1);
        let mut state = state_with_client();
        let id = state
            .mcache
            .put(
                msg.clone(),
                GossipNetMessageV2::NewState(block.clone()),
                peer(),
                time,
            )
            .unwrap();
        let request = pb::ControlIWant {
            message_ids: vec![id.to_raw_bytes()],
        };
        state.respond_to_iwant_requests(&peer(), std::slice::from_ref(&request), time);
        assert!(state.clients[&peer()].message.publish.is_empty());
        state.mcache.map.insert(
            id,
            P2pNetworkPubsubMessageCacheMessage::PreValidatedBlockMessage {
                block_hash: block.try_hash().unwrap(),
                message: msg,
                peer_id: peer(),
                time,
            },
        );
        state.respond_to_iwant_requests(&peer(), &[request], time);
        assert!(state.clients[&peer()].message.publish.is_empty());
    }

    #[test]
    fn frontier_forwarding_and_ihave_cannot_outlive_the_slot() {
        let block = mesa_block();
        let time = slot_start(&block);
        let msg = message(block.clone(), 1);
        let mut state = state_with_client();
        let id = cache_validated(&mut state, msg.clone(), time);
        state
            .topics
            .entry(TOPIC.to_owned())
            .or_default()
            .insert(peer(), Default::default());
        // Not on mesh: forward by IHAVE, followed by an IWANT request.
        assert!(state.block_gossip.announce(&msg, time));
        state.reduce_incoming_validated_message(id, PeerId::from_bytes([2; 32]), &msg);
        let mut rpc = std::mem::take(&mut state.clients.get_mut(&peer()).unwrap().message);
        assert_eq!(rpc.control.as_ref().unwrap().ihave.len(), 1);
        let mut fresh = rpc.clone();
        state.filter_outgoing_rpc(&mut fresh, peer(), time);
        assert_eq!(fresh, rpc);
        // A changed best tip, catchup completion, or re-encoded message for
        // this hash must not initiate another announcement, even while fresh.
        assert!(!state.block_gossip.announce(&message(block, 2), time));
        state.filter_outgoing_rpc(&mut rpc, peer(), time + 90_000_000_000);
        assert!(rpc.control.is_none());
        let mut stale = state_with_client();
        assert!(!stale.block_gossip.announce(&msg, time + 90_000_000_000));
    }

    /// What go-yamux puts in one data frame, as OCaml's libp2p helper uses it.
    const GO_YAMUX_FRAME: usize = 64 * 1024 - 12;

    fn peer() -> PeerId {
        PeerId::from_bytes([1; 32])
    }

    fn state_with_client() -> P2pNetworkPubsubState {
        let mut state = P2pNetworkPubsubState::default();
        state.clients.insert(
            peer(),
            P2pNetworkPubsubClientState {
                protocol: BroadcastAlgorithm::Meshsub1_1_0,
                addr: ConnectionAddr {
                    sock_addr: "127.0.0.1:8302".parse().unwrap(),
                    incoming: false,
                },
                outgoing_stream_id: None,
                message: pb::Rpc {
                    subscriptions: vec![],
                    publish: vec![],
                    control: None,
                },
                cache: Default::default(),
                iwant_replies: Default::default(),
                buffer: vec![],
                incoming_messages: vec![],
            },
        );
        state
    }

    /// A length-delimited RPC publishing one message of `size` bytes.
    fn rpc(size: usize, fill: u8) -> Vec<u8> {
        let message = pb::Message {
            from: Some(vec![0; 38]),
            data: Some(vec![fill; size]),
            seqno: Some(vec![0; 8]),
            topic: TOPIC.to_owned(),
            signature: Some(vec![fill; 64]),
            key: None,
        };
        let rpc = pb::Rpc {
            subscriptions: vec![],
            publish: vec![message],
            control: None,
        };
        let mut data = vec![];
        prost::Message::encode_length_delimited(&rpc, &mut data).unwrap();
        data
    }

    /// Feeds `data` in pieces of `piece` bytes, and returns the size of each
    /// message received and how many bytes are left buffered.
    fn receive(data: &[u8], piece: usize) -> (Vec<usize>, usize) {
        let mut state = state_with_client();
        for piece in data.chunks(piece) {
            state
                .reduce_incoming_data(&peer(), Data::from(piece.to_vec()), Timestamp::ZERO)
                .unwrap();
        }
        let client = &state.clients[&peer()];
        let sizes = client
            .incoming_messages
            .iter()
            .map(|m| m.data.as_ref().map_or(0, Vec::len))
            .collect();
        (sizes, client.buffer.len())
    }

    /// A block from an OCaml peer is 0.1 to 2 MB, in 64 KiB frames. Before,
    /// only a message in at most two pieces was received.
    #[test]
    fn a_block_sized_message_in_go_yamux_frames_is_received() {
        for size in [1_000, 100_000, 150_000, 650_000, 2_200_000] {
            assert_eq!(
                receive(&rpc(size, 7), GO_YAMUX_FRAME),
                (vec![size], 0),
                "{size} bytes"
            );
        }
        // One byte at a time, which splits the length prefix too.
        assert_eq!(receive(&rpc(3_000, 7), 1), (vec![3_000], 0));
    }

    #[test]
    fn several_messages_in_one_piece_are_all_received() {
        let mut data = rpc(10, 1);
        data.extend(rpc(200_000, 2));
        data.extend(rpc(30, 3));
        assert_eq!(receive(&data, data.len()), (vec![10, 200_000, 30], 0));
        // The start of a fourth stays buffered until the rest arrives.
        let fourth = rpc(500, 4);
        data.extend(&fourth[..100]);
        assert_eq!(receive(&data, GO_YAMUX_FRAME), (vec![10, 200_000, 30], 100));
    }

    #[test]
    fn a_length_over_the_limit_drops_the_buffer() {
        let mut data = vec![];
        prost::encode_length_delimiter(MAX_RPC_SIZE + 1, &mut data).unwrap();
        data.extend([0; 100]);
        assert_eq!(receive(&data, data.len()), (vec![], 0));

        // The next message is received again.
        let mut state = state_with_client();
        for piece in [data, rpc(20, 5)] {
            state
                .reduce_incoming_data(&peer(), Data::from(piece), Timestamp::ZERO)
                .unwrap();
        }
        assert_eq!(state.clients[&peer()].incoming_messages.len(), 1);
    }

    /// Block 564003, with its proof and the SNARK work it carries, as a Mesa
    /// mainnet node served it (`mina-rpc-capture`): 650 KB.
    fn mesa_best_tip() -> mina_p2p_messages::v2::MinaBlockBlockStableV2 {
        use mina_p2p_messages::{rpc::GetBestTipV2Response, rpc_kernel::ResponsePayload};

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../p2p-messages/tests/files/mesa/get_best_tip_v3.bin"
        );
        let bytes = std::fs::read(path).unwrap();
        let payload =
            ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut &bytes[..]).unwrap();
        payload.0.unwrap().0.unwrap().data
    }

    /// A real block, gossiped the way OCaml's libp2p helper sends it: one RPC
    /// in go-yamux frames. It is received whole, and decodes to the same block.
    #[test]
    fn a_mesa_block_in_go_yamux_frames_decodes() {
        let block = mesa_best_tip();
        let hash = block.try_hash().unwrap();
        let data =
            super::super::encode_message(&GossipNetMessageV2::NewState(block.into())).unwrap();
        let message = pb::Message {
            from: Some(vec![0; 38]),
            data: Some(data.clone()),
            seqno: Some(vec![0; 8]),
            topic: TOPIC.to_owned(),
            signature: Some(vec![9; 64]),
            key: None,
        };
        let rpc = pb::Rpc {
            subscriptions: vec![],
            publish: vec![message],
            control: None,
        };
        let mut bytes = vec![];
        prost::Message::encode_length_delimited(&rpc, &mut bytes).unwrap();
        assert!(bytes.len() > 9 * GO_YAMUX_FRAME, "{} bytes", bytes.len());

        let mut state = state_with_client();
        for frame in bytes.chunks(GO_YAMUX_FRAME) {
            state
                .reduce_incoming_data(&peer(), Data::from(frame.to_vec()), Timestamp::ZERO)
                .unwrap();
        }
        let client = state.clients.get_mut(&peer()).unwrap();
        assert!(client.buffer.is_empty());
        let received = std::mem::take(&mut client.incoming_messages);
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].data.as_ref(), Some(&data));

        let Ok(Some(GossipNetMessageV2::NewState(decoded))) =
            state.reduce_incoming_message(&received[0], 10)
        else {
            panic!("not a block");
        };
        assert_eq!(decoded.try_hash().unwrap(), hash);
    }

    /// Any split of a stream of RPCs, at any byte: every message arrives, in
    /// order, with its own bytes.
    #[test]
    fn rpcs_split_anywhere_are_all_received() {
        // xorshift64, so that a failure can be replayed.
        let mut seed = 0x1234_5678_9abc_def1_u64;
        let mut next = move |bound: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % bound as u64) as usize
        };
        for _ in 0..20 {
            let sizes: Vec<usize> = (0..12)
                .map(|i| match next(4) {
                    0 => next(10),
                    1 => next(2_000),
                    2 => next(140_000),
                    _ => 2 * GO_YAMUX_FRAME + next(300_000),
                } + i)
                .collect();
            let mut stream = vec![];
            for (i, &size) in sizes.iter().enumerate() {
                stream.extend(rpc(size, i as u8));
            }

            let mut state = state_with_client();
            let mut rest = &stream[..];
            while !rest.is_empty() {
                let piece = match next(3) {
                    0 => 1 + next(16),
                    1 => GO_YAMUX_FRAME,
                    _ => 1 + next(3 * GO_YAMUX_FRAME),
                }
                .min(rest.len());
                let (piece, after) = rest.split_at(piece);
                state
                    .reduce_incoming_data(&peer(), Data::from(piece.to_vec()), Timestamp::ZERO)
                    .unwrap();
                rest = after;
            }

            let client = &state.clients[&peer()];
            assert!(client.buffer.is_empty());
            let received: Vec<_> = client
                .incoming_messages
                .iter()
                .map(|m| m.data.clone().unwrap_or_default())
                .collect();
            assert_eq!(received.len(), sizes.len(), "sizes {sizes:?}");
            for (i, (data, &size)) in received.iter().zip(&sizes).enumerate() {
                assert_eq!(data.len(), size, "message {i} of {sizes:?}");
                assert!(data.iter().all(|&b| b == i as u8), "message {i}");
            }
        }
    }

    /// A complete RPC that is not a protobuf `Rpc` is skipped; the stream goes
    /// on from the next one.
    #[test]
    fn an_rpc_that_does_not_decode_is_skipped() {
        let mut data = vec![5, 0xff, 0xff, 0xff, 0xff, 0xff];
        data.extend(rpc(70_000, 6));
        assert_eq!(receive(&data, GO_YAMUX_FRAME), (vec![70_000], 0));
        assert_eq!(receive(&data, 1), (vec![70_000], 0));
    }

    /// The start of an RPC buffered from a stream that failed must not be
    /// prepended to the next stream's data: the reducer clears the buffer when
    /// a peer opens a new stream to us (`NewStream { incoming: true, .. }`).
    #[test]
    fn a_partial_rpc_from_a_failed_stream_is_dropped() {
        let first = rpc(100_000, 1);
        let second = rpc(500, 2);

        let mut state = state_with_client();
        state
            .reduce_incoming_data(
                &peer(),
                Data::from(first[..1_000].to_vec()),
                Timestamp::ZERO,
            )
            .unwrap();
        // Kept, the first RPC's start swallows the new stream's data.
        let mut kept = state.clone();
        kept.reduce_incoming_data(&peer(), Data::from(second.clone()), Timestamp::ZERO)
            .unwrap();
        assert!(kept.clients[&peer()].incoming_messages.is_empty());

        state.clients.get_mut(&peer()).unwrap().clear_buffer();
        state
            .reduce_incoming_data(&peer(), Data::from(second), Timestamp::ZERO)
            .unwrap();
        let client = &state.clients[&peer()];
        assert_eq!(client.incoming_messages.len(), 1);
        assert!(client.buffer.is_empty());
    }
}
