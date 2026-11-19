use std::sync::Arc;

use binprot::{BinProtRead, BinProtWrite};
use mina_core::{bug_condition, error, fuzz_maybe, fuzzed_maybe, warn, Substate};
use mina_p2p_messages::{
    rpc,
    rpc_kernel::{
        BinprotTag, Error as RpcError, MessageHeader, NeedsLength, PayloadBinprotReader as _,
        QueryHeader, ResponseHeader, ResponsePayload, RpcMethod, RpcQueryReadError,
        RpcResponseReadError, RpcResult,
    },
    v2,
    versioned::Ver,
};
use redux::Dispatcher;

use crate::{
    channels::rpc::{
        staged_ledger_serve, BestTipWithProof, CatchupRequest, P2pChannelsRpcAction, P2pRpcRequest,
        P2pRpcResponse, StagedLedgerAuxAndPendingCoinbases, MAX_CATCHUP_RPC_REQUESTS,
    },
    connection::outgoing::P2pConnectionOutgoingInitOpts,
    disconnection::{P2pDisconnectionAction, P2pDisconnectionReason},
    Data, Limit, P2pLimits, P2pNetworkSchedulerAction, P2pNetworkState,
    P2pNetworkStreamResetReason, P2pNetworkYamuxAction, PeerId, YamuxFlags,
};

use self::p2p_network_rpc_state::{P2pNetworkRpcError, PendingIncomingQuery};

use super::*;

