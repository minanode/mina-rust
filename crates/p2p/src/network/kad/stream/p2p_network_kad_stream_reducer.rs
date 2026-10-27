use mina_core::{bug_condition, fuzzed_maybe, warn, Substate, SubstateAccess};
use quick_protobuf::{serialize_into_vec, BytesReader, MessageWrite};
use redux::ActionWithMeta;

use crate::{
    token::{DiscoveryAlgorithm, StreamKind},
    ConnectionAddr, Data, Limit, P2pLimits, P2pNetworkKadRequestAction, P2pNetworkKadState,
    P2pNetworkKademliaRpcFromMessageError, P2pNetworkKademliaRpcReply,
    P2pNetworkKademliaRpcRequest, P2pNetworkSchedulerAction, P2pNetworkStreamProtobufError,
    P2pNetworkStreamResetReason, P2pNetworkYamuxAction, P2pState, PeerId, StreamId, YamuxFlags,
};

use super::{
    super::Message, P2pNetworkKadIncomingStreamState, P2pNetworkKadOutgoingStreamState,
    P2pNetworkKadStreamState, P2pNetworkKademliaStreamAction,
    P2pNetworkKademliaStreamWaitOutgoingCallback,
};

// The rust-libp2p version used by our libp2p test peers defaults to 16 KiB.
// OCaml's go-libp2p helper accepts 4 MiB. Send within the smaller bound.
// https://github.com/o1-labs/rust-libp2p/blob/e88cec09bd2554f4cc79ff90f200e4c864adf495/protocols/kad/src/protocol.rs#L46
const KADEMLIA_OUTGOING_REPLY_LIMIT: usize = 16 * 1024;
const MAX_INCOMING_KADEMLIA_STREAMS_PER_PEER: usize = 4;

/// Grow geometrically from bytes received, never from the untrusted length prefix.
fn append_fragment(buffer: &mut Vec<u8>, len: usize, fragment: &[u8]) -> bool {
    let remaining = len.saturating_sub(buffer.len());
    let added = fragment.len().min(remaining);
    let needed = buffer.len() + added;
    if needed > buffer.capacity() {
        let capacity = partial_capacity(needed, len);
        buffer.reserve_exact(capacity - buffer.len());
    }
    buffer.extend_from_slice(&fragment[..added]);
    buffer.len() == len
}

fn partial_capacity(needed: usize, len: usize) -> usize {
    needed.next_power_of_two().max(1024).min(len)
}

fn framed_payload(
    data: &[u8],
    limit: Limit<usize>,
) -> Result<(usize, &[u8]), P2pNetworkStreamProtobufError<P2pNetworkKademliaRpcFromMessageError>> {
    let mut reader = BytesReader::from_bytes(data);
    let len = reader
        .read_varint32(data)
        .map_err(|_| P2pNetworkStreamProtobufError::MessageLength)? as usize;
    if len > limit {
        return Err(P2pNetworkStreamProtobufError::Limit(len, limit));
    }
    let payload = data
        .len()
        .checked_sub(reader.len())
        .and_then(|offset| data.get(offset..))
        .ok_or_else(|| P2pNetworkStreamProtobufError::Message("out of bounds".to_owned()))?;
    Ok((len, payload))
}

fn incoming_error_reset_actions(
    addr: ConnectionAddr,
    peer_id: PeerId,
    stream_id: StreamId,
    error: &P2pNetworkStreamProtobufError<P2pNetworkKademliaRpcFromMessageError>,
) -> (P2pNetworkYamuxAction, P2pNetworkSchedulerAction) {
    (
        P2pNetworkYamuxAction::OutgoingData {
            addr,
            stream_id,
            data: Data::empty(),
            flags: YamuxFlags::RST,
        },
        P2pNetworkSchedulerAction::StreamReset {
            addr,
            peer_id,
            stream_id,
            protocol: Some(StreamKind::Discovery(DiscoveryAlgorithm::Kademlia1_0_0)),
            reason: P2pNetworkStreamResetReason::Kademlia(error.to_string()),
        },
    )
}

impl P2pNetworkKadIncomingStreamState {
    fn push_request_fragment(&mut self, fragment: &[u8]) -> Option<(usize, Vec<u8>)> {
        let Self::PartialRequestReceived { len, data, .. } = self else {
            return None;
        };
        let expected_len = *len;
        let mut buffer = std::mem::take(data);
        if append_fragment(&mut buffer, expected_len, fragment) {
            Some((expected_len, buffer))
        } else {
            *data = buffer;
            None
        }
    }

