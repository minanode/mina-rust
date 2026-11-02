use super::{
    P2pChannelsRpcAction, P2pChannelsRpcState, P2pRpcLocalState, P2pRpcPipeline, P2pRpcPipelined,
    P2pRpcRemotePendingRequestState, P2pRpcRemoteState, P2pRpcRequest, P2pRpcResponse,
    RpcChannelMsg, MAX_P2P_RPC_REMOTE_CONCURRENT_REQUESTS,
};
use crate::{
    channels::{ChannelId, ChannelMsg, MsgId, P2pChannelsEffectfulAction},
    disconnection::{P2pDisconnectionAction, P2pDisconnectionReason},
    P2pNetworkRpcAction, P2pPeerAction, P2pState,
};
use mina_core::{block::BlockWithHash, bug_condition, error, Substate};
use redux::ActionWithMeta;
use std::collections::VecDeque;

impl P2pChannelsRpcState {
    /// Substate is accessed
    pub fn reducer<Action, State>(
        mut state_context: Substate<Action, State, P2pState>,
        action: ActionWithMeta<P2pChannelsRpcAction>,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let (action, meta) = action.split();
        let p2p_state = state_context.get_substate_mut()?;
        let peer_id = *action.peer_id();
        let is_libp2p = p2p_state.is_libp2p_peer(&peer_id);
        if is_libp2p {
            if let P2pChannelsRpcAction::RequestSend { request, .. } = &action {
                if matches!(request.as_ref(), P2pRpcRequest::CompletedSnarks) {
                    let ip = p2p_state.libp2p_peer_ip(&peer_id);
                    p2p_state
                        .completed_snarks_requests
                        .record(peer_id, ip, meta.time());
                    mina_core::debug!(meta.time(); kind = "P2pCompletedSnarksRequestSent",
                        peer_id = display(peer_id), ip = debug(ip),
                        requests = p2p_state.completed_snarks_requests.sent);
                }
                if matches!(request.as_ref(), P2pRpcRequest::BestTipWithProof) {
                    let ip = p2p_state.libp2p_peer_ip(&peer_id);
                    p2p_state.best_tip_requests.record(peer_id, ip, meta.time());
                    mina_core::debug!(meta.time();
                        kind = "P2pBestTipRequestSent",
                        summary = "best tip requested, within the peer's budget",
                        peer_id = display(peer_id),
                        ip = debug(ip),
                        in_window = p2p_state
                            .best_tip_requests
                            .peer_requests_in_window(&peer_id, meta.time()),
                    );
                }
            }
        }
        // A sharing browser serves bootstraps to at most `max_bootstraps`
        // browsers at once, counted from the requests it serves
        // (crate::sharing); beyond that, an empty answer.
        let mut refused = false;
        if let P2pChannelsRpcAction::RequestReceived { request, .. } = &action {
            if crate::sharing::is_bootstrap_request(request) {
                refused = !p2p_state.sharing_serves(&peer_id, meta.time());
                if !refused {
                    p2p_state.sharing_record_served(peer_id, meta.time());
                }
            }
        }
        // OCaml serving budgets are charged only on the libp2p wire path.
        // WebRTC keeps its existing per-peer pending-request cap.
        let peer_state = &mut p2p_state
            .get_ready_peer_mut(&peer_id)
            .ok_or_else(|| format!("Peer state not found for: {action:?}"))?
            .channels;

        let next_local_rpc_id = &mut peer_state.next_local_rpc_id;
        let rpc_state = &mut peer_state.rpc;
        let pipeline = &mut peer_state.rpc_pipeline;
        let sync_stats = &mut peer_state.sync_stats;

        match action {
            P2pChannelsRpcAction::Init { .. } => {
                *rpc_state = Self::Init { time: meta.time() };

                let dispatcher = state_context.into_dispatcher();

                dispatcher.push(P2pChannelsEffectfulAction::InitChannel {
                    peer_id,
                    id: ChannelId::Rpc,
                    on_success: redux::callback!(
                        on_rpc_channel_init(peer_id: crate::PeerId) -> crate::P2pAction {
                            P2pChannelsRpcAction::Pending { peer_id }
                        }
                    ),
                });
                Ok(())
            }
            P2pChannelsRpcAction::Pending { .. } => {
                *rpc_state = Self::Pending { time: meta.time() };
                Ok(())
            }
            P2pChannelsRpcAction::Ready { .. } => {
                *rpc_state = Self::Ready {
                    time: meta.time(),
                    local: P2pRpcLocalState::WaitingForRequest { time: meta.time() },
                    remote: P2pRpcRemoteState {
                        pending_requests: VecDeque::with_capacity(
                            MAX_P2P_RPC_REMOTE_CONCURRENT_REQUESTS,
                        ),
                        last_responded: redux::Timestamp::ZERO,
                    },
                };
                *pipeline = P2pRpcPipeline::for_peer(is_libp2p);

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let p2p_state: &P2pState = state.substate()?;

                if let Some(callback) = &p2p_state.callbacks.on_p2p_channels_rpc_ready {
                    dispatcher.push_callback(callback.clone(), peer_id);
                }
                Ok(())
            }
            P2pChannelsRpcAction::RequestSend {
                id,
                request,
                on_init,
                ..
            } => {
                let Self::Ready { local, .. } = rpc_state else {
                    bug_condition!(
                        "Invalid state for `P2pChannelsRpcAction::RequestSend`, state: {:?}",
                        rpc_state
                    );
                    return Ok(());
                };
                *next_local_rpc_id += 1;
                if local.can_send_request() {
                    *local = P2pRpcLocalState::Requested {
                        time: meta.time(),
                        id,
                        request: request.clone(),
                    };
                } else {
                    // The slot is taken: a sync request to a WebRTC peer
                    // (enabling condition, `P2pRpcPipeline::has_room`).
                    pipeline.requests.push(P2pRpcPipelined {
                        sent: meta.time(),
                        id,
                        request: request.clone(),
                        timed_out: None,
                    });
                }

                let dispatcher = state_context.into_dispatcher();

                #[cfg(feature = "p2p-libp2p")]
                if is_libp2p {
                    if let Some((query, data)) =
                        super::libp2p::internal_request_into_libp2p(*request.clone(), id)
                    {
                        dispatcher.push(P2pNetworkRpcAction::OutgoingQuery {
                            peer_id,
                            query,
                            data,
                        });
                    }
                    if let Some(on_init) = on_init {
                        dispatcher.push_callback(on_init, (peer_id, id, *request));
                    }

                    return Ok(());
                }

                dispatcher.push(P2pChannelsEffectfulAction::MessageSend {
                    peer_id,
                    msg_id: MsgId::first(),
                    msg: ChannelMsg::Rpc(RpcChannelMsg::Request(id, *request.clone())),
                });

                if let Some(callback) = on_init {
                    dispatcher.push_callback(callback, (peer_id, id, *request));
                }
                Ok(())
            }
            P2pChannelsRpcAction::Timeout { id, .. } => {
                let Self::Ready { local, .. } = rpc_state else {
                    bug_condition!(
                        "Invalid state for `P2pChannelsRpcAction::Timeout`, state: {:?}",
                        rpc_state
                    );
                    return Ok(());
                };
                // A pipelined request times out as the slot's does: retried
                // elsewhere the first time, the peer dropped the second.
                if let Some(pipelined) = pipeline.get_mut(id) {
                    if pipelined.timed_out.is_some() {
                        let kind = pipelined.request.kind();
                        let dispatcher = state_context.into_dispatcher();
                        dispatcher.push(P2pDisconnectionAction::Init {
                            peer_id,
                            reason: P2pDisconnectionReason::TransitionFrontierRpcTimeout(kind),
                        });
                        return Ok(());
                    }
                    mina_core::warn!(meta.time();
                        kind = "P2pChannelsRpcTimeout",
                        summary = format!(
                            "{:?} request timed out, retrying it elsewhere",
                            pipelined.request.kind()
                        ),
                        peer_id = display(peer_id),
                        pipelined = true,
                    );
                    // Its send time stays: the second timeout counts from it.
                    pipelined.timed_out = Some(meta.time());
                    sync_stats.record_timeout(meta.time());

                    let (dispatcher, state) = state_context.into_dispatcher_and_state();
                    let p2p_state: &P2pState = state.substate()?;
                    if let Some(callback) = &p2p_state.callbacks.on_p2p_channels_rpc_timeout {
                        dispatcher.push_callback(callback.clone(), (peer_id, id));
                    }
                    return Ok(());
                }
                match local {
                    // The first timeout: the request is given up on and
                    // retried elsewhere, but the peer is kept, as a slow
                    // answer is no reason to lose a peer.
                    P2pRpcLocalState::Requested {
                        time: sent,
                        request,
                        ..
                    } => {
                        mina_core::warn!(meta.time();
                            kind = "P2pChannelsRpcTimeout",
                            summary = format!(
                                "{:?} request timed out, retrying it elsewhere",
                                request.kind()
                            ),
                            peer_id = display(peer_id),
                        );
                        if !is_libp2p && P2pRpcPipeline::takes(request) {
                            sync_stats.record_timeout(meta.time());
                        }
                        *local = P2pRpcLocalState::TimedOut {
                            time: meta.time(),
                            sent: *sent,
                            id,
                            request: std::mem::take(request),
                        };

                        let (dispatcher, state) = state_context.into_dispatcher_and_state();
                        let p2p_state: &P2pState = state.substate()?;

                        if let Some(callback) = &p2p_state.callbacks.on_p2p_channels_rpc_timeout {
                            dispatcher.push_callback(callback.clone(), (peer_id, id));
                        }
                    }
                    // Still no answer after a second timeout: the peer is
                    // gone or stuck.
                    P2pRpcLocalState::TimedOut { request, .. } => {
                        let kind = request.kind();
                        let dispatcher = state_context.into_dispatcher();
                        dispatcher.push(P2pDisconnectionAction::Init {
                            peer_id,
                            reason: P2pDisconnectionReason::TransitionFrontierRpcTimeout(kind),
                        });
                    }
                    _ => {
                        bug_condition!(
                            "Invalid state for `P2pChannelsRpcAction::Timeout`, state: {:?}",
                            rpc_state
                        );
                    }
                }

                Ok(())
            }
            P2pChannelsRpcAction::ResponseReceived {
                response,
                id: rpc_id,
                ..
            } => {
                let Self::Ready { local, .. } = rpc_state else {
                    bug_condition!(
                        "Invalid state for `P2pChannelsRpcAction::ResponseReceived`, state: {:?}",
                        rpc_state
                    );
                    return Ok(());
                };
                // A WebRTC peer's answer to a ledger query or block, of the
                // kind asked for, times the peer for the sync
                // (`plan_sync_peers`). An empty answer, such as a sharing
                // browser's refusal, an error or another kind doesn't
                // (`sync_answer_matches`).
                let ranked_sent = |sent: redux::Timestamp, request: &P2pRpcRequest| {
                    (!is_libp2p
                        && response
                            .as_deref()
                            .is_some_and(|r| super::sync_answer_matches(request, r)))
                    .then_some(sent)
                };
                let answered_sent = if let Some(pipelined) = pipeline.get(rpc_id) {
                    let answered_sent = ranked_sent(pipelined.sent, &pipelined.request);
                    pipeline.answer(rpc_id);
                    answered_sent
                } else {
                    let (P2pRpcLocalState::Requested {
                        id,
                        request,
                        time: sent,
                    }
                    | P2pRpcLocalState::TimedOut {
                        id, request, sent, ..
                    }) = local
                    else {
                        bug_condition!(
                            "Invalid state for `P2pChannelsRpcAction::ResponseReceived`, state: {:?}",
                            rpc_state
                        );
                        return Ok(());
                    };
                    let answered_sent = ranked_sent(*sent, request);
                    *local = P2pRpcLocalState::Responded {
                        time: meta.time(),
                        id: *id,
                        request: std::mem::take(request),
                    };
                    answered_sent
                };
                if let Some(sent) = answered_sent {
                    sync_stats.record_answer(sent, meta.time());
                }

                if !is_libp2p {
                    if let Some(P2pRpcResponse::Transaction(tx)) = response.as_deref() {
                        if let Ok(hash) = tx.hash() {
                            mina_core::debug!(meta.time(); kind = "WebRtcTransactionBodyReceived",
                                peer_id = display(peer_id), rpc_id = display(rpc_id), hash = display(hash));
                        }
                    }
                }

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let p2p_state: &P2pState = state.substate()?;

                if let Some(P2pRpcResponse::BestTipWithProof(resp)) = response.as_deref() {
                    let Ok(best_tip) = BlockWithHash::try_new(resp.best_tip.clone()) else {
                        error!(meta.time(); "P2pChannelsRpcAction::ResponseReceived: Invalid bigint in block");
                        return Ok(());
                    };

                    dispatcher.push(P2pPeerAction::BestTipUpdate { peer_id, best_tip });
                }

                if let Some(callback) = &p2p_state.callbacks.on_p2p_channels_rpc_response_received {
                    dispatcher.push_callback(callback.clone(), (peer_id, rpc_id, response));
                }
                Ok(())
            }
            P2pChannelsRpcAction::RequestReceived { id, request, .. } => {
                let Self::Ready { remote, .. } = rpc_state else {
                    bug_condition!(
                        "Invalid state for `P2pChannelsRpcAction::RequestReceived`, state: {:?}",
                        rpc_state
                    );
                    return Ok(());
                };
                remote
                    .pending_requests
                    .push_back(P2pRpcRemotePendingRequestState {
                        time: meta.time(),
                        id,
                        request: *request.clone(),
                        is_pending: false,
                    });

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let p2p_state: &P2pState = state.substate()?;

                let invalid_query = match request.as_ref() {
                    P2pRpcRequest::LedgerQuery(hash, query) => super::validate_ledger_query(
                        query,
                        mina_core::constants::constraint_constants().ledger_depth as usize,
                    )
                    .err()
                    .map(|error| {
                        Box::new(P2pRpcResponse::LedgerQueryError(hash.clone(), error.into()))
                    }),
                    _ => None,
                };
                if invalid_query.is_some() || refused {
                    if refused {
                        mina_core::debug!(meta.time();
                            kind = "P2pSharingBootstrapRefused",
                            summary = "serving two bootstraps already",
                            peer_id = display(peer_id),
                        );
                    }
                    dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                        peer_id,
                        id,
                        response: invalid_query,
                    });
                    return Ok(());
                }
                if let Some(callback) = &p2p_state.callbacks.on_p2p_channels_rpc_request_received {
                    dispatcher.push_callback(callback.clone(), (peer_id, id, request));
                }
                Ok(())
            }
            P2pChannelsRpcAction::ResponsePending { id, .. } => {
                let Self::Ready { remote, .. } = rpc_state else {
                    bug_condition!(
                        "Invalid state for `P2pChannelsRpcAction::ResponsePending`, state: {:?}",
                        rpc_state
                    );
                    return Ok(());
                };
                if let Some(req) = remote.pending_requests.iter_mut().find(|r| r.id == id) {
                    req.is_pending = true;
                }
                Ok(())
            }
            P2pChannelsRpcAction::SyncAnswerRejected { .. } => {
                sync_stats.record_rejected(meta.time());
                Ok(())
            }
            P2pChannelsRpcAction::ResponseSend { id, response, .. } => {
                let Self::Ready { remote, .. } = rpc_state else {
                    bug_condition!(
                        "Invalid state for `P2pChannelsRpcAction::ResponseSend`, state: {:?}",
                        rpc_state
                    );
                    return Ok(());
                };

                let request = remote
                    .pending_requests
                    .iter()
                    .position(|r| r.id == id)
                    .and_then(|pos| remote.pending_requests.remove(pos))
                    .map(|r| r.request);
                if request.is_some() {
                    remote.last_responded = meta.time();
                }

                let dispatcher = state_context.into_dispatcher();

                #[cfg(feature = "p2p-libp2p")]
                if is_libp2p {
                    let encoded = match response {
                        Some(response) => {
                            super::libp2p::internal_response_into_libp2p(*response, id)
                        }
                        None => request
                            .as_ref()
                            .and_then(|r| super::libp2p::unavailable_response_into_libp2p(r, id)),
                    }
                    .or_else(|| {
                        request
                            .as_ref()
                            .and_then(|r| super::libp2p::unavailable_response_into_libp2p(r, id))
                    });
                    if let Some((response, data)) = encoded {
                        dispatcher.push(P2pNetworkRpcAction::OutgoingResponse {
                            peer_id,
                            response,
                            data,
                        });
                    }

                    return Ok(());
                }

                let response = response
                    .filter(|r| !matches!(r.as_ref(), P2pRpcResponse::LedgerQueryError(..)));
                dispatcher.push(P2pChannelsEffectfulAction::MessageSend {
                    peer_id,
                    msg_id: MsgId::first(),
                    msg: ChannelMsg::Rpc(RpcChannelMsg::Response(id, response.map(|v| *v))),
                });
                Ok(())
            }
        }
    }
}