impl P2pNetworkRpcState {
    /// Substate is accessed
    pub fn reducer<State, Action>(
        mut state_context: Substate<Action, State, P2pNetworkState>,
        action: redux::ActionWithMeta<P2pNetworkRpcAction>,
        limits: &P2pLimits,
        catchup_enabled: bool,
        rpc_channel_available: bool,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let (mut action, meta) = action.split();
        if let P2pNetworkRpcAction::Refusal {
            peer_id,
            serving,
            block,
            ..
        } = &action
        {
            state_context.get_substate_mut()?.refusal_warnings.record(
                *peer_id,
                *serving,
                *block,
                meta.time(),
            );
            return Ok(());
        }
        let served_query = match &action {
            P2pNetworkRpcAction::IncomingMessage {
                message: RpcMessage::Query { header, bytes },
                ..
            } => served_wire_rpc(header, bytes, catchup_enabled),
            _ => None,
        };
        let outstanding = if served_query.is_some() {
            state_context
                .get_substate()?
                .scheduler
                .rpc_incoming_streams
                .get(action.peer_id())
                .map(|streams| streams.values().map(|s| s.pending_incoming.len()).sum())
                .unwrap_or(0)
        } else {
            0
        };
        let channel_rejected = served_query.is_some()
            && (!rpc_channel_available
                || outstanding
                    >= if catchup_enabled {
                        MAX_CATCHUP_RPC_REQUESTS
                    } else {
                        crate::channels::rpc::MAX_P2P_RPC_REMOTE_CONCURRENT_REQUESTS
                    });
        // Reserve expected response bytes per active query. The queue check
        // in Yamux accounts for actual bytes;
        // this check prevents many peers from starting expensive work before
        // those responses have been encoded.
        let staged_ledger_reservation = state_context
            .get_substate()?
            .staged_ledger_reservation(limits);
        let memory_rejected = if let P2pNetworkRpcAction::IncomingMessage {
            peer_id,
            message: RpcMessage::Query { header, .. },
            ..
        } = &action
        {
            if served_query.is_none() || channel_rejected {
                false
            } else {
                let scheduler = &state_context.get_substate()?.scheduler;
                let (queued, peer_queued) = scheduler.queued_outgoing_bytes(peer_id);
                let (active, peer_active) = scheduler.reserved_rpc_response_bytes(peer_id);
                let reserve = response_reservation(header, staged_ledger_reservation);
                !limits.rpc_work_budget_allows(
                    queued.saturating_add(active),
                    peer_queued.saturating_add(peer_active),
                    reserve,
                )
            }
        } else {
            false
        };
        let rate_rejected = if let (
            Some((method, cost)),
            P2pNetworkRpcAction::IncomingMessage { addr, peer_id, .. },
        ) = (served_query, &action)
        {
            !channel_rejected
                && !memory_rejected
                && !state_context
                    .get_substate_mut()
                    .expect("network substate")
                    .serving_limits
                    .admit(method, *peer_id, addr.sock_addr.ip(), meta.time(), cost)
        } else {
            false
        };
        // Allocate a local ID before borrowing a stream. Preserve the peer's
        // ID in pending_incoming and restore it when encoding the response.
        let mut incoming_query = None;
        if let P2pNetworkRpcAction::IncomingMessage {
            message: RpcMessage::Query { header, .. },
            ..
        } = &mut action
        {
            incoming_query = Some(header.clone());
            let network = state_context.get_substate_mut()?;
            header.id = network.next_incoming_rpc_id;
            network.next_incoming_rpc_id = network
                .next_incoming_rpc_id
                .checked_add(1)
                .ok_or("inbound RPC ID exhausted")?;
        }
        // Measure a staged-ledger answer against its limit and the budgets
        // while the whole network state can be read, before it is encoded.
        let staged_ledger = if let P2pNetworkRpcAction::OutgoingStagedLedgerResponse {
            peer_id,
            response,
            parts,
        } = &action
        {
            let network = state_context.get_substate_mut()?;
            let plan = StagedLedgerAnswer::plan(network, limits, peer_id, response.id, parts);
            if let Some(len) = plan.as_ref().and_then(StagedLedgerAnswer::len) {
                network.staged_ledger_answer_len = Some(len);
            }
            plan
        } else {
            None
        };
        let budgeted = if matches!(action, P2pNetworkRpcAction::IncomingData { .. }) {
            state_context
                .get_substate()?
                .scheduler
                .rpc_budgeted_answer_bytes()
        } else {
            0
        };
        let rpc_state = state_context
            .get_substate_mut()?
            .find_rpc_state_mut(&action)
            .ok_or_else(|| format!("RPC state not found for action: {:?}", action))
            .inspect_err(|e| bug_condition!("{}", e))?;

        match action {
            P2pNetworkRpcAction::Refusal { .. } => unreachable!(),
            P2pNetworkRpcAction::Init {
                incoming,
                addr,
                peer_id,
                stream_id,
            } => {
                rpc_state.is_incoming = incoming;
                rpc_state.last_received = Some(meta.time());

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkRpcAction::OutgoingData {
                    addr,
                    peer_id,
                    stream_id,
                    data: Data::from(RpcMessage::Handshake.into_bytes()),
                    fin: false,
                });
                Ok(())
            }
            P2pNetworkRpcAction::IncomingData {
                data,
                addr,
                peer_id,
                stream_id,
            } => {
                let budgeted_elsewhere = budgeted.saturating_sub(rpc_state.incoming_budgeted);
                let taken = rpc_state.take_incoming(&data, limits, budgeted_elsewhere);
                match &taken.error {
                    Some(err @ P2pNetworkRpcError::Limit(..)) => {
                        // The stream takes no more actions from here on.
                        mina_core::warn!(meta.time();
                            kind = "P2pNetworkRpcLimitExceeded",
                            summary = format!("RPC stream stopped: {err}"),
                            peer_id = display(peer_id),
                        );
                    }
                    Some(err) => {
                        warn!(meta.time(); summary = "unreadable RPC message", peer_id = display(&peer_id), error = display(err));
                    }
                    None => {}
                }
                if let Some((query, len)) = &taken.deferred {
                    mina_core::info!(meta.time();
                        kind = "P2pNetworkRpcAnswerDeferred",
                        summary = format!(
                            "{} answer of {len} bytes discarded, {budgeted_elsewhere} bytes of large block answers already in flight; asking again",
                            query.tag.to_string_lossy()
                        ),
                        peer_id = display(peer_id),
                    );
                }

                let incoming = rpc_state.incoming.front().cloned();
                let dispatcher = state_context.into_dispatcher();

                if let Some((query, _)) = taken.deferred {
                    // As for an answer that failed: the block is asked again,
                    // of another peer first.
                    dispatcher.push(P2pChannelsRpcAction::ResponseReceived {
                        peer_id,
                        id: query.id,
                        response: None,
                    });
                }
                if let Some(message) = incoming {
                    dispatcher.push(P2pNetworkRpcAction::IncomingMessage {
                        addr,
                        peer_id,
                        stream_id,
                        message,
                    })
                }

                Ok(())
            }
            ref action @ P2pNetworkRpcAction::IncomingMessage {
                ref message,
                addr,
                peer_id,
                stream_id,
            } => {
                rpc_state.last_received = Some(meta.time());
                if let RpcMessage::Response { header, .. } = &message {
                    if let Some(QueryHeader { id, tag, version }) = &rpc_state.pending {
                        *rpc_state
                            .total_stats
                            .entry((tag.clone(), *version))
                            .or_default() += 1;
                        if id != &header.id {
                            error!(meta.time(); "receiving response with wrong id: {}", header.id);
                        }
                    } else {
                        error!(meta.time(); "receiving response without query");
                    }
                } else if let RpcMessage::Query { header, bytes } = &message {
                    if rate_rejected || memory_rejected || channel_rejected {
                        let wire_header = incoming_query.as_ref().unwrap_or(header);
                        rpc_state.incoming.pop_front();
                        let data = if rate_rejected || memory_rejected {
                            rate_limit_response_data(addr.sock_addr, peer_id)
                        } else {
                            unimplemented_response_data(&wire_header.tag, wire_header.version)
                        };
                        let next = rpc_state.incoming.front().cloned();
                        let dispatcher = state_context.into_dispatcher();
                        if rate_rejected || memory_rejected {
                            dispatcher.push(P2pNetworkRpcAction::Refusal {
                                peer_id,
                                stream_id,
                                serving: true,
                                block: wire_header.tag.as_ref() == rpc::GetTransitionChainV2::NAME,
                            });
                        }
                        dispatcher.push(P2pNetworkRpcAction::OutgoingData {
                            addr,
                            peer_id,
                            stream_id,
                            fin: false,
                            data: RpcMessage::Response {
                                header: ResponseHeader { id: wire_header.id },
                                bytes: data,
                            }
                            .into_bytes()
                            .into(),
                        });
                        if let Some(message) = next {
                            dispatcher.push(P2pNetworkRpcAction::IncomingMessage {
                                addr,
                                peer_id,
                                stream_id,
                                message,
                            });
                        }
                        return Ok(());
                    }
                    if let Some(wire_header) = incoming_query {
                        rpc_state.pending_incoming.insert(
                            header.id,
                            PendingIncomingQuery {
                                reserved_bytes: if served_query.is_some() {
                                    response_reservation(&wire_header, staged_ledger_reservation)
                                } else {
                                    0
                                },
                                header: wire_header,
                                started_at: meta.time(),
                            },
                        );
                    } else if rpc_state.pending.is_none() {
                        rpc_state.pending = Some(header.clone());
                    } else {
                        error!(meta.time(); "receiving query while another query is pending");
                    }
                }

                rpc_state.incoming.pop_front();

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let network_state: &P2pNetworkState = state.substate()?;
                let state = network_state
                    .find_rpc_state(action)
                    .ok_or_else(|| format!("RPC state not found for action: {:?}", action))?;

                match &message {
                    RpcMessage::Handshake => {
                        if !state.is_incoming {
                            dispatcher.push(P2pChannelsRpcAction::Ready { peer_id });
                        }
                    }
                    RpcMessage::Heartbeat => {}
                    RpcMessage::Query { header, bytes } => {
                        if catchup_enabled {
                            mina_core::debug!(meta.time();
                                kind = "P2pCatchupRpcQuery",
                                peer_id = display(peer_id),
                                rpc = header.tag.to_string_lossy(),
                                version = header.version,
                                id = header.id,
                                stream_id = stream_id,
                            );
                        }
                        match dispatch_rpc_query(
                            peer_id,
                            header,
                            bytes,
                            dispatcher,
                            catchup_enabled,
                        ) {
                            Ok(QueryDispatch::Request) => {}
                            Ok(QueryDispatch::Immediate) => {}
                            Ok(QueryDispatch::Menu) => {
                                mina_core::debug!(meta.time();
                                    kind = "P2pNetworkRpcMenuAnswered",
                                    summary = "answered the versioned RPC menu",
                                    peer_id = display(peer_id),
                                );
                            }
                            Ok(QueryDispatch::Unimplemented) => {
                                mina_core::debug!(meta.time();
                                    kind = "P2pNetworkRpcUnimplementedQuery",
                                    summary = format!(
                                        "answered unimplemented rpc {}:{}",
                                        header.tag.to_string_lossy(), header.version
                                    ),
                                    peer_id = display(peer_id),
                                );
                            }
                            Err(e) => {
                                dispatcher.push(P2pDisconnectionAction::Init {
                                    peer_id,
                                    reason: P2pDisconnectionReason::P2pChannelReceiveFailed(
                                        e.to_string(),
                                    ),
                                });
                            }
                        }
                    }
                    RpcMessage::Response {
                        header: ResponseHeader { id },
                        bytes,
                    } => match state.pending.as_ref() {
                        Some(query_header) if &query_header.id == id => {
                            let query_header = query_header.clone();
                            // unset pending
                            dispatcher
                                .push(P2pNetworkRpcAction::PrunePending { peer_id, stream_id });

                            match dispatch_rpc_response(peer_id, &query_header, bytes, dispatcher) {
                                Ok(None) => {}
                                Ok(Some(_error)) => {
                                    dispatcher.push(P2pNetworkRpcAction::Refusal {
                                        peer_id,
                                        stream_id,
                                        serving: false,
                                        block: query_header.tag.as_ref()
                                            == rpc::GetTransitionChainV2::NAME,
                                    });
                                }
                                Err(e) => {
                                    dispatcher.push(P2pDisconnectionAction::Init {
                                        peer_id,
                                        reason: P2pDisconnectionReason::P2pChannelReceiveFailed(
                                            e.to_string(),
                                        ),
                                    });
                                }
                            }
                        }
                        Some(header) => {
                            error!(meta.time(); "received response with it {} while expecting {id}", header.id);
                        }
                        None => {
                            error!(meta.time(); "received response while no query is sent");
                        }
                    },
                }

                if let Some(message) = state.incoming.front().cloned() {
                    dispatcher.push(P2pNetworkRpcAction::IncomingMessage {
                        addr,
                        peer_id,
                        stream_id,
                        message,
                    });
                }
                Ok(())
            }
            P2pNetworkRpcAction::PrunePending { .. } => {
                rpc_state.pending = None;
                Ok(())
            }
            P2pNetworkRpcAction::Timeout {
                addr,
                peer_id,
                stream_id,
            } => {
                // OCaml 4.0.2 does not propagate a closed outgoing pipe to
                // CloseStream. Its server's heartbeat timeout closes Async RPC
                // and the incoming handler resets the underlying stream.
                // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_net2/libp2p_stream.ml#L201-L216>
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkYamuxAction::OutgoingData {
                    addr,
                    stream_id,
                    data: Data::default(),
                    flags: YamuxFlags::RST,
                });
                dispatcher.push(P2pNetworkSchedulerAction::StreamReset {
                    addr,
                    peer_id,
                    stream_id,
                    protocol: None,
                    reason: P2pNetworkStreamResetReason::RpcTimeout,
                });
                Ok(())
            }
            P2pNetworkRpcAction::HeartbeatSend {
                addr,
                peer_id,
                stream_id,
            } => {
                rpc_state.last_heartbeat_sent = Some(meta.time());

                let dispatcher = state_context.into_dispatcher();

                dispatcher.push(P2pNetworkRpcAction::OutgoingData {
                    addr,
                    peer_id,
                    stream_id,
                    data: Data::from(RpcMessage::Heartbeat.into_bytes()),
                    fin: false,
                });

                Ok(())
            }
            P2pNetworkRpcAction::OutgoingQuery {
                query,
                data,
                peer_id,
            } => {
                rpc_state.abandon_budgeted_answer();
                rpc_state.last_id = query.id;
                rpc_state.pending = Some(query.clone());

                let addr = rpc_state.addr;
                let stream_id = rpc_state.stream_id;
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pNetworkRpcAction::OutgoingData {
                    addr,
                    peer_id,
                    stream_id,
                    data: Data::from(
                        RpcMessage::Query {
                            header: query,
                            bytes: data,
                        }
                        .into_bytes(),
                    ),
                    fin: false,
                });