    pub fn reducer<State, Action>(
        mut state_context: Substate<Action, State, P2pNetworkKadState>,
        action: ActionWithMeta<P2pNetworkKademliaStreamAction>,
        limits: &P2pLimits,
    ) -> Result<(), String>
    where
        State: SubstateAccess<P2pNetworkKadState> + SubstateAccess<P2pState>,
        Action: crate::P2pActionTrait<State>,
    {
        let (action, meta) = action.split();
        let Some(P2pNetworkKadStreamState::Incoming(state)) = state_context
            .get_substate_mut()?
            .find_kad_stream_state_mut(action.peer_id(), action.stream_id())
        else {
            return Err("invalid stream".to_owned());
        };

        match (&mut *state, action) {
            (
                P2pNetworkKadIncomingStreamState::Default,
                P2pNetworkKademliaStreamAction::New { incoming, .. },
            ) if incoming => {
                *state = P2pNetworkKadIncomingStreamState::WaitingForRequest {
                    expect_close: false,
                };
                Ok(())
            }
            (
                P2pNetworkKadIncomingStreamState::WaitingForRequest { .. },
                P2pNetworkKademliaStreamAction::IncomingData {
                    data,
                    addr,
                    peer_id,
                    stream_id,
                },
            ) => {
                let data = &data.0;
                match framed_payload(data, limits.kademlia_request()) {
                    Ok((encoded_len, remaining_data)) => {
                        if encoded_len > remaining_data.len() {
                            let mut buffer = Vec::new();
                            append_fragment(&mut buffer, encoded_len, remaining_data);
                            *state = P2pNetworkKadIncomingStreamState::PartialRequestReceived {
                                len: encoded_len,
                                data: buffer,
                                since: meta.time(),
                                addr,
                            };
                            return Ok(());
                        }
                        state.handle_incoming_request(encoded_len, remaining_data)?;
                    }
                    Err(error) => *state = P2pNetworkKadIncomingStreamState::Error(error),
                };

                let state = state.clone();
                let dispatcher = state_context.into_dispatcher();

                match state {
                    P2pNetworkKadIncomingStreamState::RequestIsReady {
                        data: P2pNetworkKademliaRpcRequest::FindNode { key },
                    } => {
                        dispatcher.push(P2pNetworkKademliaStreamAction::WaitOutgoing {
                            addr,
                            peer_id,
                            stream_id,
                            callback: P2pNetworkKademliaStreamWaitOutgoingCallback::answer_find_node_request(key)
                        });
                    }
                    P2pNetworkKadIncomingStreamState::Error(error) => {
                        let (yamux, scheduler) =
                            incoming_error_reset_actions(addr, peer_id, stream_id, &error);
                        dispatcher.push(yamux);
                        dispatcher.push(scheduler);
                    }
                    _ => bug_condition!("Invalid state"),
                }

                Ok(())
            }
            (
                P2pNetworkKadIncomingStreamState::PartialRequestReceived { .. },
                P2pNetworkKademliaStreamAction::IncomingData {
                    data: new_data,
                    addr,
                    peer_id,
                    stream_id,
                },
            ) => {
                let Some((expected_len, data)) = state.push_request_fragment(&new_data.0) else {
                    return Ok(());
                };

                state.handle_incoming_request(expected_len, &data)?;
                let state = state.clone();
                let dispatcher = state_context.into_dispatcher();

                match state {
                    P2pNetworkKadIncomingStreamState::RequestIsReady {
                        data: P2pNetworkKademliaRpcRequest::FindNode { key },
                    } => {
                        dispatcher.push(P2pNetworkKademliaStreamAction::WaitOutgoing {
                            addr,
                            peer_id,
                            stream_id,
                            callback: P2pNetworkKademliaStreamWaitOutgoingCallback::answer_find_node_request(key)
                        });
                    }
                    P2pNetworkKadIncomingStreamState::Error(error) => {
                        warn!(meta.time(); summary = "error handling kademlia action", error = display(&error));
                        let (yamux, scheduler) =
                            incoming_error_reset_actions(addr, peer_id, stream_id, &error);
                        dispatcher.push(yamux);
                        dispatcher.push(scheduler);
                    }
                    _ => bug_condition!("Invalid state"),
                }

                Ok(())
            }
            (
                P2pNetworkKadIncomingStreamState::RequestIsReady { .. },
                P2pNetworkKademliaStreamAction::WaitOutgoing {
                    addr,
                    peer_id,
                    stream_id,
                    callback,
                },
            ) => {
                *state = P2pNetworkKadIncomingStreamState::WaitingForReply;
                let dispatcher = state_context.into_dispatcher();
                match callback {
                    P2pNetworkKademliaStreamWaitOutgoingCallback::AnswerFindNodeRequest {
                        callback,
                        args,
                    } => dispatcher.push_callback(callback, (addr, peer_id, stream_id, args)),
                    P2pNetworkKademliaStreamWaitOutgoingCallback::UpdateFindNodeRequest {
                        callback,
                        args,
                    } => dispatcher.push_callback(callback, (addr, peer_id, stream_id, args)),
                };
                Ok(())
            }
            (
                P2pNetworkKadIncomingStreamState::WaitingForReply,
                P2pNetworkKademliaStreamAction::SendResponse {
                    data,
                    peer_id,
                    addr,
                    stream_id,
                },
            ) => {
                let bytes =
                    serialize_response_bounded(data, Limit::Some(KADEMLIA_OUTGOING_REPLY_LIMIT))?;
                *state = P2pNetworkKadIncomingStreamState::ResponseBytesAreReady {
                    bytes: bytes.clone(),
                };

                let dispatcher = state_context.into_dispatcher();
                let data = fuzzed_maybe!(Data::from(bytes), crate::fuzzer::mutate_kad_data);
                let flags = fuzzed_maybe!(Default::default(), crate::fuzzer::mutate_yamux_flags);

                dispatcher.push(P2pNetworkYamuxAction::OutgoingData {
                    addr,
                    stream_id,
                    data,
                    flags,
                });
                dispatcher.push(P2pNetworkKademliaStreamAction::WaitIncoming {
                    addr,
                    peer_id,
                    stream_id,
                });
                Ok(())
            }
            (
                P2pNetworkKadIncomingStreamState::ResponseBytesAreReady { .. },
                P2pNetworkKademliaStreamAction::WaitIncoming { .. },
            ) => {
                *state = P2pNetworkKadIncomingStreamState::WaitingForRequest { expect_close: true };
                Ok(())
            }
            (
                P2pNetworkKadIncomingStreamState::WaitingForRequest { expect_close, .. },
                P2pNetworkKademliaStreamAction::RemoteClose {
                    addr,
                    peer_id,
                    stream_id,
                },
            ) if *expect_close => {
                *state = P2pNetworkKadIncomingStreamState::Closing;

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkYamuxAction::OutgoingData {
                    addr,
                    stream_id,
                    data: Data::empty(),
                    flags: YamuxFlags::FIN,
                });
                dispatcher.push(P2pNetworkKademliaStreamAction::Prune {
                    addr,
                    peer_id,
                    stream_id,
                });
                Ok(())
            }
            action => Err(format!(
                "incorrect kademlia incoming stream action {action:?}"
            )),
        }
    }

    fn handle_incoming_request(&mut self, len: usize, data: &[u8]) -> Result<(), String> {
        let mut reader = BytesReader::from_bytes(data);

        let message = match reader.read_message_by_len::<Message>(data, len) {
            Ok(v) => v,
            Err(e) => {
                *self = P2pNetworkKadIncomingStreamState::Error(
                    P2pNetworkStreamProtobufError::Message(e.to_string()),
                );
                return Ok(());
            }
        };

        let data = match P2pNetworkKademliaRpcRequest::try_from(message) {
            Ok(v) => v,
            Err(e) => {
                *self = P2pNetworkKadIncomingStreamState::Error(e.into());
                return Ok(());
            }
        };

        *self = P2pNetworkKadIncomingStreamState::RequestIsReady { data };
        Ok(())
    }
}

