use binprot::BinProtWrite;
use mina_core::{
    block::{
        prevalidate::{validate_delta_block_chain, BlockPrevalidationError},
        BlockWithHash,
    },
    bug_condition, log,
    transaction::TransactionWithHash,
};
use mina_p2p::{
    channels::{
        best_tip::P2pChannelsBestTipAction,
        rpc::{BestTipWithProof, P2pChannelsRpcAction, P2pRpcRequest, P2pRpcResponse},
        streaming_rpc::P2pStreamingRpcResponseFull,
    },
    disconnection::{P2pDisconnectionAction, P2pDisconnectionReason},
    P2pNetworkPubsubAction, PeerId,
};
use mina_p2p_messages::{
    bigint::InvalidBigInt,
    gossip::GossipNetMessageV2,
    v2::{self, MinaLedgerSyncLedgerAnswerStableV2, StateHash},
};
use redux::{ActionMeta, ActionWithMeta, Dispatcher};

use crate::{
    p2p_ready,
    snark_pool::candidate::SnarkPoolCandidateAction,
    transaction_pool::candidate::TransactionPoolCandidateAction,
    transition_frontier::{
        body_reference::validate_body_reference,
        candidate::{allow_block_too_late, TransitionFrontierCandidateAction},
        sync::{
            ledger::{
                snarked::{
                    PeerLedgerQueryError, PeerLedgerQueryResponse,
                    TransitionFrontierSyncLedgerSnarkedAction,
                },
                staged::{
                    PeerStagedLedgerPartsFetchError, TransitionFrontierSyncLedgerStagedAction,
                },
            },
            PeerBlockFetchError, TransitionFrontierSyncAction,
        },
    },
    watched_accounts::{
        WatchedAccountLedgerInitialState, WatchedAccountsLedgerInitialStateGetError,
    },
    Action, State, WatchedAccountsAction,
};

use super::P2pCallbacksAction;

fn get_rpc_request<'a>(state: &'a State, peer_id: &PeerId, id: u64) -> Option<&'a P2pRpcRequest> {
    state
        .p2p
        .get_ready_peer(peer_id)
        .and_then(|s| s.channels.rpc.local_responded_request())
        .filter(|(rpc_id, _)| *rpc_id == id)
        .map(|(_, req)| req)
}

fn bounded_completed_snarks<'a>(
    works: impl IntoIterator<Item = &'a mina_core::snark::Snark>,
) -> mina_p2p_messages::rpc::CompletedSnarks {
    bounded_completed_snarks_with_byte_limit(
        works,
        mina_p2p_messages::rpc::MAX_COMPLETED_SNARKS_BYTES,
    )
}

fn bounded_completed_snarks_with_byte_limit<'a>(
    works: impl IntoIterator<Item = &'a mina_core::snark::Snark>,
    byte_limit: usize,
) -> mina_p2p_messages::rpc::CompletedSnarks {
    let mut result = mina_p2p_messages::list::List::new();
    // Include room for the RPC envelope and list length.
    let mut bytes = 64usize;
    for work in works
        .into_iter()
        .take(mina_p2p_messages::rpc::MAX_COMPLETED_SNARKS)
    {
        // A pool work's proofs come back from the proof cache (PC1).
        let Ok(encoded) = work.to_wire() else {
            continue;
        };
        let mut work_bytes = Vec::new();
        if encoded.binprot_write(&mut work_bytes).is_err() {
            break;
        }
        if bytes.saturating_add(work_bytes.len()) > byte_limit {
            break;
        }
        bytes += work_bytes.len();
        result.push_back(encoded);
    }
    mina_p2p_messages::rpc::CompletedSnarks(result)
}