                Ok(())
            }
            P2pNetworkRpcAction::OutgoingData {
                addr,
                stream_id,
                mut data,
                ..
            } => {
                let dispatcher = state_context.into_dispatcher();
                fuzz_maybe!(&mut data, crate::fuzzer::mutate_rpc_data);
                let flags = fuzzed_maybe!(Default::default(), crate::fuzzer::mutate_yamux_flags);

                dispatcher.push(P2pNetworkYamuxAction::OutgoingData {
                    addr,
                    stream_id,
                    data,
                    flags,
                });

                Ok(())
            }
            P2pNetworkRpcAction::OutgoingResponse {
                peer_id,
                mut response,
                data,
            } => {
                let correlated = rpc_state.pending_incoming.remove(&response.id);
                if correlated.is_none()
                    && !matches!(rpc_state.pending, Some(QueryHeader { id, .. }) if id == response.id)
                {
                    bug_condition!("pending query does not match the response");
                    return Ok(());
                }
                let stream_id = rpc_state.stream_id;
                let addr = rpc_state.addr;
                let dispatcher = state_context.into_dispatcher();

                if let Some(query) = correlated {
                    response.id = query.header.id;
                } else {
                    dispatcher.push(P2pNetworkRpcAction::PrunePending { peer_id, stream_id });
                }
                dispatcher.push(P2pNetworkRpcAction::OutgoingData {
                    addr,
                    peer_id,
                    stream_id,
                    data: Data::from(
                        RpcMessage::Response {
                            header: response,
                            bytes: data,
                        }
                        .into_bytes(),
                    ),
                    fin: false,
                });
                Ok(())
            }
            P2pNetworkRpcAction::OutgoingStagedLedgerResponse {
                peer_id,
                response,
                parts,
            } => {
                // Removing the query releases its reservation. The plan was
                // measured without it, and the queue check of the frames below
                // runs before any other action, so it sees the same budgets.
                let Some(query) = rpc_state.pending_incoming.remove(&response.id) else {
                    bug_condition!("staged-ledger answer for a query that is not pending");
                    return Ok(());
                };
                let stream_id = rpc_state.stream_id;
                let addr = rpc_state.addr;
                let header = ResponseHeader {
                    id: query.header.id,
                };
                let dispatcher = state_context.into_dispatcher();
                let chunks = match staged_ledger {
                    Some(StagedLedgerAnswer::Send(message)) => {
                        staged_ledger_serve::encode_chunks(&parts, &message, STAGED_LEDGER_CHUNK)
                            .map(|chunks| (message.len, chunks))
                            .map_err(|error| format!("it can't be encoded: {error}"))
                    }
                    Some(StagedLedgerAnswer::TooLarge(max)) => {
                        Err(format!("it is over the {max}-byte limit"))
                    }
                    Some(StagedLedgerAnswer::NoRoom(len)) => Err(format!(
                        "the send budgets have no room for its {len} bytes now"
                    )),
                    Some(StagedLedgerAnswer::Unavailable(error)) => {
                        Err(format!("it can't be measured: {error}"))
                    }
                    None => Err("its stream is gone".to_owned()),
                };
                match chunks {
                    Ok((len, chunks)) => {
                        mina_core::debug!(meta.time();
                            kind = "P2pStagedLedgerAnswerSent",
                            summary = format!("staged-ledger answer of {len} bytes sent"),
                            peer_id = display(peer_id),
                        );
                        for chunk in chunks {
                            dispatcher.push(P2pNetworkRpcAction::OutgoingData {
                                addr,
                                peer_id,
                                stream_id,
                                data: Data::from(chunk),
                                fin: false,
                            });
                        }
                    }
                    Err(reason) => {
                        mina_core::info!(meta.time();
                            kind = "P2pStagedLedgerAnswerRefused",
                            summary = format!("staged-ledger answer refused, {reason}: sent none"),
                            peer_id = display(peer_id),
                        );
                        let none: rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response = None;
                        dispatcher.push(P2pNetworkRpcAction::OutgoingData {
                            addr,
                            peer_id,
                            stream_id,
                            data: Data::from(
                                RpcMessage::Response {
                                    header,
                                    bytes: crate::channels::rpc::response_bytes(none).into(),
                                }
                                .into_bytes(),
                            ),
                            fin: false,
                        });
                    }
                }
                Ok(())
            }
        }
    }

    /// Whether the pending query is ours and its answer carries blocks.
    fn awaits_block_answer(&self) -> bool {
        !self.is_incoming
            && self.pending.as_ref().is_some_and(|query| {
                let tag: &[u8] = query.tag.as_ref();
                tag == rpc::GetTransitionChainV2::NAME || tag == rpc::GetBestTipV2::NAME
            })
    }

    /// Discards the rest of a large block answer still arriving when a new
    /// query goes out on the stream. Its query has timed out, so the answer
    /// would be dropped anyway; this releases its buffer and its share of the
    /// budget at once, rather than when the rest arrives or the peer goes.
    fn abandon_budgeted_answer(&mut self) {
        if let (Some(len), true) = (self.incoming_len, self.incoming_budgeted > 0) {
            self.skip_remaining = len.saturating_add(8).saturating_sub(self.buffer.len());
            self.buffer = Vec::new();
            self.incoming_len = None;
            self.incoming_budgeted = 0;
        }
    }

    /// Reads `data`, the next bytes of the stream, into `incoming`, and keeps
    /// the start of an incomplete message in `buffer`. `budgeted_elsewhere` is
    /// what other streams hold of
    /// [`P2pLimits::RPC_LARGE_BLOCK_ANSWERS_BUDGET`].
    ///
    /// A message's buffer grows with the bytes received, never past the
    /// length it declares, which the limits bound. A block answer over
    /// [`P2pLimits::RPC_BLOCK_ANSWER_UNBUDGETED`] is read only if it fits the
    /// budget. Otherwise its bytes are discarded as they arrive, and its query
    /// is no longer pending and is returned, to be answered as failed.
    fn take_incoming(
        &mut self,
        mut data: &[u8],
        limits: &P2pLimits,
        budgeted_elsewhere: usize,
    ) -> TakenIncoming {
        const PREFIX: usize = 8;
        let mut taken = TakenIncoming::default();
        loop {
            if self.skip_remaining > 0 {
                let skipped = self.skip_remaining.min(data.len());
                self.skip_remaining -= skipped;
                data = &data[skipped..];
                if self.skip_remaining > 0 {
                    break;
                }
            }
            if self.buffer.len() < PREFIX {
                let n = (PREFIX - self.buffer.len()).min(data.len());
                self.buffer.extend_from_slice(&data[..n]);
                data = &data[n..];
                if self.buffer.len() < PREFIX {
                    break;
                }
            }
            let len = match self.incoming_len {
                Some(len) => len,
                None => {
                    let mut prefix = [0; PREFIX];
                    prefix.copy_from_slice(&self.buffer[..PREFIX]);
                    let len = usize::try_from(u64::from_le_bytes(prefix)).unwrap_or(usize::MAX);
                    if let Err(err) = self.check_rpc_limit(len, limits) {
                        self.error = Some(err.clone());
                        taken.error = Some(err);
                        break;
                    }
                    if self.awaits_block_answer() && len > P2pLimits::RPC_BLOCK_ANSWER_UNBUDGETED {
                        if budgeted_elsewhere.saturating_add(len)
                            > P2pLimits::RPC_LARGE_BLOCK_ANSWERS_BUDGET
                        {
                            self.buffer = Vec::new();
                            self.skip_remaining = len;
                            taken.deferred = self.pending.take().map(|query| (query, len));
                            continue;
                        }
                        self.incoming_budgeted = len;
                    }
                    self.incoming_len = Some(len);
                    len
                }
            };
            let total = len.saturating_add(PREFIX);
            let n = (total - self.buffer.len()).min(data.len());
            extend_within(&mut self.buffer, &data[..n], total);
            data = &data[n..];
            if self.buffer.len() < total {
                break;
            }

            let message = std::mem::take(&mut self.buffer);
            self.incoming_len = None;
            self.incoming_budgeted = 0;
            let mut slice = &message[PREFIX..];
            let msg = match MessageHeader::binprot_read(&mut slice) {
                Ok(MessageHeader::Heartbeat) => RpcMessage::Heartbeat,
                Ok(MessageHeader::Response(h))
                    if h.id == u64::from_le_bytes(*b"RPC\x00\x00\x00\x00\x00") =>
                {
                    RpcMessage::Handshake
                }
                Ok(MessageHeader::Query(header)) => RpcMessage::Query {
                    header,
                    bytes: slice.to_vec().into(),
                },
                Ok(MessageHeader::Response(header)) => RpcMessage::Response {
                    header,
                    bytes: slice.to_vec().into(),
                },
                Err(err) => {
                    let err = P2pNetworkRpcError::Binprot(err.to_string());
                    self.error = Some(err.clone());
                    taken.error = Some(err);
                    break;
                }
            };
            self.incoming.push_back(msg);
        }
        taken
    }

    fn check_rpc_limit(&self, len: usize, limits: &P2pLimits) -> Result<(), P2pNetworkRpcError> {
        let (limit, kind): (_, &[u8]) = if self.is_incoming {
            // only requests are allowed
            (limits.rpc_query(), b"<query>")
        } else if let Some(QueryHeader { tag, .. }) = self.pending.as_ref() {
            use mina_p2p_messages::rpc::*;
            match tag.as_ref() {
                GetCompletedSnarksV1::NAME => (
                    Limit::Some(rpc::MAX_COMPLETED_SNARKS_BYTES),
                    GetCompletedSnarksV1::NAME,
                ),
                GetBestTipV2::NAME => (limits.rpc_get_best_tip(), GetBestTipV2::NAME),
                AnswerSyncLedgerQueryV2::NAME => (
                    limits.rpc_answer_sync_ledger_query(),
                    AnswerSyncLedgerQueryV2::NAME,
                ),
                GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::NAME => (
                    limits.rpc_get_staged_ledger(),
                    GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::NAME,
                ),
                GetTransitionChainV2::NAME => (
                    limits.rpc_get_transition_chain(),
                    GetTransitionChainV2::NAME,
                ),
                GetSomeInitialPeersV1ForV2::NAME => (
                    limits.rpc_get_some_initial_peers(),
                    GetSomeInitialPeersV1ForV2::NAME,
                ),
                _ => (Limit::Some(0), b"<unimplemented>"),
            }
        } else {
            (limits.rpc_service_message(), b"<service_messages>")
        };
        let kind = String::from_utf8_lossy(kind);
        if len > limit {
            Err(P2pNetworkRpcError::Limit(kind.into_owned(), len, limit))
        } else {
            Ok(())
        }
    }
}