/// A FIND_NODE answer contains at most 20 peers, but learned peer entries can
/// carry many addresses. Trim the farthest entries to the remote packet limit.
/// Check the encoded size before allocating the output buffer; learned
/// addresses can make a reply very large.
fn serialize_response_bounded(
    mut reply: P2pNetworkKademliaRpcReply,
    limit: Limit<usize>,
) -> Result<Vec<u8>, String> {
    loop {
        let message = Message::try_from(&reply).map_err(|e| e.to_string())?;
        if quick_protobuf::sizeofs::sizeof_len(message.get_size()) <= limit {
            return serialize_into_vec(&message).map_err(|e| e.to_string());
        }
        let P2pNetworkKademliaRpcReply::FindNode { closer_peers } = &mut reply;
        if closer_peers.pop().is_none() {
            return Err(format!("empty Kademlia reply exceeds {limit} bytes"));
        }
    }
}

impl P2pNetworkKadOutgoingStreamState {
    fn push_reply_fragment(&mut self, fragment: &[u8]) -> Option<(usize, Vec<u8>)> {
        let Self::PartialReplyReceived { len, data, .. } = self else {
            return None;
        };
        let expected_len = *len;
        let mut buffer = std::mem::take(data);
        if append_fragment(&mut buffer, expected_len, fragment) {
            Some((expected_len, buffer))
        } else {
            *data = buffer;
            None
        }
    }

