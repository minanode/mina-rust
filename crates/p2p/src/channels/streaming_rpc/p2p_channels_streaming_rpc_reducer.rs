use mina_core::{bug_condition, Substate};
use redux::ActionWithMeta;

use crate::{
    channels::{ChannelId, ChannelMsg, MsgId, P2pChannelsEffectfulAction},
    P2pState, PeerId,
};

use super::{
    staged_ledger_parts::{
        parts_count, staged_ledger_parts_complete, take_staged_ledger_parts,
        StagedLedgerPartsFromLastProgress, StagedLedgerPartsReceiveProgress,
        StagedLedgerPartsSendProgress,
    },
    P2pChannelsStreamingRpcAction, P2pChannelsStreamingRpcState, P2pStreamingRpcId,
    P2pStreamingRpcLocalState, P2pStreamingRpcReceiveProgress, P2pStreamingRpcRemoteState,
    P2pStreamingRpcRequest, P2pStreamingRpcResponseFull, P2pStreamingRpcSendProgress,
    StreamingRpcChannelMsg,
};

impl P2pChannelsStreamingRpcState {
    pub fn reducer<Action, State>(
        mut state_context: Substate<Action, State, P2pState>,
        action: ActionWithMeta<P2pChannelsStreamingRpcAction>,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let (action, meta) = action.split();
        let peer_id = *action.peer_id();
        let p2p_state = state_context.get_substate_mut()?;

        // The staged ledger is a bootstrap's: a sharing browser serves it to
        // at most `max_bootstraps` browsers at once (crate::sharing); beyond
        // that, an empty answer.
        let mut refused = false;
        if let P2pChannelsStreamingRpcAction::RequestReceived { .. } = &action {
            refused = !p2p_state.sharing_serves(&peer_id, meta.time());
            if !refused {
                p2p_state.sharing_record_served(peer_id, meta.time());
            }
        }
        // This channel is WebRTC-only; OCaml serving budgets apply to libp2p.
        let channels_state = &mut p2p_state
            .get_ready_peer_mut(&peer_id)
            .ok_or_else(|| format!("Invalid state for: {action:?}"))?
            .channels;

        let next_local_rpc_id = &mut channels_state.next_local_rpc_id;
        let streaming_rpc_state = &mut channels_state.streaming_rpc;

        match action {
            P2pChannelsStreamingRpcAction::Init { .. } => {
                *streaming_rpc_state = Self::Init { time: meta.time() };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pChannelsEffectfulAction::InitChannel {
                    peer_id,
                    id: ChannelId::StreamingRpc,
                    on_success: redux::callback!(
                        on_streaming_rpc_channel_init(peer_id: crate::PeerId) -> crate::P2pAction {
                            P2pChannelsStreamingRpcAction::Pending { peer_id }
                        }
                    ),
                });
                Ok(())
            }
            P2pChannelsStreamingRpcAction::Pending { .. } => {
                *streaming_rpc_state = Self::Pending { time: meta.time() };
                Ok(())
            }
            P2pChannelsStreamingRpcAction::Ready { .. } => {
                *streaming_rpc_state = Self::Ready {
                    time: meta.time(),
                    local: P2pStreamingRpcLocalState::WaitingForRequest { time: meta.time() },
                    remote: P2pStreamingRpcRemoteState::WaitingForRequest { time: meta.time() },
                    remote_last_responded: redux::Timestamp::ZERO,
                };

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let p2p_state: &P2pState = state.substate()?;

                if let Some(callback) = &p2p_state.callbacks.on_p2p_channels_streaming_rpc_ready {
                    dispatcher.push_callback(callback.clone(), ());
                }
                Ok(())
            }
            P2pChannelsStreamingRpcAction::RequestSend {
                id,
                request,
                on_init,
                ..
            } => {
                let Self::Ready { local, .. } = streaming_rpc_state else {
                    bug_condition!(
                        "`P2pChannelsStreamingRpcAction::RequestSend` with state {:?}",
                        streaming_rpc_state
                    );
                    return Ok(());
                };

                *next_local_rpc_id += 1;
                *local = P2pStreamingRpcLocalState::Requested {
                    time: meta.time(),
                    id,
                    request: request.clone(),
                    progress: match &*request {
                        P2pStreamingRpcRequest::StagedLedgerParts(_) => {
                            Into::into(StagedLedgerPartsReceiveProgress::BasePending {
                                time: meta.time(),
                            })
                        }
                        P2pStreamingRpcRequest::StagedLedgerPartsFromLast(_) => {
                            StagedLedgerPartsFromLastProgress::new(meta.time()).into()
                        }
                    },
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pChannelsEffectfulAction::MessageSend {
                    peer_id,
                    msg_id: MsgId::first(),
                    msg: StreamingRpcChannelMsg::Request(id, *request.clone()).into(),
                });
                if let Some(callback) = on_init {
                    dispatcher.push_callback(callback, (peer_id, id, *request));
                }
                Ok(())
            }
            P2pChannelsStreamingRpcAction::Timeout { id, .. } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let p2p_state: &P2pState = state.substate()?;

                if let Some(callback) = &p2p_state.callbacks.on_p2p_channels_streaming_rpc_timeout {
                    dispatcher.push_callback(callback.clone(), (peer_id, id));
                }

                Ok(())
            }
            P2pChannelsStreamingRpcAction::ResponseNextPartGet { id, .. } => {
                let Self::Ready {
                    local: P2pStreamingRpcLocalState::Requested { progress, .. },
                    ..
                } = streaming_rpc_state
                else {
                    bug_condition!("{:?} with state {:?}", action, streaming_rpc_state);
                    return Ok(());
                };

                if !progress.set_next_pending(meta.time()) {
                    bug_condition!("progress state already pending: {progress:?}");
                }

                if !progress.is_part_pending() {
                    bug_condition!("progress state is not pending {:?}", progress);
                }

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pChannelsEffectfulAction::MessageSend {
                    peer_id,
                    msg_id: MsgId::first(),
                    msg: ChannelMsg::StreamingRpc(StreamingRpcChannelMsg::Next(id)),
                });
                Ok(())
            }
            P2pChannelsStreamingRpcAction::ResponsePartReceived { response, id, .. } => {
                let Self::Ready { local, .. } = streaming_rpc_state else {
                    bug_condition!(
                        "`P2pChannelsStreamingRpcAction::ResponsePartReceived` with state {:?}",
                        streaming_rpc_state
                    );
                    return Ok(());
                };
                if let P2pStreamingRpcLocalState::Cancelled { id, request, .. } = local {
                    // The part on its way when we cancelled: not needed.
                    *local = P2pStreamingRpcLocalState::Responded {
                        time: meta.time(),
                        id: *id,
                        request: std::mem::take(request),
                    };
                    return Ok(());
                }
                let P2pStreamingRpcLocalState::Requested { progress, .. } = local else {
                    bug_condition!(
                        "`P2pChannelsStreamingRpcAction::ResponsePartReceived` with state {:?}",
                        local
                    );
                    return Ok(());
                };
                if !progress.update(meta.time(), response) {
                    bug_condition!("progress response mismatch! {progress:?}");
                }
                let done = progress.is_done();

                // STAGED-SPLIT: another peer may be sending the same staged
                // ledger from the other end. Once the two have every part
                // between them, they are put together here and the other
                // is cancelled.
                let partner = staged_ledger_partner(p2p_state, &peer_id);
                let partner_cancelled = !done
                    && partner.as_ref().is_some_and(|(partner, _)| {
                        put_staged_ledger_together(p2p_state, &peer_id, partner, meta.time())
                    });

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let state: &P2pState = state.substate()?;
                let Some(peer) = state.get_ready_peer(&peer_id) else {
                    return Ok(());
                };

                if let Some(response) = peer.channels.streaming_rpc.local_done_response() {
                    match partner {
                        Some((partner, partner_rpc_id)) if partner_cancelled => {
                            dispatcher.push(P2pChannelsEffectfulAction::MessageSend {
                                peer_id: partner,
                                msg_id: MsgId::first(),
                                msg: StreamingRpcChannelMsg::Cancel(partner_rpc_id).into(),
                            });
                        }
                        Some((partner, partner_rpc_id)) => {
                            dispatcher.push(P2pChannelsStreamingRpcAction::RequestCancel {
                                peer_id: partner,
                                id: partner_rpc_id,
                            });
                        }
                        None => {}
                    }
                    dispatcher.push(P2pChannelsStreamingRpcAction::ResponseReceived {
                        peer_id,
                        id,
                        response: Some(response),
                    });
                    return Ok(());
                }
                dispatcher.push(P2pChannelsStreamingRpcAction::ResponseNextPartGet { peer_id, id });
                Ok(())
            }
            P2pChannelsStreamingRpcAction::ResponseReceived {
                id: rpc_id,
                response,
                ..
            } => {
                let Self::Ready { local, .. } = streaming_rpc_state else {
                    bug_condition!(
                        "`P2pChannelsStreamingRpcAction::ResponseReceived` with state {:?}",
                        streaming_rpc_state
                    );
                    return Ok(());
                };
                if let P2pStreamingRpcLocalState::Cancelled { id, request, .. } = local {
                    // The peer had nothing more to send when we cancelled.
                    *local = P2pStreamingRpcLocalState::Responded {
                        time: meta.time(),
                        id: *id,
                        request: std::mem::take(request),
                    };
                    return Ok(());
                }
                let P2pStreamingRpcLocalState::Requested { id, request, .. } = local else {
                    bug_condition!(
                        "`P2pChannelsStreamingRpcAction::ResponseReceived` with state {:?}",
                        streaming_rpc_state
                    );
                    return Ok(());
                };
                *local = P2pStreamingRpcLocalState::Responded {
                    time: meta.time(),
                    id: *id,
                    request: std::mem::take(request),
                };

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let p2p_state: &P2pState = state.substate()?;

                if let Some(callback) = &p2p_state
                    .callbacks
                    .on_p2p_channels_streaming_rpc_response_received
                {
                    dispatcher.push_callback(callback.clone(), (peer_id, rpc_id, response))
                }

                Ok(())
            }
            P2pChannelsStreamingRpcAction::RequestReceived { id, request, .. } => {
                let Self::Ready { remote, .. } = streaming_rpc_state else {
                    bug_condition!(
                        "`P2pChannelsStreamingRpcAction::RequestReceived` with state {:?}",
                        streaming_rpc_state
                    );
                    return Ok(());
                };
                *remote = P2pStreamingRpcRemoteState::Requested {
                    time: meta.time(),
                    id,
                    request,
                    progress: StagedLedgerPartsSendProgress::LedgerGetIdle { time: meta.time() }
                        .into(),
                };
                if refused {
                    mina_core::debug!(meta.time();
                        kind = "P2pSharingBootstrapRefused",
                        summary = "serving two bootstraps already",
                        peer_id = display(peer_id),
                    );
                    let dispatcher = state_context.into_dispatcher();
                    dispatcher.push(P2pChannelsStreamingRpcAction::ResponsePending { peer_id, id });
                    dispatcher.push(P2pChannelsStreamingRpcAction::ResponseSendInit {
                        peer_id,
                        id,
                        response: None,
                    });
                    return Ok(());
                }
                // async ledger request will be triggered by `LedgerReadAction::FindTodos`.
                Ok(())
            }
            P2pChannelsStreamingRpcAction::ResponsePending { .. } => {
                let Self::Ready {
                    remote:
                        P2pStreamingRpcRemoteState::Requested {
                            request, progress, ..
                        },
                    ..
                } = streaming_rpc_state
                else {
                    bug_condition!("{:?} with state {:?}", action, streaming_rpc_state);
                    return Ok(());
                };
                match &**request {
                    P2pStreamingRpcRequest::StagedLedgerParts(_)
                    | P2pStreamingRpcRequest::StagedLedgerPartsFromLast(_) => {
                        *progress =
                            StagedLedgerPartsSendProgress::LedgerGetPending { time: meta.time() }
                                .into();
                    }
                }
                Ok(())
            }
            P2pChannelsStreamingRpcAction::ResponseSendInit { response, id, .. } => {
                let Self::Ready {
                    remote:
                        P2pStreamingRpcRemoteState::Requested {
                            request, progress, ..
                        },
                    ..
                } = streaming_rpc_state
                else {
                    bug_condition!(
                        "`P2pChannelsStreamingRpcAction::ResponseSendInit` with state {:?}",
                        streaming_rpc_state
                    );
                    return Ok(());
                };
                match (&**request, &response) {
                    (
                        P2pStreamingRpcRequest::StagedLedgerPartsFromLast(_),
                        Some(P2pStreamingRpcResponseFull::StagedLedgerParts(data)),
                    ) => {
                        *progress = StagedLedgerPartsSendProgress::FromLastSending {
                            time: meta.time(),
                            data: data.clone(),
                            sent: 0,
                        }
                        .into();
                    }
                    (_, Some(P2pStreamingRpcResponseFull::StagedLedgerParts(data))) => {
                        *progress = StagedLedgerPartsSendProgress::LedgerGetSuccess {
                            time: meta.time(),
                            data: Some(data.clone()),
                        }
                        .into();
                    }
                    (_, None) => {
                        *progress =
                            StagedLedgerPartsSendProgress::Success { time: meta.time() }.into();
                    }
                }

                let dispatcher = state_context.into_dispatcher();
                if response.is_none() {
                    let msg = StreamingRpcChannelMsg::Response(id, None).into();
                    dispatcher.push(P2pChannelsEffectfulAction::MessageSend {
                        peer_id,
                        msg_id: MsgId::first(),
                        msg,
                    });
                    dispatcher.push(P2pChannelsStreamingRpcAction::ResponseSent { peer_id, id });
                    return Ok(());
                }

                dispatcher
                    .push(P2pChannelsStreamingRpcAction::ResponsePartNextSend { peer_id, id });
                Ok(())
            }
            P2pChannelsStreamingRpcAction::ResponsePartNextSend { id, .. } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let state: &P2pState = state.substate()?;

                let Some(response) = state
                    .get_ready_peer(&peer_id)
                    .and_then(|peer| peer.channels.streaming_rpc.remote_next_msg().map(Box::new))
                else {
                    return Ok(());
                };

                dispatcher.push(P2pChannelsStreamingRpcAction::ResponsePartSend {
                    peer_id,
                    id,
                    response,
                });

                Ok(())
            }
            P2pChannelsStreamingRpcAction::ResponsePartSend { id, response, .. } => {
                let Self::Ready {
                    remote: P2pStreamingRpcRemoteState::Requested { progress, .. },
                    ..
                } = streaming_rpc_state
                else {
                    bug_condition!(
                        "`P2pChannelsStreamingRpcAction::ResponsePartSend` with state {:?}",
                        streaming_rpc_state
                    );
                    return Ok(());
                };
                match progress {
                    P2pStreamingRpcSendProgress::StagedLedgerParts(progress) => {
                        *progress = match progress {
                            StagedLedgerPartsSendProgress::LedgerGetSuccess {
                                data: Some(data),
                                ..
                            } => StagedLedgerPartsSendProgress::BaseSent {
                                time: meta.time(),
                                data: data.clone(),
                            },
                            StagedLedgerPartsSendProgress::BaseSent { data, .. } => {
                                StagedLedgerPartsSendProgress::ScanStateBaseSent {
                                    time: meta.time(),
                                    data: data.clone(),
                                }
                            }
                            StagedLedgerPartsSendProgress::ScanStateBaseSent { data, .. } => {
                                StagedLedgerPartsSendProgress::PreviousIncompleteZkappUpdatesSent {
                                    time: meta.time(),
                                    data: data.clone(),
                                }
                            }
                            StagedLedgerPartsSendProgress::PreviousIncompleteZkappUpdatesSent {
                                data,
                                ..
                            } => StagedLedgerPartsSendProgress::ScanStateTreesSending {
                                time: meta.time(),
                                data: data.clone(),
                                tree_index: 0,
                            },
                            StagedLedgerPartsSendProgress::ScanStateTreesSending {
                                data,
                                tree_index,
                                ..
                            } => StagedLedgerPartsSendProgress::ScanStateTreesSending {
                                time: meta.time(),
                                data: data.clone(),
                                tree_index: *tree_index + 1,
                            },
                            StagedLedgerPartsSendProgress::FromLastSending {
                                data, sent, ..
                            } => {
                                let sent = *sent + 1;
                                if sent >= parts_count(data) {
                                    StagedLedgerPartsSendProgress::Success { time: meta.time() }
                                } else {
                                    StagedLedgerPartsSendProgress::FromLastSending {
                                        time: meta.time(),
                                        data: data.clone(),
                                        sent,
                                    }
                                }
                            }
                            progress => {
                                bug_condition!("unexpected state during `P2pStreamingRpcSendProgress::StagedLedgerParts`: {progress:?}");
                                return Ok(());
                            }
                        };

                        if let StagedLedgerPartsSendProgress::ScanStateTreesSending {
                            data,
                            tree_index,
                            ..
                        } = progress
                        {
                            let target_index = data.scan_state.scan_state.trees.1.len();
                            if *tree_index >= target_index {
                                *progress =
                                    StagedLedgerPartsSendProgress::Success { time: meta.time() };
                            }
                        }
                    }
                }

                let dispatcher = state_context.into_dispatcher();

                let msg = StreamingRpcChannelMsg::Response(id, Some(*response)).into();
                dispatcher.push(P2pChannelsEffectfulAction::MessageSend {
                    peer_id,
                    msg_id: MsgId::first(),
                    msg,
                });
                dispatcher.push(P2pChannelsStreamingRpcAction::ResponseSent { peer_id, id });
                Ok(())
            }
            P2pChannelsStreamingRpcAction::ResponseSent { id, .. } => {
                let (remote, request) = match streaming_rpc_state {
                    Self::Ready { remote, .. } => match remote {
                        P2pStreamingRpcRemoteState::Requested { request, .. } => {
                            let request = std::mem::take(request);
                            (remote, request)
                        }
                        _ => {
                            bug_condition!(
                                "`P2pChannelsStreamingRpcAction::ResponseSent` with state {:?}",
                                streaming_rpc_state
                            );
                            return Ok(());
                        }
                    },
                    _ => {
                        bug_condition!(
                            "`P2pChannelsStreamingRpcAction::ResponseSent` with state {:?}",
                            streaming_rpc_state
                        );
                        return Ok(());
                    }
                };
                *remote = P2pStreamingRpcRemoteState::Responded {
                    time: meta.time(),
                    id,
                    request,
                };

                Ok(())
            }
            P2pChannelsStreamingRpcAction::RequestCancel { id, .. } => {
                let Self::Ready {
                    local: local @ P2pStreamingRpcLocalState::Requested { .. },
                    ..
                } = streaming_rpc_state
                else {
                    bug_condition!("{:?} with state {:?}", action, streaming_rpc_state);
                    return Ok(());
                };
                let P2pStreamingRpcLocalState::Requested {
                    request, progress, ..
                } = local
                else {
                    return Ok(());
                };
                let request = std::mem::take(request);
                // A part asked for, or the first, may still come.
                *local = if progress.is_part_pending() {
                    P2pStreamingRpcLocalState::Cancelled {
                        time: meta.time(),
                        id,
                        request,
                    }
                } else {
                    P2pStreamingRpcLocalState::Responded {
                        time: meta.time(),
                        id,
                        request,
                    }
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(P2pChannelsEffectfulAction::MessageSend {
                    peer_id,
                    msg_id: MsgId::first(),
                    msg: StreamingRpcChannelMsg::Cancel(id).into(),
                });
                Ok(())
            }
            P2pChannelsStreamingRpcAction::CancelReceived { id, .. } => {
                let Self::Ready { remote, .. } = streaming_rpc_state else {
                    return Ok(());
                };
                let P2pStreamingRpcRemoteState::Requested {
                    id: rpc_id,
                    request,
                    progress,
                    ..
                } = remote
                else {
                    return Ok(());
                };
                if *rpc_id != id {
                    return Ok(());
                }
                // Before its first part the requester waits for one: it
                // gets an empty answer instead. After, the part it asked for
                // has gone already, or goes before this.
                let unsent = progress.external_data_todo() || progress.external_data_pending();
                // Drops the parts we held for it.
                *remote = P2pStreamingRpcRemoteState::Responded {
                    time: meta.time(),
                    id,
                    request: std::mem::take(request),
                };
                if unsent {
                    let dispatcher = state_context.into_dispatcher();
                    dispatcher.push(P2pChannelsEffectfulAction::MessageSend {
                        peer_id,
                        msg_id: MsgId::first(),
                        msg: StreamingRpcChannelMsg::Response(id, None).into(),
                    });
                }
                Ok(())
            }
        }
    }
}