/// What [`P2pNetworkRpcState::take_incoming`] found besides messages.
#[derive(Debug, Default)]
struct TakenIncoming {
    /// The error that stopped the stream.
    error: Option<P2pNetworkRpcError>,
    /// The query whose answer was discarded for the large block answer
    /// budget, and the answer's length.
    deferred: Option<(QueryHeader, usize)>,
}

/// Appends `bytes` to `buf`, which holds the start of a message of `len`
/// bytes. Grows by doubling, as `Vec` would, so the allocation stays paced by
/// the bytes received, but never past `len`, as the WebRTC service does.
fn extend_within(buf: &mut Vec<u8>, bytes: &[u8], len: usize) {
    let need = buf.len().saturating_add(bytes.len());
    if need > buf.capacity() {
        let target = need
            .max(buf.capacity().saturating_mul(2))
            .min(len.max(need));
        buf.reserve_exact(target - buf.len());
    }
    buf.extend_from_slice(bytes);
}

/// The pieces a staged-ledger answer is encoded in. Each waits in the Yamux
/// queue as one frame and is freed once sent, and splitting one at the edge of
/// the peer's receive window copies at most this much.
const STAGED_LEDGER_CHUNK: usize = 1024 * 1024;

/// What to do with a staged-ledger answer, decided before it is encoded.
enum StagedLedgerAnswer {
    /// Encode and send it: it fits its limit and the send budgets.
    Send(staged_ledger_serve::StagedLedgerMessage),
    /// It is over [`P2pLimits::rpc_served_staged_ledger`], here.
    TooLarge(usize),
    /// It is within the limit, but its bytes, here, don't fit the send
    /// budgets now.
    NoRoom(usize),
    /// Its parts can't be encoded.
    Unavailable(String),
}

impl StagedLedgerAnswer {
    /// Measures the answer with `parts` to `peer_id`'s query `id`, with the
    /// query's own reservation released. `None` if the query is not pending.
    fn plan(
        network: &P2pNetworkState,
        limits: &P2pLimits,
        peer_id: &PeerId,
        id: u64,
        parts: &StagedLedgerAuxAndPendingCoinbases,
    ) -> Option<Self> {
        let rpc = network
            .scheduler
            .rpc_incoming_streams
            .get(peer_id)?
            .values()
            .find(|stream| stream.pending_incoming.contains_key(&id))?;
        let query = rpc.pending_incoming.get(&id)?;
        let mut header = Vec::new();
        MessageHeader::Response(ResponseHeader {
            id: query.header.id,
        })
        .binprot_write(&mut header)
        .ok()?;
        let max = match limits.rpc_served_staged_ledger() {
            Limit::Some(max) => max,
            Limit::Unlimited => usize::MAX,
        };
        Some(match staged_ledger_serve::measure(parts, &header, max) {
            Err(error) => Self::Unavailable(error.to_string()),
            Ok(None) => Self::TooLarge(max),
            Ok(Some(message)) => {
                if network.scheduler.outgoing_data_admitted(
                    &rpc.addr,
                    peer_id,
                    rpc.stream_id,
                    message.len,
                    query.reserved_bytes,
                    limits,
                ) {
                    Self::Send(message)
                } else {
                    Self::NoRoom(message.len)
                }
            }
        })
    }