    pub fn reducer<State, Action>(
        mut state_context: Substate<Action, State, P2pNetworkKadState>,
        action: ActionWithMeta<P2pNetworkKademliaStreamAction>,
        limits: &P2pLimits,
    ) -> Result<(), String>
    where
        State: SubstateAccess<P2pNetworkKadState> + SubstateAccess<P2pState>,
        Action: crate::P2pActionTrait<State>,
    {
        let (action, meta) = action.split();
        let Some(P2pNetworkKadStreamState::Outgoing(state)) = state_context
            .get_substate_mut()?
            .find_kad_stream_state_mut(action.peer_id(), action.stream_id())
        else {
            bug_condition!("Stream not found");
            return Ok(());
        };

        match (&mut *state, action) {
            (
                P2pNetworkKadOutgoingStreamState::Default,
                P2pNetworkKademliaStreamAction::New { incoming, .. },
            ) if !incoming => {
                *state = P2pNetworkKadOutgoingStreamState::WaitingForRequest {
                    expect_close: false,
                };
                Ok(())
            }

            (
                P2pNetworkKadOutgoingStreamState::WaitingForRequest { .. },
                P2pNetworkKademliaStreamAction::SendRequest {
                    data,
                    addr,
                    stream_id,
                    peer_id,
                },
            ) => {
                let message = Message::from(&data);
                let bytes = serialize_into_vec(&message).map_err(|e| format!("{e}"))?;
                *state = P2pNetworkKadOutgoingStreamState::RequestBytesAreReady {
                    bytes: bytes.clone(),
                };

                let dispatcher = state_context.into_dispatcher();
                let data = fuzzed_maybe!(Data::from(bytes), crate::fuzzer::mutate_kad_data);
                let flags = fuzzed_maybe!(Default::default(), crate::fuzzer::mutate_yamux_flags);

                dispatcher.push(P2pNetworkYamuxAction::OutgoingData {
                    addr,
                    stream_id,
                    data,
                    flags,
                });
                dispatcher.push(P2pNetworkKademliaStreamAction::WaitIncoming {
                    addr,
                    peer_id,
                    stream_id,
                });
                dispatcher.push(P2pNetworkKadRequestAction::RequestSent { peer_id });
                Ok(())
            }
            (
                P2pNetworkKadOutgoingStreamState::RequestBytesAreReady { .. },
                P2pNetworkKademliaStreamAction::WaitIncoming { .. },
            ) => {
                *state = P2pNetworkKadOutgoingStreamState::WaitingForReply;
                Ok(())
            }

            (
                P2pNetworkKadOutgoingStreamState::WaitingForReply,
                P2pNetworkKademliaStreamAction::IncomingData {
                    data,
                    peer_id,
                    stream_id,
                    addr,
                },
            ) => {
                let data = &data.0;
                match framed_payload(data, limits.kademlia_response()) {
                    Ok((encoded_len, remaining_data)) => {
                        if encoded_len > remaining_data.len() {
                            let mut buffer = Vec::new();
                            append_fragment(&mut buffer, encoded_len, remaining_data);
                            *state = P2pNetworkKadOutgoingStreamState::PartialReplyReceived {
                                len: encoded_len,
                                data: buffer,
                                since: meta.time(),
                                addr,
                            };
                            return Ok(());
                        }
                        state.handle_incoming_response(encoded_len, remaining_data)?;
                    }
                    Err(error) => *state = P2pNetworkKadOutgoingStreamState::Error(error),
                };

                let state = state.clone();
                let dispatcher = state_context.into_dispatcher();

                match state {
                    P2pNetworkKadOutgoingStreamState::ResponseIsReady {
                        data:
                            P2pNetworkKademliaRpcReply::FindNode {
                                closer_peers: closest_peers,
                            },
                    } => {
                        dispatcher.push(P2pNetworkKademliaStreamAction::WaitOutgoing {
                            addr,
                            peer_id,
                            stream_id,
                            callback: P2pNetworkKademliaStreamWaitOutgoingCallback::update_find_node_request(closest_peers)
                        });
                    }
                    P2pNetworkKadOutgoingStreamState::Error(error) => {
                        warn!(meta.time(); summary = "error handling kademlia action", error = display(&error));
                        dispatcher.push(P2pNetworkYamuxAction::OutgoingData {
                            addr,
                            stream_id,
                            data: Data::empty(),
                            flags: YamuxFlags::RST,
                        });
                        dispatcher.push(P2pNetworkSchedulerAction::StreamReset {
                            addr,
                            peer_id,
                            stream_id,
                            protocol: Some(StreamKind::Discovery(
                                DiscoveryAlgorithm::Kademlia1_0_0,
                            )),
                            reason: P2pNetworkStreamResetReason::Kademlia(error.to_string()),
                        });
                    }
                    _ => {
                        bug_condition!("Invalid state");
                    }
                }

                Ok(())
            }
            (
                P2pNetworkKadOutgoingStreamState::PartialReplyReceived { .. },
                P2pNetworkKademliaStreamAction::IncomingData {
                    data: new_data,
                    stream_id,
                    peer_id,
                    addr,
                },
            ) => {
                let Some((expected_len, data)) = state.push_reply_fragment(&new_data) else {
                    return Ok(());
                };

                state.handle_incoming_response(expected_len, &data)?;
                let state = state.clone();
                let dispatcher = state_context.into_dispatcher();

                match state {
                    P2pNetworkKadOutgoingStreamState::ResponseIsReady {
                        data:
                            P2pNetworkKademliaRpcReply::FindNode {
                                closer_peers: closest_peers,
                            },
                    } => {
                        dispatcher.push(P2pNetworkKademliaStreamAction::WaitOutgoing {
                            addr,
                            peer_id,
                            stream_id,
                            callback: P2pNetworkKademliaStreamWaitOutgoingCallback::update_find_node_request(closest_peers)
                        });
                    }
                    P2pNetworkKadOutgoingStreamState::Error(error) => {
                        warn!(meta.time(); summary = "error handling kademlia action", error = display(&error));
                        dispatcher.push(P2pNetworkYamuxAction::OutgoingData {
                            addr,
                            stream_id,
                            data: Data::empty(),
                            flags: YamuxFlags::RST,
                        });
                        dispatcher.push(P2pNetworkSchedulerAction::StreamReset {
                            addr,
                            peer_id,
                            stream_id,
                            protocol: Some(StreamKind::Discovery(
                                DiscoveryAlgorithm::Kademlia1_0_0,
                            )),
                            reason: P2pNetworkStreamResetReason::Kademlia(error.to_string()),
                        });
                    }
                    _ => bug_condition!("Invalid state"),
                }

                Ok(())
            }
            (
                P2pNetworkKadOutgoingStreamState::ResponseIsReady { .. },
                P2pNetworkKademliaStreamAction::WaitOutgoing {
                    addr,
                    peer_id,
                    stream_id,
                    callback,
                },
            ) => {
                *state = P2pNetworkKadOutgoingStreamState::WaitingForRequest { expect_close: true };
                let dispatcher = state_context.into_dispatcher();
                match callback {
                    P2pNetworkKademliaStreamWaitOutgoingCallback::AnswerFindNodeRequest {
                        callback,
                        args,
                    } => dispatcher.push_callback(callback, (addr, peer_id, stream_id, args)),
                    P2pNetworkKademliaStreamWaitOutgoingCallback::UpdateFindNodeRequest {
                        callback,
                        args,
                    } => dispatcher.push_callback(callback, (addr, peer_id, stream_id, args)),
                };
                Ok(())
            }
            (
                P2pNetworkKadOutgoingStreamState::WaitingForRequest { expect_close },
                P2pNetworkKademliaStreamAction::Close {
                    addr,
                    peer_id,
                    stream_id,
                },
            ) if *expect_close => {
                *state =
                    P2pNetworkKadOutgoingStreamState::RequestBytesAreReady { bytes: Vec::new() };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkYamuxAction::OutgoingData {
                    addr,
                    stream_id,
                    data: Data::empty(),
                    flags: YamuxFlags::FIN,
                });
                dispatcher.push(P2pNetworkKademliaStreamAction::Prune {
                    addr,
                    peer_id,
                    stream_id,
                });
                Ok(())
            }
            (
                P2pNetworkKadOutgoingStreamState::Closing,
                P2pNetworkKademliaStreamAction::RemoteClose {
                    addr,
                    peer_id,
                    stream_id,
                },
            ) => {
                *state = P2pNetworkKadOutgoingStreamState::Closed;
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkKademliaStreamAction::Prune {
                    addr,
                    peer_id,
                    stream_id,
                });
                Ok(())
            }
            (state, action) => Err(format!(
                "kademlia outgoing stream state {state:?} is incorrect for action {action:?}",
            )),
        }
    }

    fn handle_incoming_response(&mut self, len: usize, data: &[u8]) -> Result<(), String> {
        let mut reader = BytesReader::from_bytes(data);

        let message = match reader.read_message_by_len::<Message>(data, len) {
            Ok(v) => v,
            Err(e) => {
                *self = P2pNetworkKadOutgoingStreamState::Error(
                    P2pNetworkStreamProtobufError::Message(e.to_string()),
                );
                return Ok(());
            }
        };

        let data = match P2pNetworkKademliaRpcReply::try_from(message) {
            Ok(v) => v,
            Err(e) => {
                *self = P2pNetworkKadOutgoingStreamState::Error(e.into());
                return Ok(());
            }
        };

        *self = P2pNetworkKadOutgoingStreamState::ResponseIsReady { data };
        Ok(())
    }
}