impl crate::State {
    pub fn p2p_callback_reducer(
        state_context: crate::Substate<Self>,
        action: ActionWithMeta<&P2pCallbacksAction>,
    ) {
        let (action, meta) = action.split();
        let time = meta.time();
        let (dispatcher, state) = state_context.into_dispatcher_and_state();

        match action {
            P2pCallbacksAction::P2pChannelsRpcReady { peer_id } => {
                let peer_id = *peer_id;

                if state.p2p.get_peer(&peer_id).is_some_and(|p| p.is_libp2p) {
                    // for webrtc peers, we don't need to send this rpc, as we
                    // will receive current best tip in best tip channel anyways.
                    // For a libp2p peer it is sent only within the peer's best
                    // tip budget, which outlives the connection, so a peer that
                    // keeps reconnecting is not asked more often. If the budget
                    // refuses it now, it is sent later, on `CheckTimeouts`.
                    dispatcher.push(P2pChannelsRpcAction::RequestSend {
                        peer_id,
                        id: 0,
                        request: Box::new(P2pRpcRequest::BestTipWithProof),
                        on_init: None,
                    });
                }

                dispatcher.push(TransitionFrontierCandidateAction::ChainProofPeersQuery);
                dispatcher.push(TransitionFrontierSyncLedgerSnarkedAction::PeersQuery);
                dispatcher.push(TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchInit);
                dispatcher.push(TransitionFrontierSyncAction::BlocksPeersQuery);
            }
            P2pCallbacksAction::P2pChannelsRpcTimeout { peer_id, id } => {
                // The request timed out once. The peer is kept, as it may
                // only be slow, but what we asked of it is retried elsewhere.
                // A second timeout of the same request disconnects it (the
                // RPC channel's reducer).
                let peer_id = *peer_id;
                let rpc_id = *id;

                dispatcher.push(TransitionFrontierCandidateAction::ChainProofPeersQuery);
                dispatcher.push(
                    TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsError {
                        peer_id,
                        rpc_id,
                        error: PeerLedgerQueryError::Timeout,
                    },
                );
                dispatcher.push(
                    TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressError {
                        peer_id,
                        rpc_id,
                        error: PeerLedgerQueryError::Timeout,
                    },
                );
                dispatcher.push(
                    TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchError {
                        peer_id,
                        rpc_id,
                        error: PeerStagedLedgerPartsFetchError::Timeout,
                    },
                );
                dispatcher.push(TransitionFrontierSyncAction::BlocksPeerQueryError {
                    peer_id,
                    rpc_id,
                    error: PeerBlockFetchError::Timeout,
                });
            }
            P2pCallbacksAction::P2pChannelsRpcResponseReceived {
                peer_id,
                id,
                response,
            } => {
                // The peer's RPC slot is free: a chain proof we need takes it
                // before ledger and block queries do, so that a new best tip
                // doesn't wait for them to finish. Actions run depth first,
                // and an answer's follow-ups ask the next query of a free
                // peer at once (a ledger answer's `ChildHashesAccepted`
                // dispatches `PeersQuery`), so the chain proof query goes
                // before them. After a best tip answer, which may carry the
                // proof itself, it goes after.
                let is_best_tip_answer = matches!(
                    response.as_deref(),
                    Some(P2pRpcResponse::BestTipWithProof(_))
                );
                if !is_best_tip_answer {
                    dispatcher.push(TransitionFrontierCandidateAction::ChainProofPeersQuery);
                }
                let request = get_rpc_request(state, peer_id, *id);
                State::handle_rpc_channels_response(
                    dispatcher,
                    meta,
                    *id,
                    *peer_id,
                    request,
                    response,
                    state.verification.fetch_arrival(),
                );
                if is_best_tip_answer {
                    dispatcher.push(TransitionFrontierCandidateAction::ChainProofPeersQuery);
                }
                dispatcher.push(TransitionFrontierSyncLedgerSnarkedAction::PeersQuery);
                dispatcher.push(TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchInit);
                dispatcher.push(TransitionFrontierSyncAction::BlocksPeersQuery);
            }
            P2pCallbacksAction::P2pChannelsRpcRequestReceived {
                peer_id,
                id,
                request,
            } => {
                State::handle_rpc_channels_request(
                    dispatcher,
                    state,
                    meta,
                    *request.clone(),
                    *peer_id,
                    *id,
                );
            }
            P2pCallbacksAction::P2pChannelsStreamingRpcReady => {
                dispatcher.push(TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchInit);
            }
            P2pCallbacksAction::P2pChannelsStreamingRpcTimeout { peer_id, id } => {
                let peer_id = *peer_id;
                let rpc_id = *id;

                let Some(peer) = state.p2p.get_ready_peer(&peer_id) else {
                    bug_condition!("get_ready_peer({:?}) returned None", peer_id);
                    return;
                };
                let Some(rpc_kind) = peer.channels.streaming_rpc.pending_local_rpc_kind() else {
                    bug_condition!("peer: {:?} pending_local_rpc_kind() returned None", peer_id);
                    return;
                };
                dispatcher.push(
                    TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchError {
                        peer_id,
                        rpc_id,
                        error: PeerStagedLedgerPartsFetchError::Timeout,
                    },
                );
                dispatcher.push(P2pDisconnectionAction::Init {
                    peer_id,
                    reason: P2pDisconnectionReason::TransitionFrontierStreamingRpcTimeout(rpc_kind),
                });
            }
            P2pCallbacksAction::P2pChannelsStreamingRpcResponseReceived {
                peer_id,
                id,
                response,
            } => {
                let peer_id = *peer_id;
                let rpc_id = *id;

                match response {
                    None => {
                        dispatcher.push(
                            TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchError {
                                peer_id,
                                rpc_id,
                                error: PeerStagedLedgerPartsFetchError::DataUnavailable,
                            },
                        );
                    }
                    Some(P2pStreamingRpcResponseFull::StagedLedgerParts(parts)) => {
                        dispatcher.push(
                            TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchSuccess {
                                peer_id,
                                rpc_id,
                                parts: parts.clone(),
                            },
                        );
                    }
                }
                dispatcher.push(TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchInit);
            }
            P2pCallbacksAction::P2pDisconnection { peer_id } => {
                let peer_id = *peer_id;

                if let Some(s) = state.transition_frontier.sync.ledger() {
                    s.snarked()
                        .map(|s| {
                            s.peer_address_query_pending_rpc_ids(&peer_id)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                        .into_iter()
                        .for_each(|rpc_id| {
                            dispatcher.push(
                                TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressError {
                                    peer_id,
                                    rpc_id,
                                    error: PeerLedgerQueryError::Disconnected,
                                },
                            );
                        });

                    if let Some(rpc_id) = s
                        .snarked()
                        .and_then(|s| s.peer_num_accounts_rpc_id(&peer_id))
                    {
                        dispatcher.push(
                            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsError {
                                peer_id,
                                rpc_id,
                                error: PeerLedgerQueryError::Disconnected,
                            },
                        );
                    }

                    if let Some(rpc_id) = s.staged().and_then(|s| s.parts_fetch_rpc_id(&peer_id)) {
                        dispatcher.push(
                            TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchError {
                                peer_id,
                                rpc_id,
                                error: PeerStagedLedgerPartsFetchError::Disconnected,
                            },
                        )
                    }
                }

                state
                    .transition_frontier
                    .sync
                    .blocks_fetch_from_peer_pending_rpc_ids(&peer_id)
                    .for_each(|rpc_id| {
                        dispatcher.push(TransitionFrontierSyncAction::BlocksPeerQueryError {
                            peer_id,
                            rpc_id,
                            error: PeerBlockFetchError::Disconnected,
                        });
                    });

                state
                    .watched_accounts
                    .iter()
                    .filter_map(|(pub_key, a)| match &a.initial_state {
                        WatchedAccountLedgerInitialState::Pending {
                            peer_id: account_peer_id,
                            ..
                        } => {
                            if account_peer_id == &peer_id {
                                Some(WatchedAccountsAction::LedgerInitialStateGetError {
                                    pub_key: pub_key.clone(),
                                    error:
                                        WatchedAccountsLedgerInitialStateGetError::PeerDisconnected,
                                })
                            } else {
                                None
                            }
                        }
                        _ => None,
                    })
                    .for_each(|action| dispatcher.push(action));

                dispatcher.push(TransactionPoolCandidateAction::PeerPrune { peer_id });
                dispatcher.push(SnarkPoolCandidateAction::PeerPrune { peer_id });
            }
            P2pCallbacksAction::RpcRespondBestTip { peer_id } => {
                let Some(best_tip) = state.transition_frontier.best_tip() else {
                    bug_condition!("Best tip not found");
                    return;
                };

                dispatcher.push(P2pChannelsBestTipAction::ResponseSend {
                    peer_id: *peer_id,
                    best_tip: best_tip.clone(),
                });
            }
            P2pCallbacksAction::P2pPubsubValidateMessage { message_id } => {
                let Some(message_content) = state.p2p.ready().and_then(|p2p| {
                    p2p.network
                        .scheduler
                        .broadcast_state
                        .mcache
                        .get_message(message_id)
                }) else {
                    bug_condition!("Failed to find message for id: {:?}", message_id);
                    return;
                };

                let pre_validation_result = match message_content {
                    GossipNetMessageV2::NewState(new_best_tip) => {
                        match BlockWithHash::try_new(new_best_tip.clone()) {
                            Ok(block) => {
                                let allow_block_too_late = allow_block_too_late(state, &block);
                                let result = state.prevalidate_block(&block, allow_block_too_late);
                                dispatcher.push(
                                    crate::verification::VerificationUpdate::headers(
                                        &block,
                                        crate::verification::Arrival::Gossip,
                                        &result,
                                    )
                                    .action(),
                                );
                                match result {
                                    Ok(()) => PreValidationResult::Continue,
                                    // A body proof read failing here is
                                    // this node's fault, not the sender's.
                                    Err(error)
                                        if matches!(
                                            error,
                                            BlockPrevalidationError::ReceivedTooEarly { .. }
                                                | BlockPrevalidationError::BodyProofUnavailable { .. }
                                        ) =>
                                    {
                                        PreValidationResult::Ignore {
                                            reason: format!(
                                                "Block prevalidation failed: {:?}",
                                                error
                                            ),
                                        }
                                    }
                                    Err(error) => PreValidationResult::Reject {
                                        reason: format!("Block prevalidation failed: {:?}", error),
                                    },
                                }
                            }
                            Err(_) => {
                                log::error!(time; "P2pCallbacksAction::P2pPubsubValidateMessage: Invalid bigint in block");
                                PreValidationResult::Reject{reason: "P2pCallbacksAction::P2pPubsubValidateMessage: Invalid bigint in block".to_owned()}
                            }
                        }
                    }
                    _ => {
                        // TODO: add pre validation for Snark pool and Transaction pool diffs
                        PreValidationResult::Continue
                    }
                };

                match pre_validation_result {
                    PreValidationResult::Continue => {
                        dispatcher.push(P2pNetworkPubsubAction::ValidateIncomingMessage {
                            message_id: *message_id,
                        });
                    }
                    PreValidationResult::Reject { reason } => {
                        dispatcher.push(P2pNetworkPubsubAction::RejectMessage {
                            message_id: Some(mina_p2p::BroadcastMessageId::MessageId {
                                message_id: *message_id,
                            }),
                            peer_id: None,
                            reason,
                        });
                    }
                    PreValidationResult::Ignore { reason } => {
                        dispatcher.push(P2pNetworkPubsubAction::IgnoreMessage {
                            message_id: Some(mina_p2p::BroadcastMessageId::MessageId {
                                message_id: *message_id,
                            }),
                            reason,
                        });
                    }
                }
            }
        }
    }

    fn handle_rpc_channels_request(
        dispatcher: &mut Dispatcher<Action, State>,
        state: &State,
        meta: ActionMeta,
        request: P2pRpcRequest,
        peer_id: PeerId,
        id: u64,
    ) {
        // Startup restore keeps best_chain at genesis until its commit. Treat
        // that frontier as unavailable, just as OCaml does while bootstrapping.
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/rpcs.ml#L1135-L1151>
        let best_chain = if state.transition_frontier.persist_holds_sync() {
            &[][..]
        } else {
            state.transition_frontier.best_chain.as_slice()
        };
        match request {
            P2pRpcRequest::Catchup(request) => {
                use mina_p2p::channels::rpc::CatchupRequest;
                let history = &state.transition_frontier.root_history;
                let historical_count = match &request {
                    CatchupRequest::TransitionChain(hashes) => hashes
                        .iter()
                        .filter(|hash| {
                            history
                                .get(&v2::StateHash::from(v2::DataHashLibStateHashStableV1(
                                    (*hash).clone(),
                                )))
                                .is_some()
                        })
                        .count(),
                    CatchupRequest::TransitionChainProof(hash) => usize::from(
                        history
                            .get(&v2::StateHash::from(v2::DataHashLibStateHashStableV1(
                                hash.clone(),
                            )))
                            .is_some(),
                    ),
                    _ => 0,
                };
                if !best_chain.is_empty() && historical_count > 0 {
                    mina_core::debug!(meta.time(); kind = "P2pCatchupRootHistoryHit",
                        peer_id = peer_id.to_string(), rpc_id = id, historical_count = historical_count);
                }
                let response = super::super::catchup::respond(
                    best_chain,
                    &state.transition_frontier.root_history,
                    request,
                );
                dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                    peer_id,
                    id,
                    response: Some(Box::new(P2pRpcResponse::Catchup(Box::new(response)))),
                });
            }
            P2pRpcRequest::BestTipWithProof => {
                let response = None.or_else(|| {
                    if !state.transition_frontier.can_prove_best_tip() {
                        return None;
                    }
                    let best_tip = best_chain.last()?;
                    let mut chain_iter = best_chain.iter();
                    let root_block = chain_iter.next()?;
                    // TODO(binier): cache body hashes
                    let Ok(body_hashes) = chain_iter
                        .map(|b| b.header().protocol_state.body.try_hash())
                        .collect::<Result<_, _>>()
                    else {
                        mina_core::error!(meta.time(); "P2pRpcRequest::BestTipWithProof: invalid protocol state");
                        return None;
                    };

                    Some(BestTipWithProof {
                        best_tip: best_tip.block().clone(),
                        proof: (body_hashes, root_block.block().clone()),
                    })
                });
                let response = response.map(P2pRpcResponse::BestTipWithProof).map(Box::new);
                dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                    peer_id,
                    id,
                    response,
                });
            }
            P2pRpcRequest::Block(hash) => {
                let response = best_chain
                    .iter()
                    .rev()
                    .find(|b| b.hash() == &hash)
                    .map(|b| b.block().clone())
                    .map(P2pRpcResponse::Block)
                    .map(Box::new);
                dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                    peer_id,
                    id,
                    response,
                });
            }
            P2pRpcRequest::LedgerQuery(..) => {
                // async ledger request will be triggered
                // by `LedgerReadAction::FindTodos`.
            }
            P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(..) => {
                // async ledger request will be triggered
                // by `LedgerReadAction::FindTodos`.
            }
            P2pRpcRequest::Transaction(hash) => {
                let tx = state.transaction_pool.get(&hash);
                let response = tx
                    .map(|v| v.forget_check())
                    .map(|tx| (&tx).into())
                    .map(P2pRpcResponse::Transaction)
                    .map(Box::new);

                dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                    peer_id,
                    id,
                    response,
                });
            }
            P2pRpcRequest::CompletedSnarks => {
                // OCaml rpcs.ml:815-834 takes the first 32 completed jobs in
                // work-selector order, or None without a frontier/pool.
                // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/rpcs.ml#L815-L834>
                let response = state
                    .transition_frontier
                    .best_tip()
                    .and_then(|_| state.snark_pool.completed_snarks_for_rpc())
                    .map(|works| P2pRpcResponse::CompletedSnarks(bounded_completed_snarks(works)));
                if let Some(P2pRpcResponse::CompletedSnarks(works)) = response.as_ref() {
                    mina_core::info!(meta.time(); kind = "P2pCompletedSnarksServed",
                        peer_id = display(peer_id), count = works.0.len());
                }
                dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                    peer_id,
                    id,
                    response: response.map(Box::new),
                });
            }
            P2pRpcRequest::Snark(job_id) => {
                let job = state.snark_pool.get(&job_id);
                let response = job
                    .and_then(|job| job.snark.as_ref())
                    .filter(|snark| {
                        state.transition_frontier.best_tip().is_some_and(|tip| {
                            state.snark_pool.work_is_checked(&snark.work, tip.hash())
                        })
                    })
                    .map(|snark| snark.work.clone())
                    .map(P2pRpcResponse::Snark)
                    .map(Box::new);

                dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                    peer_id,
                    id,
                    response,
                });
            }
            P2pRpcRequest::InitialPeers => {
                let p2p = p2p_ready!(state.p2p, meta.time());
                let peers = p2p
                    .peers
                    .iter()
                    .filter_map(|(_, v)| v.dial_opts.clone())
                    .collect();
                let response = Some(Box::new(P2pRpcResponse::InitialPeers(peers)));

                dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                    peer_id,
                    id,
                    response,
                });
            }
        }
    }

    fn handle_rpc_channels_response(
        dispatcher: &mut Dispatcher<Action, State>,
        meta: ActionMeta,
        id: u64,
        peer_id: PeerId,
        request: Option<&P2pRpcRequest>,
        response: &Option<Box<P2pRpcResponse>>,
        arrival: crate::verification::Arrival,
    ) {
        // Non-ledger callbacks can follow a best-tip update that already
        // reused the RPC slot. Preserve their existing response handling;
        // ledger answers require the matching recorded request and id.
        if matches!(
            response.as_deref(),
            Some(P2pRpcResponse::LedgerQuery(..) | P2pRpcResponse::LedgerQueryError(..))
        ) && !matches!(request, Some(P2pRpcRequest::LedgerQuery(..)))
        {
            return;
        }
        if let Some(P2pRpcRequest::LedgerQuery(_, query)) = request {
            use v2::MinaLedgerSyncLedgerQueryStableV1 as Query;
            // The transport has already answered. Match the constructor to
            // the recorded query before marking any ledger attempt Success;
            // a wrong-kind answer has no remaining transport timeout.
            // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/syncable_ledger/syncable_ledger.ml#L724-L832>
            let matches_query = match response.as_deref() {
                None | Some(P2pRpcResponse::LedgerQueryError(..)) => true,
                Some(P2pRpcResponse::LedgerQuery(answer)) => matches!(
                    (query, answer),
                    (
                        Query::NumAccounts,
                        MinaLedgerSyncLedgerAnswerStableV2::NumAccounts(..)
                    ) | (
                        Query::WhatChildHashes(..),
                        MinaLedgerSyncLedgerAnswerStableV2::ChildHashesAre(..)
                    ) | (
                        Query::WhatContents(..),
                        MinaLedgerSyncLedgerAnswerStableV2::ContentsAre(..)
                    )
                ),
                _ => false,
            };
            if !matches_query {
                let error = PeerLedgerQueryError::InvalidResponse;
                dispatcher.push(match query {
                    Query::NumAccounts => {
                        TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsError {
                            peer_id,
                            rpc_id: id,
                            error,
                        }
                    }
                    _ => TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressError {
                        peer_id,
                        rpc_id: id,
                        error,
                    },
                });
                return;
            }
        }
        match response.as_deref() {
            None | Some(P2pRpcResponse::LedgerQueryError(..)) => {
                let ledger_error = if matches!(
                    response.as_deref(),
                    Some(P2pRpcResponse::LedgerQueryError(..))
                ) {
                    PeerLedgerQueryError::LedgerUnavailable
                } else {
                    PeerLedgerQueryError::DataUnavailable
                };
                match request {
                    Some(P2pRpcRequest::Transaction(hash)) => {
                        let hash = hash.clone();
                        dispatcher
                            .push(TransactionPoolCandidateAction::FetchError { peer_id, hash });
                        return;
                    }
                    Some(P2pRpcRequest::CompletedSnarks) => {
                        mina_core::debug!(meta.time(); kind = "SnarkPoolCompletedSnarksUnavailable", peer_id = display(peer_id));
                        return;
                    }
                    Some(P2pRpcRequest::Snark(job_id)) => {
                        let job_id = job_id.clone();
                        dispatcher
                            .push(SnarkPoolCandidateAction::WorkFetchError { peer_id, job_id });
                        return;
                    }
                    _ => {}
                }

                dispatcher.push(
                    TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsError {
                        peer_id,
                        rpc_id: id,
                        error: ledger_error.clone(),
                    },
                );
                dispatcher.push(
                    TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressError {
                        peer_id,
                        rpc_id: id,
                        error: ledger_error,
                    },
                );
                dispatcher.push(
                    TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchError {
                        peer_id,
                        rpc_id: id,
                        error: PeerStagedLedgerPartsFetchError::DataUnavailable,
                    },
                );
                dispatcher.push(TransitionFrontierSyncAction::BlocksPeerQueryError {
                    peer_id,
                    rpc_id: id,
                    error: PeerBlockFetchError::DataUnavailable,
                });
            }
            Some(P2pRpcResponse::BestTipWithProof(resp)) => {
                let (body_hashes, root_block) = &resp.proof;

                let (Ok(best_tip), Ok(root_block)) = (
                    BlockWithHash::try_new(resp.best_tip.clone()),
                    BlockWithHash::try_new(root_block.clone()),
                ) else {
                    mina_core::error!(meta.time(); "P2pRpcResponse::BestTipWithProof: invalid blocks");
                    return;
                };

                // reconstruct hashes
                let Ok(hashes) = body_hashes
                    .iter()
                    .take(body_hashes.len().saturating_sub(1))
                    .scan(root_block.hash.clone(), |pred_hash, body_hash| {
                        *pred_hash = match StateHash::try_from_hashes(pred_hash, body_hash) {
                            Ok(hash) => hash,
                            Err(_) => return Some(Err(InvalidBigInt)),
                        };
                        Some(Ok(pred_hash.clone()))
                    })
                    .collect::<Result<Vec<_>, _>>()
                else {
                    mina_core::error!(meta.time(); "P2pRpcResponse::BestTipWithProof: invalid hashes");
                    return;
                };

                if let Some(pred_hash) = hashes.last() {
                    let expected_hash = &best_tip.block.header.protocol_state.previous_state_hash;

                    if pred_hash != expected_hash {
                        mina_core::warn!(meta.time();
                        kind = "P2pRpcBestTipHashMismatch",
                        response = serde_json::to_string(&resp).ok(),
                        expected_hash = expected_hash.to_string(),
                        calculated_hash = pred_hash.to_string());
                        return;
                    }
                }
                dispatcher.push(TransitionFrontierCandidateAction::BlockChainProofUpdate {
                    hash: best_tip.hash,
                    chain_proof: (hashes, root_block),
                });
            }
            Some(P2pRpcResponse::LedgerQuery(answer)) => match answer {
                MinaLedgerSyncLedgerAnswerStableV2::ChildHashesAre(hashes) => {
                    // The hashes of a full subtree level, at most as deep as
                    // asked for: checked against the parent when received.
                    if hashes.len() >= 2 && hashes.len().is_power_of_two() {
                        dispatcher.push(
                            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressSuccess {
                                peer_id,
                                rpc_id: id,
                                response: PeerLedgerQueryResponse::ChildHashes(
                                    hashes.iter().cloned().collect(),
                                ),
                            },
                        );
                    } else {
                        dispatcher.push(
                            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressError {
                                peer_id,
                                rpc_id: id,
                                error: PeerLedgerQueryError::DataUnavailable,
                            },
                        );
                    }
                }
                MinaLedgerSyncLedgerAnswerStableV2::ContentsAre(accounts) => {
                    dispatcher.push(
                        TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressSuccess {
                            peer_id,
                            rpc_id: id,
                            response: PeerLedgerQueryResponse::ChildAccounts(
                                accounts.iter().cloned().collect(),
                            ),
                        },
                    );
                }
                MinaLedgerSyncLedgerAnswerStableV2::NumAccounts(count, contents_hash) => {
                    dispatcher.push(
                        TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsSuccess {
                            peer_id,
                            rpc_id: id,
                            response: PeerLedgerQueryResponse::NumAccounts(
                                count.as_u64(),
                                contents_hash.clone(),
                            ),
                        },
                    );
                }
            },
            Some(P2pRpcResponse::StagedLedgerAuxAndPendingCoinbasesAtBlock(parts)) => {
                dispatcher.push(
                    TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchSuccess {
                        peer_id,
                        rpc_id: id,
                        parts: parts.clone(),
                    },
                );
            }
            Some(P2pRpcResponse::Block(block)) => {
                let Ok(block) = BlockWithHash::try_new(block.clone()) else {
                    mina_core::error!(meta.time(); "P2pRpcResponse::Block: invalid block");
                    return;
                };
                // The hash we asked for covers the protocol state alone. As
                // OCaml's catch-up does, check the rest of the header and the
                // body here, so that another peer is asked for the block
                // instead of it failing to apply and its hash being
                // blacklisted.
                // OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/ledger_catchup/super_catchup.ml#L135-L143>
                let result = validate_delta_block_chain(block.header())
                    .and_then(|()| validate_body_reference(block.block.as_ref()));
                dispatcher.push(
                    crate::verification::VerificationUpdate::headers(&block, arrival, &result)
                        .action(),
                );
                if let Err(error) = result {
                    mina_core::warn!(meta.time();
                        kind = "P2pRpcResponseBlockInvalid",
                        summary = format!("block {} from {peer_id}: {error:?}", block.hash()));
                    dispatcher.push(TransitionFrontierSyncAction::BlocksPeerQueryError {
                        peer_id,
                        rpc_id: id,
                        error: PeerBlockFetchError::from_block_check(error),
                    });
                    return;
                }
                dispatcher.push(TransitionFrontierSyncAction::BlocksPeerQuerySuccess {
                    peer_id,
                    rpc_id: id,
                    response: block,
                });
            }
            Some(P2pRpcResponse::Transaction(transaction)) => {
                match TransactionWithHash::try_new(transaction.clone()) {
                    Err(err) => bug_condition!("tx hashing failed: {err}"),
                    Ok(transaction) => {
                        dispatcher.push(TransactionPoolCandidateAction::FetchSuccess {
                            peer_id,
                            transaction,
                        })
                    }
                }
            }
            Some(P2pRpcResponse::CompletedSnarks(works)) => {
                dispatcher.push(crate::SnarkPoolAction::CompletedSnarksReceived {
                    peer_id,
                    works: works.clone(),
                });
            }
            Some(P2pRpcResponse::Snark(snark)) => {
                dispatcher.push(crate::SnarkPoolAction::WorkCheck {
                    sender: peer_id,
                    work: snark.clone(),
                    is_sender_local: false,
                    stage: crate::snark_pool::AdmissionStage::Received(
                        crate::snark_pool::WorkSource::Rpc,
                    ),
                    retry: false,
                });
            }
            Some(P2pRpcResponse::InitialPeers(_) | P2pRpcResponse::Catchup(_)) => {}
        }
    }
}