    /// The answer's length, as far as it is known: the limit for one over
    /// it. The next staged-ledger query reserves this much.
    fn len(&self) -> Option<usize> {
        match self {
            Self::Send(message) => Some(message.len),
            Self::TooLarge(len) | Self::NoRoom(len) => Some(*len),
            Self::Unavailable(_) => None,
        }
    }
}

/// Reserves work before dispatch: what the answer is expected to take of the
/// send budgets. A staged-ledger answer reserves `staged_ledger`
/// ([`P2pNetworkState::staged_ledger_reservation`]); its real length is
/// checked before it is encoded.
fn response_reservation(header: &QueryHeader, staged_ledger: usize) -> usize {
    match header.tag.as_ref() {
        rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::NAME => staged_ledger,
        rpc::GetCompletedSnarksV1::NAME => 8 * 1024 * 1024,
        rpc::GetBestTipV2::NAME | rpc::GetTransitionChainProofV1ForV2::NAME => 4 * 1024 * 1024,
        _ => crate::channels::rpc::MAX_SERVED_LEDGER_ANSWER_BYTES,
    }
}

/// Charge a chain batch once, by its original length, even on the legacy
/// non-allowlisted dispatcher. Menu and unimplemented methods are unchanged.
fn served_wire_rpc(
    header: &QueryHeader,
    mut bytes: &[u8],
    catchup: bool,
) -> Option<(crate::channels::rpc::ServedRpc, u32)> {
    use crate::channels::rpc::ServedRpc as M;
    let method = match (header.tag.as_ref(), header.version) {
        (rpc::GetBestTipV2::NAME, rpc::GetBestTipV2::VERSION) => {
            rpc::GetBestTipV2::query_payload(&mut bytes).ok()?;
            M::BestTip
        }
        (rpc::GetCompletedSnarksV1::NAME, rpc::GetCompletedSnarksV1::VERSION) => {
            rpc::GetCompletedSnarksV1::query_payload(&mut bytes).ok()?;
            M::CompletedSnarks
        }
        (rpc::AnswerSyncLedgerQueryV2::NAME, rpc::AnswerSyncLedgerQueryV2::VERSION) => {
            rpc::AnswerSyncLedgerQueryV2::query_payload(&mut bytes).ok()?;
            M::LedgerQuery
        }
        (
            rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::NAME,
            rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::VERSION,
        ) => {
            rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::query_payload(&mut bytes).ok()?;
            M::StagedLedger
        }
        (rpc::GetSomeInitialPeersV1ForV2::NAME, rpc::GetSomeInitialPeersV1ForV2::VERSION) => {
            rpc::GetSomeInitialPeersV1ForV2::query_payload(&mut bytes).ok()?;
            M::InitialPeers
        }
        (rpc::GetTransitionChainV2::NAME, rpc::GetTransitionChainV2::VERSION) => {
            let hashes = rpc::GetTransitionChainV2::query_payload(&mut bytes).ok()?;
            if hashes.is_empty() && !catchup {
                return None;
            }
            return Some((
                M::TransitionChain,
                u32::try_from(hashes.len()).unwrap_or(u32::MAX).max(1),
            ));
        }
        (rpc::GetAncestryV2::NAME, rpc::GetAncestryV2::VERSION) if catchup => {
            rpc::GetAncestryV2::query_payload(&mut bytes).ok()?;
            M::Ancestry
        }
        (
            rpc::GetTransitionChainProofV1ForV2::NAME,
            rpc::GetTransitionChainProofV1ForV2::VERSION,
        ) if catchup => {
            rpc::GetTransitionChainProofV1ForV2::query_payload(&mut bytes).ok()?;
            M::TransitionChainProof
        }
        (rpc::GetTransitionKnowledgeV1ForV2::NAME, rpc::GetTransitionKnowledgeV1ForV2::VERSION)
            if catchup =>
        {
            rpc::GetTransitionKnowledgeV1ForV2::query_payload(&mut bytes).ok()?;
            M::TransitionKnowledge
        }
        _ => return None,
    };
    Some((method, 1))
}

/// OCaml raises Failure; Async RPC returns Uncaught_exn with an exception
/// sexp, without closing the connection. Encode that sexp explicitly: our
/// legacy inbound error reader deliberately ignores exception details.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/gossip_net/libp2p.ml#L159-L165>
fn rate_limit_response_data(addr: std::net::SocketAddr, peer: PeerId) -> Data {
    #[derive(binprot_derive::BinProtWrite)]
    enum Sexp {
        Atom(mina_p2p_messages::string::CharString),
        List(Vec<Sexp>),
    }
    let family = if addr.is_ipv4() { "ip4" } else { "ip6" };
    let error = Sexp::List(vec![
        Sexp::Atom("Failure".into()),
        Sexp::Atom(
            format!(
                "peer exceeded capacity: /{family}/{}/tcp/{}/p2p/{}",
                addr.ip(),
                addr.port(),
                peer.to_libp2p_string(),
            )
            .into_bytes()
            .into(),
        ),
    ]);
    let mut bytes = vec![1, 3]; // Rpc_result.Error, Rpc_error.Uncaught_exn
    error
        .binprot_write(&mut bytes)
        .expect("writing RPC error to Vec");
    bytes.into()
}

/// What [`dispatch_rpc_query`] did with a query.
enum QueryDispatch {
    /// Passed on as a request, to be answered when its response is ready.
    Request,
    /// Answered directly without channel work.
    Immediate,
    /// The versioned RPC menu, answered at once.
    Menu,
    /// Answered at once with `Unimplemented_rpc`.
    Unimplemented,
}