impl P2pNetworkKadStreamState {
    pub fn reducer<State, Action>(
        mut state_context: Substate<Action, State, P2pNetworkKadState>,
        action: ActionWithMeta<P2pNetworkKademliaStreamAction>,
        limits: &P2pLimits,
    ) -> Result<(), String>
    where
        State: SubstateAccess<P2pNetworkKadState> + SubstateAccess<P2pState>,
        Action: crate::P2pActionTrait<State>,
    {
        let state = state_context.get_substate_mut()?;
        let (action, meta) = action.split();
        if let P2pNetworkKademliaStreamAction::Timeout {
            addr,
            peer_id,
            stream_id,
        } = &action
        {
            let expired = state
                .expired_partial_frames(meta.time())
                .contains(&(*addr, *peer_id, *stream_id));
            if expired {
                let (dispatcher, _) = state_context.into_dispatcher_and_state();
                let error = P2pNetworkStreamProtobufError::Message(
                    "incomplete Kademlia frame timed out".to_owned(),
                );
                let (yamux, scheduler) =
                    incoming_error_reset_actions(*addr, *peer_id, *stream_id, &error);
                dispatcher.push(yamux);
                dispatcher.push(scheduler);
            }
            return Ok(());
        }
        if let P2pNetworkKademliaStreamAction::IncomingData {
            addr,
            peer_id,
            stream_id,
            data: fragment,
        } = &action
        {
            let growth = state
                .find_kad_stream_state(peer_id, stream_id)
                .and_then(|stream| {
                    let (len, received, capacity, fragment_len) = match stream {
                        P2pNetworkKadStreamState::Incoming(
                            P2pNetworkKadIncomingStreamState::PartialRequestReceived {
                                len,
                                data,
                                ..
                            },
                        )
                        | P2pNetworkKadStreamState::Outgoing(
                            P2pNetworkKadOutgoingStreamState::PartialReplyReceived {
                                len,
                                data,
                                ..
                            },
                        ) => (*len, data.len(), data.capacity(), fragment.0.len()),
                        P2pNetworkKadStreamState::Incoming(
                            P2pNetworkKadIncomingStreamState::WaitingForRequest { .. },
                        ) => {
                            let (len, body) =
                                framed_payload(&fragment.0, limits.kademlia_request()).ok()?;
                            (len, 0, 0, body.len())
                        }
                        P2pNetworkKadStreamState::Outgoing(
                            P2pNetworkKadOutgoingStreamState::WaitingForReply,
                        ) => {
                            let (len, body) =
                                framed_payload(&fragment.0, limits.kademlia_response()).ok()?;
                            (len, 0, 0, body.len())
                        }
                        _ => return None,
                    };
                    let needed = received + fragment_len.min(len.saturating_sub(received));
                    if needed == len || needed <= capacity || needed == 0 {
                        return None;
                    }
                    Some(partial_capacity(needed, len).saturating_sub(capacity))
                });
            if growth.is_some_and(|extra| !state.can_reserve_partial_frame(peer_id, extra)) {
                let (dispatcher, _) = state_context.into_dispatcher_and_state();
                let error = P2pNetworkStreamProtobufError::Message(
                    "Kademlia incomplete frame memory budget exceeded".to_owned(),
                );
                let (yamux, scheduler) =
                    incoming_error_reset_actions(*addr, *peer_id, *stream_id, &error);
                dispatcher.push(yamux);
                dispatcher.push(scheduler);
                return Ok(());
            }
        }
        if let P2pNetworkKademliaStreamAction::New {
            addr,
            peer_id,
            stream_id,
            incoming: true,
        } = &action
        {
            let incoming_count = state.incoming_streams_count(peer_id);
            if incoming_count >= MAX_INCOMING_KADEMLIA_STREAMS_PER_PEER {
                let (dispatcher, _) = state_context.into_dispatcher_and_state();
                dispatcher.push(P2pNetworkYamuxAction::OutgoingData {
                    addr: *addr,
                    stream_id: *stream_id,
                    data: Data::empty(),
                    flags: YamuxFlags::RST,
                });
                dispatcher.push(P2pNetworkSchedulerAction::StreamReset {
                    addr: *addr,
                    peer_id: *peer_id,
                    stream_id: *stream_id,
                    protocol: Some(StreamKind::Discovery(DiscoveryAlgorithm::Kademlia1_0_0)),
                    reason: P2pNetworkStreamResetReason::Kademlia(
                        "too many concurrent incoming Kademlia streams".to_owned(),
                    ),
                });
                return Ok(());
            }
        }
        let stream_state = match &action {
            P2pNetworkKademliaStreamAction::New {
                peer_id,
                stream_id,
                incoming,
                ..
            } => state
                .create_kad_stream_state(*incoming, peer_id, stream_id)
                .map_err(|stream| {
                    format!("kademlia stream already exists for action {action:?}: {stream:?}")
                })?,
            P2pNetworkKademliaStreamAction::Prune {
                peer_id, stream_id, ..
            } => {
                return state
                    .remove_kad_stream_state(peer_id, stream_id)
                    .then_some(())
                    .ok_or_else(|| format!("kademlia stream not found for action {action:?}"))
            }
            _ => state
                .find_kad_stream_state(action.peer_id(), action.stream_id())
                .ok_or_else(|| format!("kademlia stream not found for action {action:?}"))?,
        };

        match stream_state {
            P2pNetworkKadStreamState::Incoming(_) => P2pNetworkKadIncomingStreamState::reducer(
                state_context,
                meta.with_action(action),
                limits,
            ),
            P2pNetworkKadStreamState::Outgoing(_) => P2pNetworkKadOutgoingStreamState::reducer(
                state_context,
                meta.with_action(action),
                limits,
            ),
        }
    }
}

