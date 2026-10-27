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
        BestTipWithProof, CatchupRequest, P2pChannelsRpcAction, P2pRpcRequest, P2pRpcResponse,
        StagedLedgerAuxAndPendingCoinbases, MAX_CATCHUP_RPC_REQUESTS,
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
                let reserve = response_reservation(header);
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
                rpc_state.buffer.extend_from_slice(&data);
                let mut offset = 0;
                // TODO(akoptelov): there shouldn't be the case where we have multiple incoming messages at once (or at least other than heartbeat)
                loop {
                    let Some(buf) = &rpc_state.buffer.get(offset..) else {
                        bug_condition!("Invalid range `buffer[{offset}..]`");
                        return Ok(());
                    };
                    if let Some(len_bytes) = buf.get(..8).and_then(|s| s.try_into().ok()) {
                        let len = u64::from_le_bytes(len_bytes) as usize;
                        if let Err(err) = rpc_state.check_rpc_limit(len, limits) {
                            // The stream takes no more actions from here on.
                            mina_core::warn!(meta.time();
                                kind = "P2pNetworkRpcLimitExceeded",
                                summary = format!("RPC stream stopped: {err}"),
                                peer_id = display(peer_id),
                            );
                            rpc_state.error = Some(err);
                            break;
                        }
                        if let Some(mut slice) = buf.get(8..(8 + len)) {
                            offset += 8 + len;
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
                                    warn!(meta.time(); summary = "unreadable RPC message", peer_id = display(&peer_id), error = display(&err));
                                    rpc_state.error = Some(err);
                                    continue;
                                }
                            };
                            rpc_state.incoming.push_back(msg);
                            continue;
                        }
                    }

                    if offset != 0 {
                        let Some(buf) = rpc_state.buffer.get(offset..) else {
                            bug_condition!("Invalid range `buffer[{offset}..]`");
                            return Ok(());
                        };
                        rpc_state.buffer = buf.to_vec();
                    }
                    break;
                }

                let incoming = rpc_state.incoming.front().cloned();
                let dispatcher = state_context.into_dispatcher();

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
                                    response_reservation(&wire_header)
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
        }
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

/// Reserves work before dispatch. Staged-ledger answers observed in D4 were
/// about 59 MiB; unexpectedly larger answers meet the actual Yamux queue cap.
fn response_reservation(header: &QueryHeader) -> usize {
    match header.tag.as_ref() {
        rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2::NAME => 64 * 1024 * 1024,
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
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/gossip_net/libp2p.ml#L159-L165>
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