/// The other peer streaming the same staged ledger to us from the other end,
/// if both advertised `staged_ledger_from_last` (STAGED-SPLIT).
fn staged_ledger_partner(
    p2p_state: &P2pState,
    peer_id: &PeerId,
) -> Option<(PeerId, P2pStreamingRpcId)> {
    let peer = p2p_state.get_ready_peer(peer_id)?;
    if !peer.channels.staged_ledger_from_last {
        return None;
    }
    let request = peer.channels.streaming_rpc.pending_local_rpc()?;
    p2p_state.peers.iter().find_map(|(other_id, other)| {
        let other = other.status.as_ready()?;
        if other_id == peer_id || !other.channels.staged_ledger_from_last {
            return None;
        }
        let other_request = other.channels.streaming_rpc.pending_local_rpc()?;
        if other_request.staged_ledger_block_hash() != request.staged_ledger_block_hash()
            || other_request.is_from_last() == request.is_from_last()
        {
            return None;
        }
        Some((
            *other_id,
            other.channels.streaming_rpc.pending_local_rpc_id()?,
        ))
    })
}

fn local_progress_mut<'a>(
    p2p_state: &'a mut P2pState,
    peer_id: &PeerId,
) -> Option<&'a mut P2pStreamingRpcReceiveProgress> {
    match &mut p2p_state
        .get_ready_peer_mut(peer_id)?
        .channels
        .streaming_rpc
    {
        P2pChannelsStreamingRpcState::Ready {
            local: P2pStreamingRpcLocalState::Requested { progress, .. },
            ..
        } => Some(progress),
        _ => None,
    }
}