#[cfg(test)]
mod d3f_tests {
    use std::borrow::Cow;

    use super::*;
    use crate::network::kad::{mod_Message::MessageType, P2pNetworkKadEntry};

    #[test]
    fn max_size_message_fragments_grow_linearly_from_received_bytes() {
        let len = 1 << 22;
        let fragment = [0xa5; 1024];
        let addr = ConnectionAddr {
            sock_addr: "203.0.113.7:8302".parse().unwrap(),
            incoming: true,
        };
        let mut stream = P2pNetworkKadIncomingStreamState::PartialRequestReceived {
            len,
            data: Vec::new(),
            since: redux::Timestamp::ZERO,
            addr,
        };
        let mut bytes_moved_on_growth = 0;
        let mut old_capacity = 0;
        for index in 0..(len / fragment.len()) {
            let completed = stream.push_request_fragment(&fragment);
            if index + 1 == len / fragment.len() {
                let (_, buffer) = completed.expect("final fragment completes the frame");
                assert_eq!(buffer.len(), len);
            } else {
                assert!(completed.is_none());
                let P2pNetworkKadIncomingStreamState::PartialRequestReceived { data, .. } = &stream
                else {
                    unreachable!()
                };
                if data.capacity() > old_capacity {
                    bytes_moved_on_growth += data.len() - fragment.len();
                    old_capacity = data.capacity();
                }
                assert_eq!(data.len(), (index + 1) * fragment.len());
                assert!(data.capacity() <= (data.len() * 2).max(1024));
            }
        }
        assert!(bytes_moved_on_growth <= len * 2);
    }