fn dispatch_rpc_query<State, Action>(
    peer_id: PeerId,
    QueryHeader { tag, version, id }: &QueryHeader,
    mut bytes: &[u8],
    dispatcher: &mut Dispatcher<Action, State>,
    catchup_enabled: bool,
) -> Result<QueryDispatch, RpcQueryReadError>
where
    State: crate::P2pStateTrait,
    Action: crate::P2pActionTrait<State>,
{
    let id = *id;
    if catchup_enabled {
        let request = match (tag.as_ref(), *version) {
            (rpc::GetAncestryV2::NAME, rpc::GetAncestryV2::VERSION) => Some(
                CatchupRequest::Ancestry(Box::new(rpc::GetAncestryV2::query_payload(&mut bytes)?)),
            ),
            (rpc::GetTransitionChainV2::NAME, rpc::GetTransitionChainV2::VERSION) => {
                Some(CatchupRequest::TransitionChain(
                    rpc::GetTransitionChainV2::query_payload(&mut bytes)?,
                ))
            }
            (
                rpc::GetTransitionChainProofV1ForV2::NAME,
                rpc::GetTransitionChainProofV1ForV2::VERSION,
            ) => Some(CatchupRequest::TransitionChainProof(
                rpc::GetTransitionChainProofV1ForV2::query_payload(&mut bytes)?,
            )),
            (
                rpc::GetTransitionKnowledgeV1ForV2::NAME,
                rpc::GetTransitionKnowledgeV1ForV2::VERSION,
            ) => {
                rpc::GetTransitionKnowledgeV1ForV2::query_payload(&mut bytes)?;
                Some(CatchupRequest::TransitionKnowledge)
            }
            _ => None,
        };
        if let Some(request) = request {
            dispatcher.push(P2pChannelsRpcAction::RequestReceived {
                peer_id,
                id,
                request: Box::new(P2pRpcRequest::Catchup(request)),
            });
            return Ok(QueryDispatch::Request);
        }
    }
    match (tag.as_ref(), *version) {
        (rpc::VersionedRpcMenuV1::NAME, rpc::VersionedRpcMenuV1::VERSION) => {
            let () = rpc::VersionedRpcMenuV1::query_payload(&mut bytes)?;
            dispatcher.push(P2pNetworkRpcAction::OutgoingResponse {
                peer_id,
                response: ResponseHeader { id },
                data: menu_response_data(),
            });
            return Ok(QueryDispatch::Menu);
        }
        (rpc::GetBestTipV2::NAME, rpc::GetBestTipV2::VERSION) => {
            rpc::GetBestTipV2::query_payload(&mut bytes)?;
            dispatcher.push(P2pChannelsRpcAction::RequestReceived {
                peer_id,
                id,
                request: Box::new(P2pRpcRequest::BestTipWithProof),
            });
        }
        (rpc::AnswerSyncLedgerQueryV2::NAME, rpc::AnswerSyncLedgerQueryV2::VERSION) => {
            let (hash, query) = rpc::AnswerSyncLedgerQueryV2::query_payload(&mut bytes)?;
            let hash = v2::LedgerHash::from(v2::MinaBaseLedgerHash0StableV1(hash));

            dispatcher.push(P2pChannelsRpcAction::RequestReceived {
                peer_id,
                id,
                request: Box::new(P2pRpcRequest::LedgerQuery(hash, query)),
            });
        }
        (
            rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::NAME,
            rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::VERSION,
        ) => {
            let hash =
                rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::query_payload(&mut bytes)?;
            let hash = v2::StateHash::from(v2::DataHashLibStateHashStableV1(hash));
            let request = Box::new(P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(
                hash,
            ));

            dispatcher.push(P2pChannelsRpcAction::RequestReceived {
                peer_id,
                id,
                request,
            });
        }
        (rpc::GetTransitionChainV2::NAME, rpc::GetTransitionChainV2::VERSION) => {
            let hashes = rpc::GetTransitionChainV2::query_payload(&mut bytes)?;
            if hashes.is_empty() {
                let empty: <rpc::GetTransitionChainV2 as RpcMethod>::Response =
                    Some(mina_p2p_messages::list::List::new());
                dispatcher.push(P2pNetworkRpcAction::OutgoingResponse {
                    peer_id,
                    response: ResponseHeader { id },
                    data: crate::channels::rpc::response_bytes(empty).into(),
                });
                return Ok(QueryDispatch::Immediate);
            }
            for hash in hashes {
                let hash = v2::StateHash::from(v2::DataHashLibStateHashStableV1(hash));

                dispatcher.push(P2pChannelsRpcAction::RequestReceived {
                    peer_id,
                    id,
                    request: Box::new(P2pRpcRequest::Block(hash)),
                });
            }
        }
        (rpc::GetSomeInitialPeersV1ForV2::NAME, rpc::GetSomeInitialPeersV1ForV2::VERSION) => {
            let () = rpc::GetSomeInitialPeersV1ForV2::query_payload(&mut bytes)?;
            dispatcher.push(P2pChannelsRpcAction::RequestReceived {
                peer_id,
                id,
                request: Box::new(P2pRpcRequest::InitialPeers),
            });
        }
        (rpc::GetCompletedSnarksV1::NAME, rpc::GetCompletedSnarksV1::VERSION) => {
            rpc::GetCompletedSnarksV1::query_payload(&mut bytes)?;
            dispatcher.push(P2pChannelsRpcAction::RequestReceived {
                peer_id,
                id,
                request: Box::new(P2pRpcRequest::CompletedSnarks),
            });
        }
        // Everything else, including RPCs on the menu that we don't serve
        // yet, such as `get_ancestry`, gets the error async_rpc_kernel answers
        // a tag and version it has no implementation for. The query was read
        // whole, so the stream stays usable, and the peer can ask another
        // node; disconnecting would cost every RPC it has with us.
        _ => {
            dispatcher.push(P2pNetworkRpcAction::OutgoingResponse {
                peer_id,
                response: ResponseHeader { id },
                data: unimplemented_response_data(tag, *version),
            });
            return Ok(QueryDispatch::Unimplemented);
        }
    }
    Ok(QueryDispatch::Request)
}

/// The answer to `__Versioned_rpc.Menu`: OCaml 4.0.2's menu
/// ([`rpc::VERSIONED_RPC_MENU`]).
fn menu_response_data() -> Data {
    type Payload = ResponsePayload<<rpc::VersionedRpcMenuV1 as RpcMethod>::Response>;
    let payload: Payload = RpcResult(Ok(NeedsLength(rpc::VersionedRpcMenuV1::menu())));
    let mut data = Vec::new();
    payload.binprot_write(&mut data).unwrap_or_default();
    data.into()
}

/// The answer to a query we have no implementation for.
fn unimplemented_response_data(tag: &BinprotTag, version: Ver) -> Data {
    let payload: ResponsePayload<()> =
        RpcResult(Err(RpcError::unimplemented_rpc(tag.clone(), version)));
    let mut data = Vec::new();
    payload.binprot_write(&mut data).unwrap_or_default();
    data.into()
}

/// Reads an answer's payload. A peer that answered with an error, for example
/// OCaml's rate limiter's exception (`Uncaught_exn`), failed this request but
/// kept to the protocol, so the error is returned as `Ok(Err(..))`, to be
/// passed on as no answer. Only an answer we can't decode is an `Err`.
fn read_response<M: RpcMethod>(
    bytes: &mut &[u8],
) -> Result<Result<M::Response, String>, RpcResponseReadError> {
    match M::response_payload(bytes) {
        Ok(response) => Ok(Ok(response)),
        Err(RpcResponseReadError::Failure { rpc_id, error }) => {
            Ok(Err(format!("{rpc_id}: {error}")))
        }
        Err(e) => Err(e),
    }
}