enum PreValidationResult {
    Continue,
    Reject { reason: String },
    Ignore { reason: String },
}

#[cfg(test)]
mod d4f_tests {
    use super::*;
    use mina_p2p_messages::{rpc::GetCompletedSnarksV1, rpc_kernel::PayloadBinprotReader};

    #[test]
    fn completed_snarks_response_matches_ocaml_fixture_and_caps_extra_work() {
        let fixture =
            include_bytes!("../../../../p2p-messages/tests/files/mesa/get_completed_snarks_v1.bin");
        let wire_works = GetCompletedSnarksV1::response_payload(&mut fixture.as_slice())
            .unwrap()
            .unwrap();
        let works: Vec<mina_core::snark::Snark> =
            wire_works.0.into_iter().map(Into::into).collect();
        assert_eq!(works.len(), 32);

        let bounded = bounded_completed_snarks(works.iter());
        let bytes = mina_p2p::channels::rpc::response_bytes(Some(bounded));
        assert_eq!(bytes, fixture);
        assert!(bytes.len() <= mina_p2p_messages::rpc::MAX_COMPLETED_SNARKS_BYTES);

        let mut extra = works;
        extra.push(extra[0].clone());
        let bounded = bounded_completed_snarks(extra.iter());
        assert_eq!(bounded.0.len(), 32);

        let first: mina_p2p_messages::v2::TransactionSnarkWorkTStableV2 =
            extra[0].to_wire().unwrap();
        let mut first_bytes = Vec::new();
        first.binprot_write(&mut first_bytes).unwrap();
        let byte_limit = 64 + first_bytes.len();
        let bounded = bounded_completed_snarks_with_byte_limit(extra.iter(), byte_limit);
        assert_eq!(bounded.0.len(), 1);
        assert!(mina_p2p::channels::rpc::response_bytes(Some(bounded)).len() <= byte_limit);
    }
}