    #[test]
    fn incomplete_frame_budget_is_global_and_per_peer() {
        let mut state = P2pNetworkKadState::default();
        let addr = ConnectionAddr {
            sock_addr: "203.0.113.7:8302".parse().unwrap(),
            incoming: true,
        };
        for n in 1..=16u8 {
            let peer = PeerId::from_bytes([n; 32]);
            assert!(state.can_reserve_partial_frame(&peer, 1 << 22));
            let stream = state.create_kad_stream_state(true, &peer, &1).unwrap();
            *stream = P2pNetworkKadStreamState::Incoming(
                P2pNetworkKadIncomingStreamState::PartialRequestReceived {
                    len: 1 << 22,
                    data: Vec::with_capacity(1 << 22),
                    since: redux::Timestamp::ZERO,
                    addr,
                },
            );
        }
        assert_eq!(state.partial_frame_bytes(None), 64 << 20);
        assert!(!state.can_reserve_partial_frame(&PeerId::from_bytes([17; 32]), 1));
        assert!(!state.can_reserve_partial_frame(&PeerId::from_bytes([1; 32]), 5 << 20));
    }

    #[test]
    fn stale_partial_frame_expires() {
        let mut state = P2pNetworkKadState::default();
        let peer = PeerId::from_bytes([1; 32]);
        let addr = ConnectionAddr {
            sock_addr: "203.0.113.7:8302".parse().unwrap(),
            incoming: true,
        };
        let stream = state.create_kad_stream_state(true, &peer, &1).unwrap();
        *stream = P2pNetworkKadStreamState::Incoming(
            P2pNetworkKadIncomingStreamState::PartialRequestReceived {
                len: 1 << 22,
                data: vec![1],
                since: redux::Timestamp::new(1_000_000_000_000),
                addr,
            },
        );
        assert!(state
            .expired_partial_frames(redux::Timestamp::new(1_029_000_000_000))
            .is_empty());
        assert_eq!(
            state.expired_partial_frames(redux::Timestamp::new(1_030_000_000_000)),
            vec![(addr, peer, 1)]
        );
    }

    #[test]
    fn oversized_frame_takes_stream_error_path_before_allocation() {
        // Varint-framed 4 MiB + 1, followed by no body.
        let frame = [0x81, 0x80, 0x80, 0x02];
        let result = framed_payload(&frame, Limit::Some(1 << 22));
        assert!(matches!(
            &result,
            Err(P2pNetworkStreamProtobufError::Limit(4_194_305, _))
        ));
        let addr = ConnectionAddr {
            sock_addr: "203.0.113.7:8302".parse().unwrap(),
            incoming: true,
        };
        let peer_id = PeerId::from_bytes([1; 32]);
        let (yamux, scheduler) =
            incoming_error_reset_actions(addr, peer_id, 1, &result.unwrap_err());
        assert!(matches!(
            yamux,
            P2pNetworkYamuxAction::OutgoingData {
                flags,
                stream_id: 1,
                ..
            } if flags.contains(YamuxFlags::RST)
        ));
        assert!(matches!(
            scheduler,
            P2pNetworkSchedulerAction::StreamReset {
                peer_id: id,
                stream_id: 1,
                reason: P2pNetworkStreamResetReason::Kademlia(_),
                ..
            } if id == peer_id
        ));
    }