/// If `peer_id`'s stream and `partner`'s, the same staged ledger from its
/// two ends, have every part between them, puts them together as
/// `peer_id`'s response and cancels `partner`'s request here: its parts go,
/// so whether one is on its way is read before. The caller sends the
/// cancel.
fn put_staged_ledger_together(
    p2p_state: &mut P2pState,
    peer_id: &PeerId,
    partner: &PeerId,
    time: redux::Timestamp,
) -> bool {
    use P2pStreamingRpcReceiveProgress::{StagedLedgerParts, StagedLedgerPartsFromLast};
    let complete = {
        let progress = |peer_id| {
            p2p_state
                .get_ready_peer(peer_id)?
                .channels
                .streaming_rpc
                .pending_local_rpc_progress()
        };
        match (progress(peer_id), progress(partner)) {
            (Some(StagedLedgerParts(first)), Some(StagedLedgerPartsFromLast(last)))
            | (Some(StagedLedgerPartsFromLast(last)), Some(StagedLedgerParts(first))) => {
                staged_ledger_parts_complete(first, last)
            }
            _ => None,
        }
    };
    if complete != Some(true) {
        return false;
    }
    let Some(theirs) = local_progress_mut(p2p_state, partner) else {
        return false;
    };
    let part_on_its_way = theirs.is_part_pending();
    let mut theirs = match theirs {
        StagedLedgerParts(first) => StagedLedgerParts(std::mem::take(first)),
        StagedLedgerPartsFromLast(last) => StagedLedgerPartsFromLast(std::mem::take(last)),
    };
    let Some(mine) = local_progress_mut(p2p_state, peer_id) else {
        return false;
    };
    let data = match (&mut *mine, &mut theirs) {
        (StagedLedgerParts(first), StagedLedgerPartsFromLast(last))
        | (StagedLedgerPartsFromLast(last), StagedLedgerParts(first)) => {
            take_staged_ledger_parts(first, last)
        }
        _ => None,
    };
    let Some(data) = data else {
        bug_condition!("staged ledger parts complete but not put together");
        return false;
    };
    match mine {
        StagedLedgerParts(first) => {
            *first = StagedLedgerPartsReceiveProgress::Success {
                time,
                data: data.into(),
            }
        }
        StagedLedgerPartsFromLast(last) => {
            last.time = time;
            last.data = Some(data.into());
        }
    }
    if let Some(P2pChannelsStreamingRpcState::Ready { local, .. }) = p2p_state
        .get_ready_peer_mut(partner)
        .map(|peer| &mut peer.channels.streaming_rpc)
    {
        if let P2pStreamingRpcLocalState::Requested { id, request, .. } = local {
            let (id, request) = (*id, std::mem::take(request));
            *local = if part_on_its_way {
                P2pStreamingRpcLocalState::Cancelled { time, id, request }
            } else {
                P2pStreamingRpcLocalState::Responded { time, id, request }
            };
        }
    }
    true
}