/// Passes a response on to the RPC channel. Returns the error the peer
/// answered with, if it did: the request then gets no answer (`None`), and
/// whoever made it retries elsewhere, as for any answer without the data.
fn dispatch_rpc_response<State, Action>(
    peer_id: PeerId,
    QueryHeader { tag, version, id }: &QueryHeader,
    mut bytes: &[u8],
    dispatcher: &mut Dispatcher<Action, State>,
) -> Result<Option<String>, RpcResponseReadError>
where
    State: crate::P2pStateTrait,
    Action: crate::P2pActionTrait<State>,
{
    let id = *id;
    let no_answer = |dispatcher: &mut Dispatcher<Action, State>| {
        dispatcher.push(P2pChannelsRpcAction::ResponseReceived {
            peer_id,
            id,
            response: None,
        });
    };
    let answer = |dispatcher: &mut Dispatcher<Action, State>, response: P2pRpcResponse| {
        dispatcher.push(P2pChannelsRpcAction::ResponseReceived {
            peer_id,
            id,
            response: Some(Box::new(response)),
        });
    };
    let error = match (tag.as_ref(), *version) {
        (rpc::GetCompletedSnarksV1::NAME, rpc::GetCompletedSnarksV1::VERSION) => {
            match read_response::<rpc::GetCompletedSnarksV1>(&mut bytes)? {
                Ok(Some(works)) => {
                    answer(dispatcher, P2pRpcResponse::CompletedSnarks(works));
                    None
                }
                Ok(None) => {
                    no_answer(dispatcher);
                    None
                }
                Err(error) => Some(error),
            }
        }
        (rpc::GetBestTipV2::NAME, rpc::GetBestTipV2::VERSION) => {
            match read_response::<rpc::GetBestTipV2>(&mut bytes)? {
                Ok(Some(resp)) => {
                    let response = BestTipWithProof {
                        best_tip: resp.data.into(),
                        proof: (resp.proof.0, resp.proof.1.into()),
                    };
                    answer(dispatcher, P2pRpcResponse::BestTipWithProof(response));
                    None
                }
                Ok(None) => {
                    no_answer(dispatcher);
                    None
                }
                Err(error) => Some(error),
            }
        }
        (rpc::AnswerSyncLedgerQueryV2::NAME, rpc::AnswerSyncLedgerQueryV2::VERSION) => {
            // The answer can also be an error of the query itself, such as a
            // ledger the peer doesn't have.
            let answer_or_error = read_response::<rpc::AnswerSyncLedgerQueryV2>(&mut bytes)?
                .and_then(|answer| {
                    Result::from(answer)
                        .map_err(|e| format!("{}: {e}", rpc::AnswerSyncLedgerQueryV2::rpc_id()))
                });
            match answer_or_error {
                Ok(response) => {
                    answer(dispatcher, P2pRpcResponse::LedgerQuery(response));
                    None
                }
                Err(error) => Some(error),
            }
        }
        (
            rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::NAME,
            rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::VERSION,
        ) => {
            match read_response::<rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2>(&mut bytes)? {
                Ok(Some((scan_state, hash, pending_coinbase, needed_blocks))) => {
                    let staged_ledger_hash = v2::MinaBaseLedgerHash0StableV1(hash).into();
                    let parts = Arc::new(StagedLedgerAuxAndPendingCoinbases {
                        scan_state,
                        staged_ledger_hash,
                        pending_coinbase,
                        needed_blocks,
                    });
                    answer(
                        dispatcher,
                        P2pRpcResponse::StagedLedgerAuxAndPendingCoinbasesAtBlock(parts),
                    );
                    None
                }
                Ok(None) => {
                    no_answer(dispatcher);
                    None
                }
                Err(error) => Some(error),
            }
        }
        (rpc::GetTransitionChainV2::NAME, rpc::GetTransitionChainV2::VERSION) => {
            match read_response::<rpc::GetTransitionChainV2>(&mut bytes)? {
                Ok(Some(blocks)) if !blocks.is_empty() => {
                    for block in blocks {
                        answer(dispatcher, P2pRpcResponse::Block(Arc::new(block)));
                    }
                    None
                }
                Ok(_) => {
                    no_answer(dispatcher);
                    None
                }
                Err(error) => Some(error),
            }
        }
        (rpc::GetSomeInitialPeersV1ForV2::NAME, rpc::GetSomeInitialPeersV1ForV2::VERSION) => {
            match read_response::<rpc::GetSomeInitialPeersV1ForV2>(&mut bytes)? {
                Ok(peers) if !peers.is_empty() => {
                    let peers = peers
                        .into_iter()
                        .filter_map(P2pConnectionOutgoingInitOpts::try_from_mina_rpc)
                        .collect();
                    answer(dispatcher, P2pRpcResponse::InitialPeers(peers));
                    None
                }
                Ok(_) => {
                    no_answer(dispatcher);
                    None
                }
                Err(error) => Some(error),
            }
        }
        _ => None,
    };
    if error.is_some() {
        no_answer(dispatcher);
    }
    Ok(error)
}

#[cfg(test)]
mod block_answer_limit_tests {
    use super::*;
    use crate::{
        network::pubsub::{pb, MAX_RPC_SIZE, TOPIC},
        ConnectionAddr,
    };
    use prost::Message as _;

    const ID: u64 = 7;

    /// Our RPC stream to a peer, waiting for the answer to a `tag` query.
    fn waiting_for(tag: &'static [u8]) -> P2pNetworkRpcState {
        let addr = ConnectionAddr {
            sock_addr: "127.0.0.1:8302".parse().unwrap(),
            incoming: false,
        };
        let mut rpc = P2pNetworkRpcState::new(addr, 1);
        rpc.pending = Some(QueryHeader {
            tag: tag.into(),
            version: 2,
            id: ID,
        });
        rpc
    }

    /// An answer with `payload` as it comes on the stream, length prefix
    /// first.
    fn answer(payload: Vec<u8>) -> Vec<u8> {
        RpcMessage::Response {
            header: ResponseHeader { id: ID },
            bytes: payload.into(),
        }
        .into_bytes()
    }

    /// An answer whose length prefix declares `len`.
    fn answer_of_len(len: usize) -> Vec<u8> {
        let framing = answer(vec![]).len() - 8;
        let message = answer(vec![0; len - framing]);
        assert_eq!(message.len(), 8 + len);
        message
    }

    /// The length an answer's prefix declares, which the limits check.
    fn declared(message: &[u8]) -> usize {
        u64::from_le_bytes(message[..8].try_into().unwrap()) as usize
    }

    /// A `get_transition_chain` answer payload of one block of `block` bytes.
    fn chain_answer(block: Vec<u8>) -> Vec<u8> {
        let mut list = vec![1, 1]; // `Some`, a list of one
        list.extend_from_slice(&block);
        let mut payload = vec![0]; // `Ok`
        binprot::Nat0(list.len() as u64)
            .binprot_write(&mut payload)
            .unwrap();
        payload.extend_from_slice(&list);
        payload
    }

