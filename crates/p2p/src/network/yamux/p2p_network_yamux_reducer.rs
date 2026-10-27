use std::collections::{BTreeMap, VecDeque};

use mina_core::{bug_condition, fuzz_maybe, fuzzed_maybe, Substate, SubstateAccess};

use crate::{
    yamux::p2p_network_yamux_state::{YamuxFrame, YamuxFrameInner, YamuxFrameParseError},
    Data, Limit, P2pLimits, P2pNetworkAuthState, P2pNetworkConnectionError,
    P2pNetworkConnectionMuxState, P2pNetworkNoiseAction, P2pNetworkSchedulerAction,
    P2pNetworkSchedulerState, P2pNetworkSelectAction, P2pNetworkStreamResetReason,
    P2pNetworkStreamState, SelectKind, StreamId,
};

use super::{
    p2p_network_yamux_state::{YamuxStreamState, MAX_WINDOW_SIZE},
    P2pNetworkYamuxAction, P2pNetworkYamuxState, YamuxFlags, YamuxPing,
};

impl P2pNetworkYamuxState {
    /// Handles the main reducer logic for Yamux protocol actions. It processes incoming and outgoing
    /// data, selects appropriate behavior based on frame types, and manages the state of streams
    /// within a Yamux session.
    ///
    /// # High-Level Overview
    ///
    /// - When data arrives, it is appended to an internal buffer. The buffer is then parsed for
    ///   valid Yamux frames (using protocol-specific header fields and logic). Incomplete data
    ///   remains in the buffer for future parsing.
    /// - On successful parsing, frames are enqueued for further handling (e.g., dispatching
    ///   actions to notify higher-level protocols or responding to pings).
    /// - If protocol inconsistencies or invalid headers are encountered, it marks an error or
    ///   terminates gracefully, preventing further processing of unexpected data.
    /// - Outgoing data is prepared as frames that respect the window constraints and established
    ///   flags (e.g., SYN, ACK, FIN), and they are dispatched for transmission.
    /// - Once frames are processed, the function checks if the buffer has grown beyond a certain
    ///   threshold relative to its initial capacity. If so, and if the remaining data is small,
    ///   it resets the buffer capacity to a default size to avoid excessive memory usage.
    /// - The function also manages streams and their states, ensuring that proper handshake
    ///   flags are set (SYN, ACK) when a new stream is opened or accepted, enforcing limits on
    ///   the number of streams, and notifying higher-level components about events like
    ///   incoming data or connection errors.
    pub fn reducer<State, Action>(
        // Substate is accessed
        mut state_context: Substate<Action, State, P2pNetworkSchedulerState>,
        action: redux::ActionWithMeta<P2pNetworkYamuxAction>,
        limits: &P2pLimits,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let (action, meta) = action.split();
        let queue_before = if let P2pNetworkYamuxAction::OutgoingFrame { addr, frame } = &action {
            let scheduler = state_context.get_substate()?;
            let peer = scheduler
                .connection_state(addr)
                .and_then(|c| c.peer_id())
                .copied();
            let (queued, peer_queued) = peer
                .as_ref()
                .map(|peer| scheduler.queued_outgoing_bytes(peer))
                .unwrap_or((0, 0));
            let (reserved, peer_reserved) = peer
                .as_ref()
                .map(|peer| scheduler.reserved_rpc_response_bytes(peer))
                .unwrap_or((0, 0));
            let total = queued.saturating_add(reserved);
            let peer_total = peer_queued.saturating_add(peer_reserved);
            let stream = scheduler
                .connection_state(addr)
                .and_then(|c| c.mux.as_ref())
                .and_then(|mux| match mux {
                    P2pNetworkConnectionMuxState::Yamux(yamux) => {
                        yamux.streams.get(&frame.stream_id)
                    }
                    _ => None,
                })
                .map_or(0, YamuxStreamState::pending_len);
            Some((peer, total, peer_total, peer_queued, stream))
        } else {
            None
        };
        let connection_state = state_context
            .get_substate_mut()?
            .connection_state_mut(action.addr())
            .ok_or_else(|| format!("Connection not found for action: {action:?}"))
            .inspect_err(|e| bug_condition!("{}", e))?;

        let P2pNetworkConnectionMuxState::Yamux(yamux_state) = connection_state
            .mux
            .as_mut()
            .ok_or_else(|| format!("Invalid yamux state for action: {action:?}"))?;

        if yamux_state.terminated.is_some() {
            return Ok(());
        }

        match action {
            P2pNetworkYamuxAction::IncomingData { data, addr } => {
                yamux_state.extend_buffer(&data);
                yamux_state.parse_frames();

                let frame_count = yamux_state.incoming_frame_count();
                let window_exceeded = yamux_state.window_exceeded();
                let dispatcher = state_context.into_dispatcher();
                if let Some(stream_id) = window_exceeded {
                    dispatcher.push(P2pNetworkSchedulerAction::Error {
                        addr,
                        error: P2pNetworkConnectionError::YamuxOverflow(stream_id),
                    });
                    return Ok(());
                }

                for _ in 0..frame_count {
                    dispatcher.push(P2pNetworkYamuxAction::IncomingFrame { addr })
                }

                Ok(())
            }
            P2pNetworkYamuxAction::OutgoingData {
                addr,
                stream_id,
                data,
                mut flags,
            } => {
                let stream_state = yamux_state
                    .streams
                    .get(&stream_id)
                    .ok_or_else(|| format!("Stream with id {stream_id} not found for `P2pNetworkYamuxAction::OutgoingData`"))?;

                if !stream_state.incoming && !stream_state.established && !stream_state.syn_sent {
                    flags.insert(YamuxFlags::SYN);
                } else if stream_state.incoming && !stream_state.established {
                    flags.insert(YamuxFlags::ACK);
                }

                fuzz_maybe!(&mut flags, crate::fuzzer::mutate_yamux_flags);

                let frame = YamuxFrame {
                    flags,
                    stream_id,
                    inner: YamuxFrameInner::Data(data),
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkYamuxAction::OutgoingFrame { addr, frame });

                Ok(())
            }
            P2pNetworkYamuxAction::IncomingFrame { addr } => {
                let mut pending_outgoing = VecDeque::default();
                let Some(frame) = yamux_state.incoming.pop_front() else {
                    bug_condition!(
                        "Frame not found for action `P2pNetworkYamuxAction::IncomingFrame`"
                    );
                    return Ok(());
                };

                // Frames already in flight may arrive after stream cleanup.
                // Only SYN opens a stream; a late ACK/window update must not
                // recreate a retired stream (go-yamux session.handleStreamMessage).
                if frame.stream_id != 0
                    && !frame.flags.contains(YamuxFlags::SYN)
                    && !yamux_state.streams.contains_key(&frame.stream_id)
                {
                    return Ok(());
                }

                if frame.flags.contains(YamuxFlags::SYN) {
                    yamux_state
                        .streams
                        .insert(frame.stream_id, YamuxStreamState::incoming());

                    if frame.stream_id != 0 {
                        connection_state.streams.insert(
                            frame.stream_id,
                            P2pNetworkStreamState::new_incoming(meta.time()),
                        );
                    }
                }
                if frame.flags.contains(YamuxFlags::ACK) {
                    yamux_state
                        .streams
                        .entry(frame.stream_id)
                        .or_default()
                        .established = true;
                }

                match &frame.inner {
                    YamuxFrameInner::Data(_) => {
                        if let Some(stream) = yamux_state.streams.get_mut(&frame.stream_id) {
                            // A peer that sends more than the window allows
                            // breaks flow control (go-yamux closes the session
                            // with a protocol error).
                            match stream.window_ours.checked_sub(frame.len_as_u32()) {
                                Some(window) => stream.window_ours = window,
                                None => {
                                    yamux_state.set_err(YamuxFrameParseError::WindowExceeded(
                                        frame.stream_id,
                                    ));
                                    state_context.into_dispatcher().push(
                                        P2pNetworkSchedulerAction::Error {
                                            addr,
                                            error: P2pNetworkConnectionError::YamuxOverflow(
                                                frame.stream_id,
                                            ),
                                        },
                                    );
                                    return Ok(());
                                }
                            }
                        }
                    }
                    YamuxFrameInner::WindowUpdate { difference } => {
                        let stream = yamux_state
                            .streams
                            .entry(frame.stream_id)
                            .or_insert_with(YamuxStreamState::incoming);

                        stream.window_theirs = stream.window_theirs.saturating_add(*difference);

                        if *difference > 0 {
                            // Offer every waiting frame again, oldest first:
                            // `OutgoingFrame` sends what the window now has
                            // room for and queues the rest in order.
                            if !stream.pending.is_empty() {
                                stream.pending_since = Some(meta.time());
                            }
                            pending_outgoing = stream.take_pending();
                        }
                    }
                    YamuxFrameInner::Ping { .. } => {}
                    YamuxFrameInner::GoAway(res) => yamux_state.set_res(*res),
                }

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let limits: &P2pLimits = state.substate()?;
                let connection_state =
                    <State as SubstateAccess<P2pNetworkSchedulerState>>::substate(state)?
                        .connection_state(&addr)
                        .ok_or_else(|| format!("Connection not found {}", addr))?;

                let yamux = connection_state.yamux_state().ok_or_else(|| {
                    format!(
                        "Stream with id {} not found for `P2pNetworkYamuxAction::IncomingFrame`",
                        frame.stream_id
                    )
                })?;
                let stream = yamux.streams.get(&frame.stream_id).ok_or_else(|| {
                    format!(
                        "Stream with id {} not found for `P2pNetworkYamuxAction::IncomingFrame`",
                        frame.stream_id
                    )
                })?;

                let peer_id = match connection_state
                    .auth
                    .as_ref()
                    .and_then(|P2pNetworkAuthState::Noise(noise)| noise.peer_id())
                {
                    Some(peer_id) => *peer_id,
                    None => return Ok(()),
                };

                // OCaml opens one stream per RPC query. During fresh ledger
                // bootstrap, several remain live until the RPC timeout; D4
                // observed the old 10-stream limit reset a ledger query.
                // The public default admits OCaml's 100 parallel ledger
                // queries with headroom and remains bounded per connection.
                // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/gossip_net/libp2p.ml#L894-L910>
                let max_streams = limits.max_streams();

                // In go-libp2p, which OCaml nodes run, a reset ends one stream:
                // its host resets a stream that hasn't negotiated in time, the
                // Kademlia DHT one whose answer is late, OCaml one per finished
                // RPC connection. Close only that stream, unless the connection
                // can't work without it.
                if frame.flags.contains(YamuxFlags::RST) {
                    let stream_id = frame.stream_id;
                    if connection_state.is_essential_stream(&addr, stream_id) {
                        dispatcher.push(P2pNetworkSchedulerAction::Error {
                            addr,
                            error: P2pNetworkConnectionError::StreamReset(stream_id),
                        });
                    } else {
                        dispatcher.push(P2pNetworkSchedulerAction::StreamReset {
                            addr,
                            peer_id,
                            stream_id,
                            protocol: connection_state.stream_protocol(stream_id),
                            reason: P2pNetworkStreamResetReason::RemoteReset,
                        });
                    }
                    return Ok(());
                }

                if frame.flags.contains(YamuxFlags::SYN) && frame.stream_id != 0 {
                    // The SYN stream was inserted above, so count it too.
                    if incoming_stream_limit_exceeded(&connection_state.streams, max_streams) {
                        dispatcher.push(P2pNetworkYamuxAction::OutgoingFrame {
                            addr,
                            frame: YamuxFrame {
                                flags: YamuxFlags::RST,
                                stream_id: frame.stream_id,
                                inner: YamuxFrameInner::Data(vec![].into()),
                            },
                        });
                        dispatcher.push(P2pNetworkSchedulerAction::StreamReset {
                            addr,
                            peer_id,
                            stream_id: frame.stream_id,
                            protocol: None,
                            reason: P2pNetworkStreamResetReason::StreamLimit,
                        });
                        return Ok(());
                    } else {
                        dispatcher.push(P2pNetworkSelectAction::Init {
                            addr,
                            kind: SelectKind::Stream(peer_id, frame.stream_id),
                            incoming: true,
                        });
                    }
                }
                match &frame.inner {
                    YamuxFrameInner::Data(data) => {
                        // when our window size is less than half of the max window size send window update
                        let difference = yamux.receive_window_grant(
                            frame.stream_id,
                            stream.max_window_size.saturating_mul(2).min(1024 * 1024),
                        );
                        if stream.window_ours < stream.max_window_size / 2 && difference > 0 {
                            dispatcher.push(P2pNetworkYamuxAction::OutgoingFrame {
                                addr,
                                frame: YamuxFrame {
                                    stream_id: frame.stream_id,
                                    flags: YamuxFlags::empty(),
                                    inner: YamuxFrameInner::WindowUpdate { difference },
                                },
                            });
                        }

                        dispatcher.push(P2pNetworkSelectAction::IncomingData {
                            addr,
                            peer_id,
                            stream_id: frame.stream_id,
                            data: data.clone(),
                            fin: frame.flags.contains(YamuxFlags::FIN),
                        });
                    }
                    YamuxFrameInner::Ping { opaque } => {
                        let response = frame.flags.contains(YamuxFlags::ACK);
                        // if this ping is not response create our response
                        if !response {
                            let ping = YamuxPing {
                                stream_id: frame.stream_id,
                                opaque: *opaque,
                                response: true,
                            };
                            dispatcher.push(P2pNetworkYamuxAction::OutgoingFrame {
                                addr,
                                frame: ping.into_frame(),
                            });
                        }
                    }
                    YamuxFrameInner::WindowUpdate { .. } => {
                        while let Some(frame) = pending_outgoing.pop_front() {
                            dispatcher.push(P2pNetworkYamuxAction::OutgoingFrame { addr, frame });
                        }
                        // OCaml's pinned go-yamux v4.0.0 sends FIN on a
                        // WindowUpdate, not just on Data (stream.go, sendClose).
                        // Both frame types must deliver EOF to the protocol.
                        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/libp2p_helper/src/go.mod#L94>
                        if frame.flags.contains(YamuxFlags::FIN) {
                            dispatcher.push(P2pNetworkSelectAction::IncomingData {
                                addr,
                                peer_id,
                                stream_id: frame.stream_id,
                                data: Data::default(),
                                fin: true,
                            });
                        }
                    }
                    _ => {}
                }

                Ok(())
            }
            P2pNetworkYamuxAction::OutgoingFrame { mut frame, addr } => {
                let stream_id = frame.stream_id;
                // Granted when sent, against the windows as they are then.
                if let YamuxFrameInner::WindowUpdate { difference } = &mut frame.inner {
                    *difference = yamux_state.receive_window_grant(stream_id, *difference);
                }
                let Some(stream) = yamux_state.streams.get_mut(&stream_id) else {
                    return Ok(());
                };
                if matches!(frame.inner, YamuxFrameInner::Data(_)) {
                    // Send what the peer's window has room for; the rest waits
                    // for a window update, behind what is already waiting.
                    let sendable = stream.take_sendable(frame);

                    let pending = stream.pending_len();
                    let (peer_id, total, peer_total, peer_queued, previous) =
                        queue_before.expect("outgoing frame queue was measured before mutating it");
                    let added = pending.saturating_sub(previous);
                    if pending > yamux_state.pending_outgoing_limit
                        || (added > 0
                            && !limits.outgoing_queue_budget_allows(
                                total,
                                peer_total,
                                peer_queued,
                                added,
                            ))
                    {
                        // The queued frame is discarded with this stream. A slow
                        // reader must not disconnect the whole peer or pin RAM.
                        stream.pending.clear();
                        stream.pending_since = None;
                        stream.pending_first_at = None;
                        let dispatcher = state_context.into_dispatcher();
                        dispatcher.push(P2pNetworkYamuxAction::OutgoingData {
                            addr,
                            stream_id,
                            data: Data::default(),
                            flags: YamuxFlags::RST,
                        });
                        if let Some(peer_id) = peer_id {
                            dispatcher.push(P2pNetworkSchedulerAction::StreamReset {
                                addr,
                                peer_id,
                                stream_id,
                                protocol: None,
                                reason: P2pNetworkStreamResetReason::OutgoingQueueLimit,
                            });
                        }
                        return Ok(());
                    }
                    if pending > 0 {
                        stream.pending_since.get_or_insert(meta.time());
                        stream.pending_first_at.get_or_insert(meta.time());
                    } else {
                        stream.pending_since = None;
                        stream.pending_first_at = None;
                    }
                    let Some(sendable) = sendable else {
                        return Ok(());
                    };
                    frame = sendable;
                }
                match &mut frame.inner {
                    YamuxFrameInner::Data(_) => {}
                    YamuxFrameInner::WindowUpdate { difference } => {
                        stream.window_ours = stream.window_ours.saturating_add(*difference);
                        if stream.window_ours > stream.max_window_size {
                            stream.max_window_size = stream.window_ours.min(MAX_WINDOW_SIZE);
                        }
                    }
                    _ => {}
                }

                if frame.flags.contains(YamuxFlags::FIN) {
                    connection_state.streams.remove(&frame.stream_id);
                    stream.writable = false;
                } else {
                    if frame.flags.contains(YamuxFlags::ACK) {
                        stream.established |= true;
                    }
                    if frame.flags.contains(YamuxFlags::SYN) {
                        stream.syn_sent = true;
                    }
                }

                let dispatcher = state_context.into_dispatcher();
                let data = fuzzed_maybe!(
                    Data::from(frame.into_bytes()),
                    crate::fuzzer::mutate_yamux_frame
                );
                dispatcher.push(P2pNetworkNoiseAction::OutgoingData { addr, data });
                Ok(())
            }
            P2pNetworkYamuxAction::PingStream { addr, ping } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkYamuxAction::OutgoingFrame {
                    addr,
                    frame: ping.into_frame(),
                });

                Ok(())
            }
            P2pNetworkYamuxAction::OpenStream {
                stream_id,
                stream_kind,
                addr,
            } => {
                yamux_state
                    .streams
                    .insert(stream_id, YamuxStreamState::default());
                connection_state.streams.insert(
                    stream_id,
                    P2pNetworkStreamState::new(stream_kind, meta.time()),
                );

                let peer_id = match connection_state
                    .auth
                    .as_ref()
                    .and_then(|P2pNetworkAuthState::Noise(noise)| noise.peer_id())
                {
                    Some(peer_id) => *peer_id,
                    None => return Ok(()),
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkSelectAction::Init {
                    addr,
                    kind: SelectKind::Stream(peer_id, stream_id),
                    incoming: false,
                });
                Ok(())
            }
        }
    }
}

fn incoming_stream_limit_exceeded(
    streams: &BTreeMap<StreamId, P2pNetworkStreamState>,
    max_streams: Limit<usize>,
) -> bool {
    let Limit::Some(limit) = max_streams else {
        return false;
    };
    streams
        .values()
        .filter(|stream| stream.select.is_incoming())
        .count()
        > limit
}

#[cfg(test)]
mod d4f_tests {
    use super::*;

    #[test]
    fn one_peer_cannot_open_unbounded_incoming_streams() {
        let limit = P2pLimits::default().max_streams();
        let mut streams = BTreeMap::new();
        for id in 1..=128 {
            streams.insert(
                id,
                P2pNetworkStreamState::new_incoming(redux::Timestamp::ZERO),
            );
            assert!(!incoming_stream_limit_exceeded(&streams, limit));
        }
        streams.insert(
            129,
            P2pNetworkStreamState::new_incoming(redux::Timestamp::ZERO),
        );
        assert!(incoming_stream_limit_exceeded(&streams, limit));
        streams.remove(&1);
        assert!(!incoming_stream_limit_exceeded(&streams, limit));
    }
}
