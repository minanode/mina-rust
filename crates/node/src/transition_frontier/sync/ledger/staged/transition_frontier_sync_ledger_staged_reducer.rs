use crate::p2p::{
    channels::{
        rpc::{P2pChannelsRpcAction, P2pRpcId, P2pRpcRequest},
        streaming_rpc::{P2pChannelsStreamingRpcAction, P2pStreamingRpcRequest},
    },
    PeerId,
};
use mina_p2p_messages::{hash::MinaHash, list::List, v2};
use rand::prelude::*;
use std::sync::Arc;

use crate::{
    ledger::write::{LedgerWriteAction, LedgerWriteRequest},
    transition_frontier::sync::TransitionFrontierSyncAction,
};

use super::{
    takes_staged_ledger_from_last, PeerStagedLedgerPartsFetchState,
    StagedLedgerAuxAndPendingCoinbasesValidated, StagedLedgerPartsSender,
    TransitionFrontierSyncLedgerStagedAction, TransitionFrontierSyncLedgerStagedActionWithMetaRef,
    TransitionFrontierSyncLedgerStagedState,
};

impl TransitionFrontierSyncLedgerStagedState {
    pub fn reducer(
        mut state_context: crate::Substate<Self>,
        action: TransitionFrontierSyncLedgerStagedActionWithMetaRef<'_>,
    ) {
        let Ok(state) = state_context.get_substate_mut() else {
            // TODO: log or propagate
            return;
        };
        let (action, meta) = action.split();

        match action {
            TransitionFrontierSyncLedgerStagedAction::PartsFetchPending => {
                // handled in parent. TODO(refactor) check this
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchInit);
            }
            TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchInit => {
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                // The request for a root kept as the target after the root
                // moved ended without the parts: ask for the current root's
                // instead (WEB-ROOT-RETARGET).
                if dispatcher.push_if_enabled(
                    TransitionFrontierSyncAction::LedgerRootRetarget,
                    global_state,
                    meta.time(),
                ) {
                    return;
                }
                let Some((p2p, target_best_tip, staged_ledger, fetch_attempts)) =
                    None.or_else(|| {
                        let staged = global_state.transition_frontier.sync.ledger()?.staged()?;
                        Some((
                            global_state.p2p.ready()?,
                            global_state.transition_frontier.sync.best_tip()?,
                            staged,
                            staged.fetch_attempts()?,
                        ))
                    })
                else {
                    return;
                };
                let (live_first, live_last) = staged_ledger.live_parts_streams(meta.time(), p2p);
                let second_stream = staged_ledger.wants_second_parts_stream(meta.time(), p2p);
                // Peers that may have the block, for a second stream.
                let mut may_have_block = Vec::new();

                let mut peers =
                    p2p.ready_rpc_peers_iter()
                        .fold(Vec::new(), |mut list, (peer_id, state)| {
                            if fetch_attempts
                                .get(peer_id)
                                .is_some_and(|attempt| !attempt.retry_ready(meta.time()))
                            {
                                return list;
                            }
                            let mut order = 0u8;
                            // TODO(binier): maybe take into account fetch attempt time.
                            order = order.saturating_add(if fetch_attempts.contains_key(peer_id) {
                                5
                            } else {
                                0
                            });
                            order =
                                order.saturating_add(state.best_tip.as_ref().map_or(2, |tip| {
                                    let k = target_best_tip.constants().k.as_u32();
                                    if tip.height() > target_best_tip.height()
                                        || tip.height().abs_diff(target_best_tip.height()) > k
                                    {
                                        // can't have the block we need
                                        10
                                    } else {
                                        // has common block
                                        if IntoIterator::into_iter([
                                            tip.hash(),
                                            tip.pred_hash(),
                                            target_best_tip.hash(),
                                            target_best_tip.pred_hash(),
                                        ])
                                        .collect::<std::collections::BTreeSet<_>>()
                                        .len()
                                            < 4
                                        {
                                            0
                                        } else {
                                            1
                                        }
                                    }
                                }));
                            if order < 10 {
                                may_have_block.push((*peer_id, state.channels.next_local_rpc_id()));
                            }

                            if list
                                .last()
                                .is_some_and(|(_, _, ord): &(_, _, u8)| *ord > order)
                            {
                                // remove less priority peers
                                list.clear();
                            }
                            list.push((peer_id, state.channels.next_local_rpc_id(), order));

                            list
                        });

                let block_hash = staged_ledger.target().staged.block_hash.clone();
                peers.shuffle(&mut global_state.pseudo_rng());
                // The staged ledger is tens of megabytes in one request: ask
                // the nearest WebRTC peer first (`plan_sync_peers`,
                // WEB-NEAREST-RELAY). libp2p peers keep their places.
                let peers = p2p
                    .plan_sync_peers(
                        peers.into_iter().map(|(peer_id, rpc_id, order)| {
                            (*peer_id, (peer_id, rpc_id, order), 1)
                        }),
                        meta.time(),
                    )
                    .into_ordered()
                    .map(|(peer, _)| peer);

                let mut first = live_first;
                for (&peer_id, rpc_id, _) in peers {
                    if live_first.is_some() || live_last.is_some() {
                        // A request is live: only a second stream may go.
                        break;
                    }
                    let enqueued = if p2p.is_libp2p_peer(&peer_id) {
                        // use old heavy rpc for libp2p peers.
                        dispatcher.push_if_enabled(
                            P2pChannelsRpcAction::RequestSend {
                                peer_id,
                                id: rpc_id,
                                request: Box::new(
                                    P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(
                                        block_hash.clone(),
                                    ),
                                ),
                                on_init: Some(redux::callback!(
                                    on_send_p2p_staged_ledger_parts_rpc_request(
                                        (peer_id: PeerId, rpc_id: P2pRpcId, _request: P2pRpcRequest)
                                    ) -> crate::Action {
                                        TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchPending {
                                            peer_id,
                                            rpc_id,
                                        }
                                    }
                                ))
                            },
                            global_state,
                            meta.time()
                        )
                    } else {
                        // use streaming rpc for webrtc peers.
                        dispatcher.push_if_enabled(
                            P2pChannelsStreamingRpcAction::RequestSend {
                                peer_id,
                                id: rpc_id,
                                request: Box::new(P2pStreamingRpcRequest::StagedLedgerParts(
                                    block_hash.clone(),
                                )),
                                on_init: Some(redux::callback!(
                                    on_send_streaming_p2p_staged_ledger_parts_rpc_request(
                                        (peer_id: PeerId, rpc_id: P2pRpcId, _request: P2pStreamingRpcRequest)
                                    ) -> crate::Action {
                                        TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchPending {
                                            peer_id,
                                            rpc_id,
                                        }
                                    }))
                            },
                            global_state,
                            meta.time()
                        )
                    };

                    // TODO: instead add an intermediary action for the Peer request with an enabling condition
                    // that will make sure that only one staged ledger part request
                    // is ongoing. So here we dispatch the action for all peers, but
                    // after one picks it up the rest will be filtered out.
                    if enqueued {
                        first = Some(peer_id);
                        break;
                    }
                }

                // STAGED-SPLIT: a second WebRTC peer sends the parts from the
                // other end, so that the staged ledger comes over two links at
                // once. The p2p layer puts the two together where they meet
                // and cancels the other; each goes as fast as its link, so the
                // split follows their throughput. If either fails, the other
                // carries on to the end alone.
                let other_end = match (first, live_last) {
                    (Some(peer_id), None) => Some((peer_id, true)),
                    (None, Some(peer_id)) => Some((peer_id, false)),
                    _ => None,
                };
                let Some((other_end, from_last)) = other_end.filter(|(peer_id, _)| {
                    (second_stream || live_first.is_none())
                        && staged_ledger.is_second_parts_stream_allowed(p2p)
                        && takes_staged_ledger_from_last(p2p, peer_id)
                }) else {
                    return;
                };
                let helpers = p2p
                    .plan_sync_peers(
                        may_have_block
                            .into_iter()
                            .filter(|(peer_id, _)| {
                                *peer_id != other_end && takes_staged_ledger_from_last(p2p, peer_id)
                            })
                            .map(|(peer_id, rpc_id)| (peer_id, (peer_id, rpc_id), 1)),
                        meta.time(),
                    )
                    .into_ordered()
                    .map(|(peer, _)| peer);
                for (peer_id, rpc_id) in helpers {
                    let request = if from_last {
                        P2pStreamingRpcRequest::StagedLedgerPartsFromLast(block_hash.clone())
                    } else {
                        P2pStreamingRpcRequest::StagedLedgerParts(block_hash.clone())
                    };
                    if dispatcher.push_if_enabled(
                        P2pChannelsStreamingRpcAction::RequestSend {
                            peer_id,
                            id: rpc_id,
                            request: Box::new(request),
                            on_init: Some(redux::callback!(
                                on_send_streaming_p2p_staged_ledger_second_parts_request(
                                    (peer_id: PeerId, rpc_id: P2pRpcId, _request: P2pStreamingRpcRequest)
                                ) -> crate::Action {
                                    TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchPending {
                                        peer_id,
                                        rpc_id,
                                    }
                                }))
                        },
                        global_state,
                        meta.time(),
                    ) {
                        break;
                    }
                }
            }
            TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchPending { peer_id, rpc_id } => {
                let Self::PartsFetchPending { attempts, .. } = state else {
                    return;
                };
                attempts.insert(
                    *peer_id,
                    PeerStagedLedgerPartsFetchState::Pending {
                        time: meta.time(),
                        rpc_id: *rpc_id,
                    },
                );
            }
            TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchError {
                peer_id,
                error,
                ..
            } => {
                let Self::PartsFetchPending { attempts, .. } = state else {
                    return;
                };
                let Some(attempt) = attempts.get_mut(peer_id) else {
                    return;
                };
                let PeerStagedLedgerPartsFetchState::Pending { rpc_id, .. } = &attempt else {
                    return;
                };
                *attempt = PeerStagedLedgerPartsFetchState::Error {
                    time: meta.time(),
                    rpc_id: *rpc_id,
                    error: error.clone(),
                };

                // Dispatch
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchInit);
            }
            TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchSuccess {
                peer_id,
                parts,
                ..
            } => {
                let Self::PartsFetchPending {
                    target, attempts, ..
                } = state
                else {
                    return;
                };
                let Some(attempt) = attempts.get_mut(peer_id) else {
                    return;
                };

                let expected_hash = &target.staged.hashes;
                let (validated, refusal) = match StagedLedgerAuxAndPendingCoinbasesValidated::check(
                    parts,
                    expected_hash,
                ) {
                    Ok(valid) => (
                        StagedLedgerAuxAndPendingCoinbasesValidated::Valid(Arc::new(valid)),
                        None,
                    ),
                    Err(reason) => (
                        StagedLedgerAuxAndPendingCoinbasesValidated::Invalid(parts.clone()),
                        Some(reason),
                    ),
                };

                *attempt = PeerStagedLedgerPartsFetchState::Success {
                    time: meta.time(),
                    parts: validated,
                };

                // Dispatch
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                if !dispatcher.push_if_enabled(
                    TransitionFrontierSyncLedgerStagedAction::PartsPeerValid { sender: *peer_id },
                    global_state,
                    meta.time(),
                ) {
                    dispatcher.push(TransitionFrontierSyncLedgerStagedAction::PartsPeerInvalid {
                        sender: *peer_id,
                        parts: parts.clone(),
                        reason: refusal.unwrap_or_default(),
                    });
                }
            }
            TransitionFrontierSyncLedgerStagedAction::PartsPeerInvalid { sender, .. }
            | TransitionFrontierSyncLedgerStagedAction::PartsPeerValid { sender, .. } => {
                let Self::PartsFetchPending { attempts, .. } = &mut *state else {
                    return;
                };
                let Some(attempt) = attempts.get_mut(sender) else {
                    return;
                };
                let PeerStagedLedgerPartsFetchState::Success { parts, .. } = attempt else {
                    return;
                };

                match parts {
                    StagedLedgerAuxAndPendingCoinbasesValidated::Invalid(_) => {
                        *attempt = PeerStagedLedgerPartsFetchState::Invalid { time: meta.time() };
                    }
                    StagedLedgerAuxAndPendingCoinbasesValidated::Valid(parts) => {
                        *attempt = PeerStagedLedgerPartsFetchState::Valid {
                            time: meta.time(),
                            parts: parts.clone(),
                        };
                    }
                }

                // STAGED-SPLIT: invalid parts may have been put together with
                // another peer's. Either may have sent the bad ones, so both
                // count as invalid and neither is rejected. After any invalid
                // parts the next fetch is from one peer at a time, so a peer
                // whose own parts are invalid is the one rejected. The frontier
                // keeps the refusal for later targets too
                // (`TransitionFrontierState::staged_parts_refusals`).
                let mut put_together = false;
                if let TransitionFrontierSyncLedgerStagedAction::PartsPeerInvalid { .. } = action {
                    if let Self::PartsFetchPending {
                        attempts,
                        single_stream,
                        ..
                    } = &mut *state
                    {
                        for (peer_id, other) in attempts.iter_mut() {
                            if peer_id != sender && other.fetch_pending_rpc_id().is_some() {
                                *other =
                                    PeerStagedLedgerPartsFetchState::Invalid { time: meta.time() };
                                put_together = true;
                            }
                        }
                        *single_stream = true;
                    }
                }

                // Dispatch
                let dispatcher = state_context.into_dispatcher();
                if let TransitionFrontierSyncLedgerStagedAction::PartsPeerValid { .. } = action {
                    dispatcher.push(
                        TransitionFrontierSyncLedgerStagedAction::PartsFetchSuccess {
                            sender: *sender,
                        },
                    );
                } else {
                    // `P2pSyncPeerStats::record_rejected`: it ranks as near
                    // no longer.
                    if !put_together {
                        dispatcher
                            .push(P2pChannelsRpcAction::SyncAnswerRejected { peer_id: *sender });
                    }
                    dispatcher.push(TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchInit);
                }
            }
            TransitionFrontierSyncLedgerStagedAction::PartsFetchSuccess { sender } => {
                let Self::PartsFetchPending {
                    target, attempts, ..
                } = state
                else {
                    return;
                };
                let Some(PeerStagedLedgerPartsFetchState::Valid { parts, .. }) =
                    attempts.get(sender)
                else {
                    return;
                };
                let parts = parts.clone();
                let mut other_attempts = std::mem::take(attempts);
                other_attempts.remove(sender);
                *state = Self::PartsFetchSuccess {
                    time: meta.time(),
                    target: target.clone(),
                    parts,
                    sender: StagedLedgerPartsSender {
                        peer_id: *sender,
                        other_attempts,
                    },
                };

                // Dispatch
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(TransitionFrontierSyncLedgerStagedAction::ReconstructInit);
            }
            TransitionFrontierSyncLedgerStagedAction::ReconstructEmpty => {
                // handled in parent. TODO(refactor): check this
                // Dispatch
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(TransitionFrontierSyncLedgerStagedAction::ReconstructInit);
            }
            TransitionFrontierSyncLedgerStagedAction::ReconstructInit => {
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let ledger_state = global_state.transition_frontier.sync.ledger();
                let Some((target, parts)) =
                    ledger_state.and_then(|s| s.staged()?.target_with_parts())
                else {
                    return;
                };
                let snarked_ledger_hash = target.snarked_ledger_hash.clone();
                let parts = parts.cloned();

                dispatcher.push(LedgerWriteAction::Init {
                    request: LedgerWriteRequest::StagedLedgerReconstruct {
                        snarked_ledger_hash,
                        parts,
                    },
                    on_init: redux::callback!(
                        on_staged_ledger_reconstruct_init(_request: LedgerWriteRequest) -> crate::Action {
                            TransitionFrontierSyncLedgerStagedAction::ReconstructPending
                        }
                    ),
                });
            }
            TransitionFrontierSyncLedgerStagedAction::ReconstructPending => {
                let (target, parts, sender) = match state {
                    Self::PartsFetchSuccess {
                        target,
                        parts,
                        sender,
                        ..
                    } => (target, Some(parts.clone()), Some(sender.clone())),
                    Self::ReconstructEmpty { target, .. } => (target, None, None),
                    _ => return,
                };
                *state = Self::ReconstructPending {
                    time: meta.time(),
                    target: target.clone(),
                    parts,
                    sender,
                }
            }
            TransitionFrontierSyncLedgerStagedAction::ReconstructError { error } => {
                let Self::ReconstructPending {
                    target,
                    parts,
                    sender,
                    ..
                } = state
                else {
                    return;
                };
                if let Some(sender) = sender.take() {
                    // Parts that make the staged ledger hash but don't
                    // rebuild the staged ledger. From one peer, its answer is
                    // rejected as parts that don't make the hash are. Put
                    // together from two (STAGED-SPLIT), either may have sent
                    // the part that fails: both count as invalid and neither
                    // is rejected, as when such parts don't make the hash.
                    // Either way the next fetch is from one peer at a time,
                    // so a peer whose parts fail again is the one rejected,
                    // for a later target too (`TransitionFrontierState::
                    // staged_parts_refusals`, recorded by the frontier).
                    let put_together = sender.may_be_put_together();
                    let StagedLedgerPartsSender {
                        peer_id,
                        other_attempts: mut attempts,
                    } = sender;
                    let invalid = PeerStagedLedgerPartsFetchState::Invalid { time: meta.time() };
                    for attempt in attempts.values_mut() {
                        if attempt.fetch_pending_rpc_id().is_some() {
                            *attempt = invalid.clone();
                        }
                    }
                    attempts.insert(peer_id, invalid);
                    *state = Self::PartsFetchPending {
                        time: meta.time(),
                        target: target.clone(),
                        attempts,
                        single_stream: true,
                    };
                    let dispatcher = state_context.into_dispatcher();
                    if !put_together {
                        dispatcher.push(P2pChannelsRpcAction::SyncAnswerRejected { peer_id });
                    }
                    dispatcher.push(TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchInit);
                    return;
                }
                *state = Self::ReconstructError {
                    time: meta.time(),
                    target: target.clone(),
                    parts: parts.clone(),
                    error: error.clone(),
                };
                panic!("Staged ledger reconstruct failure {error}");
            }
            TransitionFrontierSyncLedgerStagedAction::ReconstructSuccess { .. } => {
                let Self::ReconstructPending { target, parts, .. } = state else {
                    return;
                };
                *state = Self::ReconstructSuccess {
                    time: meta.time(),
                    target: target.clone(),
                    parts: parts.clone(),
                };

                // Dispatch
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(TransitionFrontierSyncLedgerStagedAction::Success);
            }
            TransitionFrontierSyncLedgerStagedAction::Success => {
                let Self::ReconstructSuccess { target, parts, .. } = state else {
                    return;
                };

                *state = Self::Success {
                    time: meta.time(),
                    target: target.clone(),
                    needed_protocol_states: parts
                        .as_ref()
                        .map(|parts| &parts.needed_blocks)
                        .unwrap_or(&List::new())
                        .iter()
                        .filter_map(|block| {
                            Some((
                                v2::StateHash::from_fp(MinaHash::try_hash(block).ok()?),
                                block.clone(),
                            ))
                        })
                        .collect(),
                };
            }
        }
    }
}