    #[test]
    fn concurrent_incoming_stream_count_excludes_our_outgoing_stream() {
        let peer_id = crate::PeerId::from_bytes([1; 32]);
        let mut state = P2pNetworkKadState::default();
        state.create_kad_stream_state(false, &peer_id, &2).unwrap();
        for stream_id in [1, 3, 5, 7] {
            state
                .create_kad_stream_state(true, &peer_id, &stream_id)
                .unwrap();
        }
        assert_eq!(
            state.incoming_streams_count(&peer_id),
            MAX_INCOMING_KADEMLIA_STREAMS_PER_PEER
        );
    }

    #[test]
    fn ocaml_get_providers_request_is_bounded_and_rejected_at_stream_level() {
        let limits = P2pLimits::default();
        assert_eq!(limits.kademlia_request(), 1 << 22);
        assert_eq!(limits.kademlia_response(), 1 << 22);

        let message = Message {
            type_pb: MessageType::GET_PROVIDERS,
            key: Cow::Owned(vec![0; 108]),
            ..Default::default()
        };
        let bytes = quick_protobuf::serialize_into_vec(&message).unwrap();
        let mut reader = BytesReader::from_bytes(&bytes);
        let len = reader.read_varint32(&bytes).unwrap() as usize;
        let payload = &bytes[bytes.len() - reader.len()..];
        assert_eq!(len, payload.len());
        assert_eq!(len, 112);
        let mut stream = P2pNetworkKadIncomingStreamState::WaitingForRequest {
            expect_close: false,
        };
        stream.handle_incoming_request(len, payload).unwrap();
        assert!(matches!(
            stream,
            P2pNetworkKadIncomingStreamState::Error(P2pNetworkStreamProtobufError::Convert(_))
        ));
    }

    #[test]
    fn find_node_reply_is_trimmed_to_remote_message_limit() {
        let peer_id: crate::PeerId = "12D3KooWFbsukJEB9eNivRuv9oDDW1quzgPxkSKC6zj1NhH85z1t"
            .parse::<libp2p_identity::PeerId>()
            .unwrap()
            .try_into()
            .unwrap();
        let entry =
            P2pNetworkKadEntry::new(peer_id, vec!["/ip4/203.0.113.7/tcp/8302".parse().unwrap()])
                .unwrap();
        let reply = P2pNetworkKademliaRpcReply::FindNode {
            closer_peers: vec![entry],
        };
        let bytes = serialize_response_bounded(reply, Limit::Some(50)).unwrap();
        assert!(bytes.len() <= 50);
        let mut reader = BytesReader::from_bytes(&bytes);
        let len = reader.read_varint32(&bytes).unwrap() as usize;
        let payload = &bytes[bytes.len() - reader.len()..];
        let decoded = BytesReader::from_bytes(payload)
            .read_message_by_len::<Message>(payload, len)
            .unwrap();
        assert!(decoded.closerPeers.is_empty());
    }

    #[test]
    fn find_node_reply_fits_rust_libp2p_default_limit() {
        let peer_id: crate::PeerId = "12D3KooWFbsukJEB9eNivRuv9oDDW1quzgPxkSKC6zj1NhH85z1t"
            .parse::<libp2p_identity::PeerId>()
            .unwrap()
            .try_into()
            .unwrap();
        let addr = format!("/dns4/{}/tcp/8302", "a".repeat(50))
            .parse()
            .unwrap();
        let entry = P2pNetworkKadEntry::new(peer_id, vec![addr; 16]).unwrap();
        let reply = P2pNetworkKademliaRpcReply::FindNode {
            closer_peers: vec![entry; 20],
        };
        let bytes =
            serialize_response_bounded(reply, Limit::Some(KADEMLIA_OUTGOING_REPLY_LIMIT)).unwrap();
        assert!(bytes.len() <= KADEMLIA_OUTGOING_REPLY_LIMIT);
        let mut reader = BytesReader::from_bytes(&bytes);
        let len = reader.read_varint32(&bytes).unwrap() as usize;
        let payload = &bytes[bytes.len() - reader.len()..];
        let decoded = BytesReader::from_bytes(payload)
            .read_message_by_len::<Message>(payload, len)
            .unwrap();
        assert!(decoded.closerPeers.len() < 20);
    }
}
