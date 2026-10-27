use crate::p2p::{
    channels::{
        rpc::{P2pChannelsRpcAction, P2pRpcId, P2pRpcRequest, P2pRpcResponse},
        streaming_rpc::{P2pChannelsStreamingRpcAction, P2pStreamingRpcRequest},
    },
    P2pAction, PeerId,
};
use mina_core::{bug_condition, requests::RequestId};
use mina_p2p_messages::v2;
use redux::Dispatcher;

use crate::{
    block_producer::vrf_evaluator::BlockProducerVrfEvaluatorAction,
    ledger_effectful::LedgerEffectfulAction, Action, RpcAction, State, Substate,
};

use super::{
    served_child_hashes_depth, LedgerAddress, LedgerReadAction, LedgerReadActionWithMetaRef,
    LedgerReadIdType, LedgerReadInitCallback, LedgerReadRequest, LedgerReadResponse,
    LedgerReadStagedLedgerAuxAndPendingCoinbases, LedgerReadState,
};

impl LedgerReadState {
    pub fn reducer(mut state_context: Substate<Self>, action: LedgerReadActionWithMetaRef<'_>) {
        let (action, meta) = action.split();
        let Ok(state) = state_context.get_substate_mut() else {
            return;
        };

        match action {
            LedgerReadAction::FindTodos => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                Self::next_read_requests_init(dispatcher, state);
            }
            LedgerReadAction::Init { request, callback } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                if state.ledger.read.has_same_request(request) {
                    return;
                }

                let id = state.ledger.read.next_req_id();
                dispatcher.push(LedgerEffectfulAction::ReadInit {
                    request: request.clone(),
                    callback: callback.clone(),
                    id,
                });
            }
            LedgerReadAction::Pending { request, .. } => {
                state.add(meta.time(), request.clone());
                if matches!(request, LedgerReadRequest::WorkCoverage(_)) {
                    state_context
                        .into_dispatcher()
                        .push(crate::SnarkPoolAction::WorkCoveragePending);
                }
            }
            LedgerReadAction::Success { id, response } => {
                state.add_response(*id, meta.time(), response.clone());

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                Self::propagate_read_response(dispatcher, state, *id, response.clone());
                dispatcher.push(LedgerReadAction::Prune { id: *id });
            }
            LedgerReadAction::Prune { id } => {
                state.remove(*id);
            }
        }
    }

    fn propagate_read_response(
        dispatcher: &mut Dispatcher<Action, State>,
        state: &State,
        id: RequestId<LedgerReadIdType>,
        response: LedgerReadResponse,
    ) {
        let Some(request) = state.ledger.read.get(id) else {
            bug_condition!("Request with id: {} not found", id);
            return;
        };

        match (request.request(), response) {
            (
                LedgerReadRequest::SnarkPoolProver(request),
                LedgerReadResponse::SnarkPoolProver(result),
            ) => {
                dispatcher.push(crate::SnarkPoolAction::ProverAccountResult {
                    request: request.clone(),
                    result,
                });
            }
            (_, LedgerReadResponse::SnarkPoolProver(_)) => unreachable!(),
            (_, LedgerReadResponse::WorkCoverage(result)) => {
                dispatcher.push(crate::SnarkPoolAction::WorkCoverageResult { result });
            }
            (
                LedgerReadRequest::DelegatorTable(ledger_hash, pub_key),
                LedgerReadResponse::DelegatorTable(table),
            ) => {
                let expected = state.block_producer.vrf_delegator_table_inputs();
                if !expected.is_some_and(|(expected_hash, producer)| {
                    ledger_hash == expected_hash && pub_key == producer
                }) {
                    bug_condition!("delegator table unexpected");
                    return;
                }
                match table {
                    None => {
                        // TODO(tizoc): Revise this, may be better to dispatch a different action here
                        // and avoid running the VRF evaluator altogether when we know that the
                        // table is empty.
                        dispatcher.push(
                            BlockProducerVrfEvaluatorAction::FinalizeDelegatorTableConstruction {
                                delegator_table: Default::default(),
                            },
                        );
                    }
                    Some(table) => {
                        dispatcher.push(
                            BlockProducerVrfEvaluatorAction::FinalizeDelegatorTableConstruction {
                                delegator_table: table.into(),
                            },
                        );
                    }
                }
            }
            (_, LedgerReadResponse::DelegatorTable(..)) => unreachable!(),
            (req, LedgerReadResponse::GetNumAccounts(resp)) => {
                for (peer_id, id, _) in find_peers_with_ledger_rpc(state, req) {
                    dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                        peer_id,
                        id,
                        response: resp.as_ref().map(|(num_accounts, hash)| {
                            Box::new(P2pRpcResponse::LedgerQuery(
                                v2::MinaLedgerSyncLedgerAnswerStableV2::NumAccounts(
                                    (*num_accounts).into(),
                                    hash.clone(),
                                ),
                            ))
                        }),
                    });
                }
            }
            (req, LedgerReadResponse::GetChildHashesAtAddr(resp)) => {
                for (peer_id, id, _) in find_peers_with_ledger_rpc(state, req) {
                    dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                        peer_id,
                        id,
                        response: resp.as_ref().map(|hashes| {
                            Box::new(P2pRpcResponse::LedgerQuery(
                                v2::MinaLedgerSyncLedgerAnswerStableV2::ChildHashesAre(
                                    hashes.iter().cloned().collect(),
                                ),
                            ))
                        }),
                    });
                }
            }
            (
                req @ LedgerReadRequest::GetChildAccountsAtAddr(hash, _),
                LedgerReadResponse::GetChildAccountsAtAddr(resp),
            ) => {
                for (peer_id, id, _) in find_peers_with_ledger_rpc(state, req) {
                    let response = match &resp {
                        Ok(accounts) => P2pRpcResponse::LedgerQuery(
                            v2::MinaLedgerSyncLedgerAnswerStableV2::ContentsAre(
                                accounts.iter().cloned().collect(),
                            ),
                        ),
                        Err(error) => {
                            P2pRpcResponse::LedgerQueryError(hash.clone(), error.as_str().into())
                        }
                    };
                    dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                        peer_id,
                        id,
                        response: Some(Box::new(response)),
                    });
                }
            }
            (_, LedgerReadResponse::GetChildAccountsAtAddr(_)) => unreachable!(),
            (req, LedgerReadResponse::GetStagedLedgerAuxAndPendingCoinbases(resp)) => {
                for (peer_id, id, is_streaming) in find_peers_with_ledger_rpc(state, req) {
                    if is_streaming {
                        dispatcher.push(P2pChannelsStreamingRpcAction::ResponseSendInit {
                            peer_id,
                            id,
                            response: resp.clone().map(Into::into),
                        });
                    } else {
                        dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                            peer_id,
                            id,
                            response: resp.clone().map(|data| {
                                Box::new(P2pRpcResponse::StagedLedgerAuxAndPendingCoinbasesAtBlock(
                                    data,
                                ))
                            }),
                        });
                    }
                }
            }
            (
                LedgerReadRequest::ScanStateSummary(ledger_hash),
                LedgerReadResponse::ScanStateSummary(scan_state),
            ) => {
                for rpc_id in state
                    .rpc
                    .scan_state_summary_rpc_ids()
                    .filter(|(_, hash, _)| *hash == ledger_hash)
                    .map(|(id, ..)| id)
                    .collect::<Vec<_>>()
                {
                    dispatcher.push(RpcAction::ScanStateSummaryGetSuccess {
                        rpc_id,
                        scan_state: scan_state.clone(),
                    });
                }
            }
            (_, LedgerReadResponse::ScanStateSummary(..)) => unreachable!(),
            (_req, LedgerReadResponse::GetAccounts(..)) => todo!(),
            (_, LedgerReadResponse::AccountsForRpc(rpc_id, accounts, account_query)) => {
                dispatcher.push(RpcAction::LedgerAccountsGetSuccess {
                    rpc_id,
                    accounts,
                    account_query,
                });
            }
            (_, LedgerReadResponse::GetLedgerStatus(rpc_id, resp)) => {
                dispatcher.push(RpcAction::LedgerStatusGetSuccess {
                    rpc_id,
                    response: resp.clone(),
                });
            }
            (_, LedgerReadResponse::GetAccountDelegators(rpc_id, resp)) => {
                dispatcher.push(RpcAction::LedgerAccountDelegatorsGetSuccess {
                    rpc_id,
                    response: resp.clone(),
                });
            }
            (_, LedgerReadResponse::LedgerExport(rpc_id, response)) => {
                dispatcher.push(RpcAction::LedgerExportSuccess { rpc_id, response });
            }
            (_, LedgerReadResponse::HardForkExport(rpc_id, response)) => {
                dispatcher.push(RpcAction::HardForkConfigSuccess { rpc_id, response });
            }
            (_, LedgerReadResponse::LightAccount(rpc_id, response)) => {
                dispatcher.push(RpcAction::LightAccountSuccess { rpc_id, response });
            }
        }
    }

    fn next_read_requests_init(dispatcher: &mut Dispatcher<Action, State>, state: &State) {
        // fetching delegator table, this is required because delegator table construction requires reading from ledger.
        // It could be that ledger read quota was reached when vrf tried to initiate that read, so we need to "retry" it if that's the case
        dispatcher.push(BlockProducerVrfEvaluatorAction::BeginDelegatorTableConstruction);

        // p2p rpcs
        let mut peers = state
            .p2p
            .ready_peers_iter()
            .filter(|(_, peer)| {
                peer.channels
                    .rpc
                    .remote_todo_requests_iter()
                    .next()
                    .is_some()
                    || peer.channels.streaming_rpc.remote_todo_request().is_some()
            })
            .map(|(peer_id, peer)| (*peer_id, peer.channels.rpc_remote_last_responded()))
            .collect::<Vec<_>>();
        peers.sort_by_key(|(_, last_responded)| *last_responded);
        for (peer_id, _) in peers {
            // Loaded masks and the genesis frontier must not be served while
            // startup restore is still loading or replaying. Decline before
            // scheduling a read, including WebRTC's streamed staged ledger.
            // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/rpcs.ml#L293-L308>
            // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/rpcs.ml#L408-L440>
            if state.transition_frontier.persist_holds_sync() {
                let Some(peer) = state.p2p.get_ready_peer(&peer_id) else {
                    continue;
                };
                for req in peer.channels.rpc.remote_todo_requests_iter() {
                    let response = match &req.request {
                        P2pRpcRequest::LedgerQuery(hash, _) => Some(Box::new(
                            P2pRpcResponse::LedgerQueryError(hash.clone(), "No Frontier".into()),
                        )),
                        P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(_) => None,
                        _ => continue,
                    };
                    dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                        peer_id,
                        id: req.id,
                        response,
                    });
                }
                if let Some((id, _)) = peer.channels.streaming_rpc.remote_todo_request() {
                    dispatcher.push(P2pChannelsStreamingRpcAction::ResponsePending { peer_id, id });
                    dispatcher.push(P2pChannelsStreamingRpcAction::ResponseSendInit {
                        peer_id,
                        id,
                        response: None,
                    });
                }
                continue;
            }
            // Unknown/evicted blocks must terminate, including when they leave
            // the serving window while waiting for a ledger-read quota.
            // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/sync_handler/sync_handler.ml#L117-L167>
            if let Some(peer) = state.p2p.ready().and_then(|p| p.get_ready_peer(&peer_id)) {
                for req in peer.channels.rpc.remote_todo_requests_iter() {
                    if let P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(hash) =
                        &req.request
                    {
                        if serving_block(state, hash).is_none() {
                            dispatcher.push(P2pChannelsRpcAction::ResponseSend {
                                peer_id,
                                id: req.id,
                                response: None,
                            });
                        }
                    }
                }
                if let Some((id, P2pStreamingRpcRequest::StagedLedgerParts(hash))) =
                    peer.channels.streaming_rpc.remote_todo_request()
                {
                    if serving_block(state, hash).is_none() {
                        dispatcher
                            .push(P2pChannelsStreamingRpcAction::ResponsePending { peer_id, id });
                        dispatcher.push(P2pChannelsStreamingRpcAction::ResponseSendInit {
                            peer_id,
                            id,
                            response: None,
                        });
                    }
                }
            }
            let Some((id, request, is_streaming)) = None.or_else(|| {
                let peer = state.p2p.ready()?.get_ready_peer(&peer_id)?;
                let mut reqs = peer.channels.rpc.remote_todo_requests_iter();
                reqs.find_map(|req| {
                    let ledger_request = match &req.request {
                        P2pRpcRequest::LedgerQuery(hash, query) => match query {
                            v2::MinaLedgerSyncLedgerQueryStableV1::NumAccounts => {
                                LedgerReadRequest::GetNumAccounts(hash.clone())
                            }
                            v2::MinaLedgerSyncLedgerQueryStableV1::WhatChildHashes(addr, depth) => {
                                let addr = LedgerAddress::from(addr);
                                let depth = served_child_hashes_depth(&addr, depth.as_u64())?;
                                LedgerReadRequest::GetChildHashesAtAddr(hash.clone(), addr, depth)
                            }
                            v2::MinaLedgerSyncLedgerQueryStableV1::WhatContents(addr) => {
                                LedgerReadRequest::GetChildAccountsAtAddr(hash.clone(), addr.into())
                            }
                        },
                        P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(block_hash) => {
                            build_staged_ledger_parts_request(state, block_hash)?
                        }
                        _ => return None,
                    };

                    Some((req.id, ledger_request, false))
                })
                .or_else(|| {
                    let (id, req) = peer.channels.streaming_rpc.remote_todo_request()?;
                    let ledger_request = match req {
                        P2pStreamingRpcRequest::StagedLedgerParts(block_hash) => {
                            build_staged_ledger_parts_request(state, block_hash)?
                        }
                    };
                    Some((id, ledger_request, true))
                })
            }) else {
                continue;
            };

            dispatcher.push(LedgerReadAction::Init {
            request,
            callback: LedgerReadInitCallback::P2pChannelsResponsePending
         {      callback: redux::callback!(on_ledger_read_init_p2p_channels_response_pending((is_streaming: bool, id: P2pRpcId, peer_id: PeerId)) -> crate::Action{
                    if is_streaming {
                        P2pAction::from(P2pChannelsStreamingRpcAction::ResponsePending {
                            peer_id,
                            id,
                        })
                    } else {
                        P2pAction::from(P2pChannelsRpcAction::ResponsePending {
                            peer_id,
                            id,
                        })
                    }
                }),
                args:(is_streaming, id, peer_id)
        }
        });

            if !state.ledger.read.is_total_cost_under_limit() {
                return;
            }
        }

        // rpcs
        let rpcs = state
            .rpc
            .scan_state_summary_rpc_ids()
            .filter(|(.., status)| status.is_init())
            .map(|(id, ..)| id)
            .collect::<Vec<_>>();

        for rpc_id in rpcs {
            dispatcher.push(RpcAction::ScanStateSummaryLedgerGetInit { rpc_id });
            if !state.ledger.read.is_total_cost_under_limit() {
                return;
            }
        }

        // These reads normally start immediately, but a cached genesis can
        // hold their admission until restore/full loading finishes as well.
        let ledger_rpcs = state
            .rpc
            .requests
            .iter()
            .filter_map(|(rpc_id, request)| {
                if !request.status.is_init() {
                    return None;
                }
                match &request.req {
                    crate::rpc::RpcRequest::LedgerExport(kind) => {
                        Some(RpcAction::LedgerExportInit {
                            rpc_id: *rpc_id,
                            kind: *kind,
                        })
                    }
                    crate::rpc::RpcRequest::LedgerStatusGet(ledger_hash) => {
                        Some(RpcAction::LedgerStatusGetInit {
                            rpc_id: *rpc_id,
                            ledger_hash: ledger_hash.clone(),
                        })
                    }
                    crate::rpc::RpcRequest::LedgerAccountDelegatorsGet(query, account_id) => {
                        Some(RpcAction::LedgerAccountDelegatorsGetInit {
                            rpc_id: *rpc_id,
                            query: query.clone(),
                            account_id: account_id.clone(),
                        })
                    }
                    crate::rpc::RpcRequest::LightAccount(query) => {
                        Some(RpcAction::LightAccountInit {
                            rpc_id: *rpc_id,
                            query: query.clone(),
                        })
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        for action in ledger_rpcs {
            dispatcher.push(action);
            if !state.ledger.read.is_total_cost_under_limit() {
                return;
            }
        }

        let ledger_account_rpc = state
            .rpc
            .accounts_request_rpc_ids()
            .filter(|(.., status)| status.is_init())
            .map(|(id, req, _)| (id, req))
            .collect::<Vec<_>>();

        for (rpc_id, req) in ledger_account_rpc {
            dispatcher.push(RpcAction::LedgerAccountsGetInit {
                rpc_id,
                account_query: req,
            });
            if !state.ledger.read.is_total_cost_under_limit() {
                return;
            }
        }
    }
}

fn find_peers_with_ledger_rpc(
    state: &crate::State,
    req: &LedgerReadRequest,
) -> Vec<(PeerId, P2pRpcId, bool)> {
    let Some(p2p) = state.p2p.ready() else {
        return Vec::new();
    };
    p2p.ready_peers_iter()
        .flat_map(|(peer_id, peer)| {
            let rpcs = peer
                .channels
                .rpc
                .remote_pending_requests_iter()
                .map(move |req| (peer_id, req.id, &req.request))
                .filter(|(_, _, peer_req)| match (req, peer_req) {
                    (
                        LedgerReadRequest::GetNumAccounts(h1),
                        P2pRpcRequest::LedgerQuery(
                            h2,
                            v2::MinaLedgerSyncLedgerQueryStableV1::NumAccounts,
                        ),
                    ) => h1 == h2,
                    (
                        LedgerReadRequest::GetChildHashesAtAddr(h1, addr1, depth1),
                        P2pRpcRequest::LedgerQuery(
                            h2,
                            v2::MinaLedgerSyncLedgerQueryStableV1::WhatChildHashes(addr2, depth2),
                        ),
                    ) => {
                        h1 == h2
                            && addr1 == &LedgerAddress::from(addr2)
                            && Some(*depth1) == served_child_hashes_depth(addr1, depth2.as_u64())
                    }
                    (
                        LedgerReadRequest::GetChildAccountsAtAddr(h1, addr1),
                        P2pRpcRequest::LedgerQuery(
                            h2,
                            v2::MinaLedgerSyncLedgerQueryStableV1::WhatContents(addr2),
                        ),
                    ) => h1 == h2 && addr1 == &LedgerAddress::from(addr2),
                    (
                        LedgerReadRequest::GetStagedLedgerAuxAndPendingCoinbases(data),
                        P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(block_hash),
                    ) => &data.block_hash == block_hash,
                    _ => false,
                })
                .map(|(peer_id, rpc_id, _)| (*peer_id, rpc_id, false));
            let streaming_rpcs = peer
                .channels
                .streaming_rpc
                .remote_pending_request()
                .into_iter()
                .filter(|(_, peer_req)| match (req, peer_req) {
                    (
                        LedgerReadRequest::GetStagedLedgerAuxAndPendingCoinbases(data),
                        P2pStreamingRpcRequest::StagedLedgerParts(block_hash),
                    ) => &data.block_hash == block_hash,
                    _ => false,
                })
                .map(|(rpc_id, _)| (*peer_id, rpc_id, true));
            rpcs.chain(streaming_rpcs)
        })
        .collect()
}

fn build_staged_ledger_parts_request(
    state: &crate::State,
    block_hash: &v2::StateHash,
) -> Option<LedgerReadRequest> {
    let tf = &state.transition_frontier;
    let ledger_hash = serving_block(state, block_hash)?
        .staged_ledger_hashes()
        .clone();
    let protocol_states = tf
        .needed_protocol_states
        .iter()
        .map(|(hash, b)| (hash.clone(), b.clone()))
        .chain(
            tf.best_chain
                .iter()
                .take_while(|b| b.hash() != block_hash)
                .map(|b| (b.hash().clone(), b.header().protocol_state.clone())),
        )
        .collect();

    Some(LedgerReadRequest::GetStagedLedgerAuxAndPendingCoinbases(
        LedgerReadStagedLedgerAuxAndPendingCoinbases {
            block_hash: block_hash.clone(),
            ledger_hash,
            protocol_states,
        },
    ))
}

fn serving_block<'a>(
    state: &'a State,
    hash: &v2::StateHash,
) -> Option<&'a mina_core::block::ArcBlockWithHash> {
    state
        .transition_frontier
        .best_chain
        .iter()
        .find(|b| b.hash() == hash)
        .map(|b| &b.block)
        .or_else(|| state.transition_frontier.root_history.get(hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ledger::LedgerAction,
        p2p::{
            channels::{rpc::*, P2pChannelsAction},
            P2pPeerState, P2pPeerStatus, P2pPeerStatusReady,
        },
    };
    use mina_core::{consensus::ConsensusConstants, constants};
    use mina_snark::{BlockVerifier, TransactionVerifier};
    use redux::{EnablingCondition, Timestamp};
    fn state() -> State {
        let constants = ConsensusConstants::create(
            constants::constraint_constants(),
            &constants::PROTOCOL_CONSTANTS,
        );
        let config = crate::Config {
            ledger: crate::config::LedgerConfig {},
            snark: crate::config::SnarkConfig {
                block_verifier_index: BlockVerifier::make(),
                block_verifier_srs: mina_snark::get_srs(),
                work_verifier_index: TransactionVerifier::make(),
                work_verifier_srs: mina_snark::get_srs(),
            },
            p2p: crate::config::P2pConfig {
                libp2p_port: None,
                listen_port: None,
                identity_pub_key: crate::p2p::identity::SecretKey::deterministic(0).public_key(),
                initial_peers: vec![],
                external_addrs: vec![],
                enabled_channels: Default::default(),
                timeouts: Default::default(),
                limits: Default::default(),
                peer_discovery: false,
                meshsub: Default::default(),
                catchup_rpc: Default::default(),
            },
            transition_frontier: crate::config::TransitionFrontierConfig::new(
                crate::config::DEVNET_CONFIG.clone(),
            ),
            archive: None,
            block_producer: None,
            global: crate::config::GlobalConfig {
                build: Box::new(crate::config::BuildEnv::get()),
                snarker: None,
                consensus_constants: constants.clone(),
                client_port: None,
                testing_run: true,
                chain_id_override: None,
                skip_proof_verification: false,
                worker_control: false,
                snarker_strategy: Default::default(),
                block_first_lead: None,
            },
            tx_pool: ledger::transaction_pool::Config {
                trust_system: (),
                pool_max_size: 10,
                slot_tx_end: None,
            },
        };
        let mut state = State::new(config, &constants, Timestamp::ZERO);
        let chain_id = serde_json::from_str(
            r#""0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886""#,
        )
        .unwrap();
        state.p2p.initialize(&chain_id).unwrap();
        state.transition_frontier.sync =
            crate::transition_frontier::sync::TransitionFrontierSyncState::Synced {
                time: Timestamp::ZERO,
            };
        state
    }
    fn peer(state: &mut State, hash: v2::StateHash, is_pending: bool) -> PeerId {
        let peer_id = PeerId::from_bytes([5; 32]);
        let mut ready = P2pPeerStatusReady::new(false, Timestamp::ZERO, &Default::default());
        ready.channels.rpc = P2pChannelsRpcState::Ready {
            time: Timestamp::ZERO,
            local: P2pRpcLocalState::WaitingForRequest {
                time: Timestamp::ZERO,
            },
            remote: P2pRpcRemoteState {
                pending_requests: [P2pRpcRemotePendingRequestState {
                    time: Timestamp::ZERO,
                    id: 7,
                    request: P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(hash),
                    is_pending,
                }]
                .into_iter()
                .collect(),
                last_responded: Timestamp::ZERO,
            },
        };
        state.p2p.ready_mut().unwrap().peers.insert(
            peer_id,
            P2pPeerState {
                is_libp2p: false,
                dial_opts: None,
                identify: None,
                status: P2pPeerStatus::Ready(Box::new(ready)),
            },
        );
        peer_id
    }

    fn root() -> mina_core::block::ArcBlockWithHash {
        use mina_p2p_messages::rpc_kernel::PayloadBinprotReader;
        let bytes = include_bytes!("../../../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin");
        let block = mina_p2p_messages::rpc::GetBestTipV2::response_payload(&mut bytes.as_slice())
            .unwrap()
            .unwrap()
            .data;
        mina_core::block::BlockWithHash::try_new(std::sync::Arc::new(block)).unwrap()
    }

    #[test]
    fn unavailable_aux_schedules_terminal_response_and_retained_aux_schedules_read() {
        let mut state = state();
        let root = root();
        let peer_id = peer(&mut state, root.hash().clone(), false);
        let mut dispatcher = Dispatcher::new();
        LedgerReadState::next_read_requests_init(&mut dispatcher, &state);
        let mut responses = 0;
        while let Some(action) = dispatcher.pop() {
            if let Action::P2p(P2pAction::Channels(P2pChannelsAction::Rpc(
                P2pChannelsRpcAction::ResponseSend {
                    peer_id: p,
                    id: 7,
                    response: None,
                },
            ))) = &action
            {
                assert_eq!(*p, peer_id);
                assert!(action.is_enabled(&state, Timestamp::ZERO));
                responses += 1;
            }
            assert!(!matches!(
                action,
                Action::Ledger(LedgerAction::Read(LedgerReadAction::Init { .. }))
            ));
        }
        assert_eq!(responses, 1);
        state.transition_frontier.root_history.0.push(root);
        LedgerReadState::next_read_requests_init(&mut dispatcher, &state);
        let mut reads = 0;
        while let Some(action) = dispatcher.pop() {
            if matches!(
                action,
                Action::Ledger(LedgerAction::Read(LedgerReadAction::Init {
                    request: LedgerReadRequest::GetStagedLedgerAuxAndPendingCoinbases(_),
                    ..
                }))
            ) {
                reads += 1;
            }
            assert!(!matches!(
                action,
                Action::P2p(P2pAction::Channels(P2pChannelsAction::Rpc(
                    P2pChannelsRpcAction::ResponseSend { .. }
                )))
            ));
        }
        assert_eq!(reads, 1);
    }

    #[test]
    fn auxiliary_read_correlation_survives_history_eviction() {
        let mut state = state();
        let root = root();
        state.transition_frontier.root_history.0.push(root.clone());
        let request = build_staged_ledger_parts_request(&state, root.hash()).unwrap();
        let peer_id = peer(&mut state, root.hash().clone(), true);
        state.transition_frontier.root_history.0.clear();
        assert_eq!(
            find_peers_with_ledger_rpc(&state, &request),
            vec![(peer_id, 7, false)]
        );
    }
}