    fn heavy_block_payload() -> Vec<u8> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../p2p-messages/tests/files/mesa/get_transition_chain_v2_590141_heavy.bin"
        );
        std::fs::read(path).unwrap()
    }

    /// Feeds `message`, or its start, to `rpc` in pieces of `piece` bytes,
    /// the first piece ending inside the length prefix, and checks after each
    /// piece that the buffer never holds more than the message declares.
    fn feed(
        rpc: &mut P2pNetworkRpcState,
        message: &[u8],
        piece: usize,
        budgeted_elsewhere: usize,
    ) -> Vec<TakenIncoming> {
        let limits = P2pLimits::default();
        let bound = 8 + declared(message);
        let mut taken = vec![rpc.take_incoming(&message[..3], &limits, budgeted_elsewhere)];
        for chunk in message[3..].chunks(piece) {
            taken.push(rpc.take_incoming(chunk, &limits, budgeted_elsewhere));
            assert!(rpc.buffer.capacity() <= bound);
        }
        taken
    }

    fn the_response(rpc: &mut P2pNetworkRpcState) -> Data {
        assert_eq!(rpc.incoming.len(), 1);
        match rpc.incoming.pop_front() {
            Some(RpcMessage::Response { header, bytes }) => {
                assert_eq!(header.id, ID);
                bytes
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }

    /// Devnet block 590141 (12 zkApp commands with 15 proofs each), as OCaml
    /// answers `get_transition_chain` for it: 3.7 MB, which the old 3.5 MB
    /// limit stopped, is read in pieces as OCaml's helper sends them.
    #[test]
    fn heavy_devnet_block_answer_is_read() {
        let payload = heavy_block_payload();
        let message = answer(payload.clone());
        assert!(declared(&message) > P2pLimits::RPC_BLOCK_ANSWER_UNBUDGETED);
        for tag in [rpc::GetTransitionChainV2::NAME, rpc::GetBestTipV2::NAME] {
            let mut rpc = waiting_for(tag);
            let taken = feed(&mut rpc, &message, 64 * 1024, 0);
            assert!(taken
                .iter()
                .all(|t| t.error.is_none() && t.deferred.is_none()));
            assert_eq!(&*the_response(&mut rpc).0, &payload[..]);
        }
    }

    /// The largest answer, sent in 64 KiB pieces, holds the budget while it
    /// arrives and a buffer of exactly its length when its last byte is
    /// missing, not the 64 MiB that doubling would take. Once read, the
    /// buffer and the budget are released.
    #[test]
    fn fragmented_largest_answer_holds_its_length() {
        let len = P2pLimits::RPC_BLOCK_ANSWER_MAX;
        let message = answer_of_len(len);
        let mut rpc = waiting_for(rpc::GetTransitionChainV2::NAME);
        let (head, last) = message.split_at(message.len() - 1);
        let taken = feed(&mut rpc, head, 64 * 1024, 0);
        assert!(taken
            .iter()
            .all(|t| t.error.is_none() && t.deferred.is_none()));
        assert_eq!(rpc.buffer.len(), 8 + len - 1);
        assert_eq!(rpc.buffer.capacity(), 8 + len);
        assert_eq!(rpc.incoming_budgeted, len);
        assert!(rpc.incoming.is_empty());

        let taken = rpc.take_incoming(last, &P2pLimits::default(), 0);
        assert!(taken.error.is_none() && taken.deferred.is_none());
        assert_eq!(
            the_response(&mut rpc).0.len(),
            len - (answer(vec![]).len() - 8)
        );
        assert_eq!(rpc.buffer.capacity(), 0);
        assert_eq!(rpc.incoming_budgeted, 0);
        assert_eq!(rpc.incoming_len, None);
    }

    /// A length prefix over the limit stops the stream before anything past
    /// the prefix is kept.
    #[test]
    fn oversized_length_prefix_stops_the_stream() {
        let mut message = (P2pLimits::RPC_BLOCK_ANSWER_MAX as u64 + 1)
            .to_le_bytes()
            .to_vec();
        message.extend_from_slice(&[0; 1024]);
        let mut rpc = waiting_for(rpc::GetTransitionChainV2::NAME);
        let taken = rpc.take_incoming(&message, &P2pLimits::default(), 0);
        assert!(matches!(taken.error, Some(P2pNetworkRpcError::Limit(..))));
        assert!(rpc.error.is_some());
        assert_eq!(rpc.buffer.len(), 8);
        assert_eq!(rpc.incoming_budgeted, 0);
    }

    /// With the budget taken, a large block answer is discarded as it
    /// arrives, its query is handed back to be answered as failed, and the
    /// stream reads what follows the answer as before.
    #[test]
    fn answer_over_the_budget_is_discarded_and_its_query_returned() {
        let message = answer(chain_answer(vec![0; 4_000_000]));
        let len = declared(&message);
        let full = P2pLimits::RPC_LARGE_BLOCK_ANSWERS_BUDGET - len + 1;
        let mut rpc = waiting_for(rpc::GetTransitionChainV2::NAME);
        let taken = feed(&mut rpc, &message, 64 * 1024, full);
        let deferred: Vec<_> = taken.into_iter().filter_map(|t| t.deferred).collect();
        assert_eq!(deferred.len(), 1);
        assert_eq!((deferred[0].0.id, deferred[0].1), (ID, len));
        assert!(rpc.pending.is_none());
        assert!(rpc.error.is_none());
        assert_eq!(rpc.buffer.capacity(), 0);
        assert_eq!(rpc.skip_remaining, 0);
        assert_eq!(rpc.incoming_budgeted, 0);
        assert!(rpc.incoming.is_empty());

        // A heartbeat after the discarded answer, in the same piece as its
        // last bytes, is read.
        let mut rpc = waiting_for(rpc::GetTransitionChainV2::NAME);
        let mut stream = message.clone();
        stream.extend_from_slice(&RpcMessage::Heartbeat.into_bytes());
        rpc.take_incoming(&stream[..100], &P2pLimits::default(), full);
        let taken = rpc.take_incoming(&stream[100..], &P2pLimits::default(), full);
        assert!(taken.error.is_none());
        assert!(matches!(
            rpc.incoming.pop_front(),
            Some(RpcMessage::Heartbeat)
        ));

        // Just within the budget, the same answer is read.
        let mut rpc = waiting_for(rpc::GetTransitionChainV2::NAME);
        let taken = feed(&mut rpc, &message, 64 * 1024, full - 1);
        assert!(taken.iter().all(|t| t.deferred.is_none()));
        assert_eq!(
            the_response(&mut rpc).0.len(),
            len - (answer(vec![]).len() - 8)
        );
    }

    /// A new query on the stream discards the rest of a large answer still
    /// arriving, releasing its budget, and the answer after it is read.
    #[test]
    fn new_query_abandons_a_large_answer_in_progress() {
        let message = answer(chain_answer(vec![0; 4_000_000]));
        let mut rpc = waiting_for(rpc::GetTransitionChainV2::NAME);
        let (head, rest) = message.split_at(1_000_000);
        feed(&mut rpc, head, 64 * 1024, 0);
        assert_eq!(rpc.incoming_budgeted, declared(&message));

        rpc.abandon_budgeted_answer();
        assert_eq!(rpc.incoming_budgeted, 0);
        assert_eq!(rpc.buffer.capacity(), 0);
        assert_eq!(rpc.skip_remaining, rest.len());

        let next = answer(chain_answer(vec![1; 100]));
        let mut stream = rest.to_vec();
        stream.extend_from_slice(&next);
        let taken = rpc.take_incoming(&stream, &P2pLimits::default(), 0);
        assert!(taken.error.is_none() && taken.deferred.is_none());
        assert_eq!(the_response(&mut rpc).0.len(), declared(&next) - 2);
    }

    /// Block answers up to the old limit, and other answers, don't use the
    /// budget and are read when it is taken.
    #[test]
    fn small_and_other_answers_ignore_the_budget() {
        let full = P2pLimits::RPC_LARGE_BLOCK_ANSWERS_BUDGET;
        let message = answer_of_len(P2pLimits::RPC_BLOCK_ANSWER_UNBUDGETED);
        let mut rpc = waiting_for(rpc::GetTransitionChainV2::NAME);
        let taken = feed(&mut rpc, &message, 64 * 1024, full);
        assert!(taken.iter().all(|t| t.deferred.is_none()));
        the_response(&mut rpc);

        let message = answer_of_len(8 * 1024 * 1024);
        let mut rpc = waiting_for(rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::NAME);
        let taken = feed(&mut rpc, &message, 64 * 1024, full);
        assert!(taken.iter().all(|t| t.deferred.is_none()));
        assert_eq!(rpc.incoming_budgeted, 0);
        the_response(&mut rpc);
    }

    /// The largest block a gossip message can carry, with the least framing a
    /// gossip message has (no signature, no sender), fits in one answer.
    #[test]
    fn largest_gossiped_block_fits_in_one_answer() {
        let gossip_len = |data_len: usize| {
            pb::Rpc {
                publish: vec![pb::Message {
                    data: Some(vec![0; data_len]),
                    topic: TOPIC.to_owned(),
                    ..Default::default()
                }],
                ..Default::default()
            }
            .encoded_len()
        };
        let framing = gossip_len(MAX_RPC_SIZE) - MAX_RPC_SIZE;
        // The gossip data is the message tag and the block.
        let block_len = MAX_RPC_SIZE - framing - 1;
        assert!(gossip_len(1 + block_len) <= MAX_RPC_SIZE);

        let len = declared(&answer(chain_answer(vec![0; block_len])));
        let limits = P2pLimits::default();
        let rpc = waiting_for(rpc::GetTransitionChainV2::NAME);
        rpc.check_rpc_limit(len, &limits).unwrap();
        assert!(rpc.check_rpc_limit(MAX_RPC_SIZE + 1, &limits).is_err());
    }
}
