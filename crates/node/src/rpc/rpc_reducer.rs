use crate::p2p::{
    connection::{incoming::P2pConnectionIncomingAction, outgoing::P2pConnectionOutgoingAction},
    webrtc::P2pConnectionResponse,
    PeerId,
};
use ledger::scan_state::transaction_logic::valid;
use mina_core::{
    block::AppliedBlock,
    bug_condition,
    requests::{RequestId, RpcId, RpcIdType},
    transaction::{TransactionPoolMessageSource, TransactionWithHash},
};
use mina_p2p_messages::v2::{
    MinaBaseSignedCommandStableV2, MinaBaseZkappCommandTStableV1WireStableV1, NonZeroCurvePoint,
};
use redux::ActionWithMeta;

use crate::{
    ledger::read::{LedgerReadAction, LedgerReadInitCallback, LedgerReadRequest},
    p2p_ready,
    rpc::{AccountQuery, GetBlockQuery, PooledCommandsQuery},
    rpc_effectful::RpcEffectfulAction,
    TransactionPoolAction,
};

use super::{
    ConsensusTimeQuery, HardForkBlockSpec, HardForkExportRequest, LedgerExportBestTip,
    LedgerExportKind, LedgerExportRequest, LightAccountRead, LightBlockRef, PeerConnectionStatus,
    RpcAction, RpcLightAccountQuery, RpcLightError, RpcPeerInfo, RpcRequest, RpcRequestExtraData,
    RpcRequestState, RpcRequestStatus, RpcScanStateSummaryGetQuery, RpcSnarkerConfig, RpcState,
    MAX_LIGHT_ACCOUNTS_PER_REQUEST, MAX_PENDING_LIGHT_ACCOUNTS, MAX_PENDING_LIGHT_REQUESTS,
    TRANSACTION_INJECT_BOOTSTRAPPING,
};

impl RpcState {
    pub fn reducer(mut state_context: crate::Substate<Self>, action: ActionWithMeta<&RpcAction>) {
        let Ok(state) = state_context.get_substate_mut() else {
            return;
        };

        let (action, meta) = action.split();
        match action {
            RpcAction::GlobalStateGet { rpc_id, filter } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::GlobalStateGet {
                    rpc_id: *rpc_id,
                    filter: filter.clone(),
                });
            }
            RpcAction::StatusGet { rpc_id } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::StatusGet { rpc_id: *rpc_id });
            }
            RpcAction::MemoryDebugGet { rpc_id } => {
                state_context
                    .into_dispatcher()
                    .push(RpcEffectfulAction::MemoryDebugGet { rpc_id: *rpc_id });
            }
            RpcAction::MetricsGet { rpc_id } => {
                state_context
                    .into_dispatcher()
                    .push(RpcEffectfulAction::MetricsGet { rpc_id: *rpc_id });
            }
            RpcAction::HeartbeatGet { rpc_id } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::HeartbeatGet { rpc_id: *rpc_id });
            }
            RpcAction::ActionStatsGet { rpc_id, query } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::ActionStatsGet {
                    rpc_id: *rpc_id,
                    query: *query,
                });
            }
            RpcAction::SyncStatsGet { rpc_id, query } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::SyncStatsGet {
                    rpc_id: *rpc_id,
                    query: *query,
                });
            }
            RpcAction::VerificationStatsGet { rpc_id, query } => {
                state_context
                    .into_dispatcher()
                    .push(RpcEffectfulAction::VerificationStatsGet {
                        rpc_id: *rpc_id,
                        query: query.clone(),
                    });
            }
            RpcAction::SnarkWorkCoverageGet { rpc_id } => {
                state_context
                    .into_dispatcher()
                    .push(RpcEffectfulAction::SnarkWorkCoverageGet { rpc_id: *rpc_id });
            }
            RpcAction::ProofMemoryGet { rpc_id } => {
                state_context
                    .into_dispatcher()
                    .push(RpcEffectfulAction::ProofMemoryGet { rpc_id: *rpc_id });
            }
            RpcAction::BlockProducerStatsGet { rpc_id } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::BlockProducerStatsGet { rpc_id: *rpc_id });
            }
            RpcAction::MessageProgressGet { rpc_id } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::MessageProgressGet { rpc_id: *rpc_id });
            }
            RpcAction::PeersGet { rpc_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let peers = collect_rpc_peers_info(state);
                dispatcher.push(RpcEffectfulAction::PeersGet {
                    rpc_id: *rpc_id,
                    peers,
                });
            }
            RpcAction::P2pConnectionOutgoingInit { rpc_id, opts } => {
                let rpc_state = RpcRequestState {
                    req: RpcRequest::P2pConnectionOutgoing(opts.clone()),
                    status: RpcRequestStatus::Init { time: meta.time() },
                    data: Default::default(),
                };
                state.requests.insert(*rpc_id, rpc_state);

                let dispatcher = state_context.into_dispatcher();

                dispatcher.push(P2pConnectionOutgoingAction::Init {
                    opts: opts.clone(),
                    rpc_id: Some(*rpc_id),
                    on_success: Some(redux::callback!(
                        on_p2p_connection_outgoing_rpc_connection_success((peer_id: PeerId, rpc_id: Option<RpcId>)) -> crate::Action {
                            let Some(rpc_id) = rpc_id else {
                                unreachable!("RPC ID not provided");
                            };

                            RpcAction::P2pConnectionOutgoingPending{ rpc_id }
                        }
                    )),
                });
            }
            RpcAction::P2pConnectionOutgoingPending { rpc_id } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    bug_condition!(
                        "Rpc state not found for RpcAction::P2pConnectionOutgoingPending({})",
                        rpc_id
                    );
                    return;
                };
                rpc.status = RpcRequestStatus::Pending { time: meta.time() };
            }
            RpcAction::P2pConnectionOutgoingError { rpc_id, error } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    bug_condition!(
                        "Rpc state not found for RpcAction::P2pConnectionOutgoingError({})",
                        rpc_id
                    );
                    return;
                };
                rpc.status = RpcRequestStatus::Error {
                    time: meta.time(),
                    error: format!("{:?}", error),
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::P2pConnectionOutgoingError {
                    rpc_id: *rpc_id,
                    error: format!("{:?}", error),
                });
            }
            RpcAction::P2pConnectionOutgoingSuccess { rpc_id } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    bug_condition!(
                        "Rpc state not found for RpcAction::P2pConnectionOutgoingSuccess({})",
                        rpc_id
                    );
                    return;
                };
                rpc.status = RpcRequestStatus::Success { time: meta.time() };
                let dispatcher = state_context.into_dispatcher();
                dispatcher
                    .push(RpcEffectfulAction::P2pConnectionOutgoingSuccess { rpc_id: *rpc_id });
            }
            RpcAction::P2pConnectionIncomingInit { rpc_id, opts } => {
                let rpc_state = RpcRequestState {
                    req: RpcRequest::P2pConnectionIncoming(opts.clone()),
                    status: RpcRequestStatus::Init { time: meta.time() },
                    data: Default::default(),
                };
                state.requests.insert(*rpc_id, rpc_state);

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let p2p = p2p_ready!(state.p2p, meta.time());

                match p2p.incoming_accept(opts.peer_id, &opts.offer) {
                    Ok(_) => {
                        dispatcher.push(P2pConnectionIncomingAction::Init {
                            opts: opts.clone(),
                            rpc_id: Some(*rpc_id),
                        });
                        dispatcher
                            .push(RpcAction::P2pConnectionIncomingPending { rpc_id: *rpc_id });
                    }
                    Err(reason) => {
                        let response = P2pConnectionResponse::Rejected(reason);
                        dispatcher.push(RpcAction::P2pConnectionIncomingRespond {
                            rpc_id: *rpc_id,
                            response,
                        });
                    }
                }
            }
            RpcAction::P2pConnectionIncomingPending { rpc_id } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    bug_condition!(
                        "Rpc state not found for RpcAction::P2pConnectionIncomingPending({})",
                        rpc_id
                    );
                    return;
                };
                rpc.status = RpcRequestStatus::Pending { time: meta.time() };
            }
            RpcAction::P2pConnectionIncomingRespond { rpc_id, response } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::P2pConnectionIncomingRespond {
                    rpc_id: *rpc_id,
                    response: response.clone(),
                });
            }
            RpcAction::P2pConnectionIncomingError { rpc_id, error } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    bug_condition!(
                        "Rpc state not found for RpcAction::P2pConnectionIncomingError({})",
                        rpc_id
                    );
                    return;
                };
                rpc.status = RpcRequestStatus::Error {
                    time: meta.time(),
                    error: format!("{:?}", error),
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::P2pConnectionIncomingError {
                    rpc_id: *rpc_id,
                    error: error.to_owned(),
                });
            }
            RpcAction::P2pConnectionIncomingSuccess { rpc_id } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    bug_condition!(
                        "Rpc state not found for RpcAction::P2pConnectionIncomingSuccess({})",
                        rpc_id
                    );
                    return;
                };
                rpc.status = RpcRequestStatus::Success { time: meta.time() };
                let dispatcher = state_context.into_dispatcher();
                dispatcher
                    .push(RpcEffectfulAction::P2pConnectionIncomingSuccess { rpc_id: *rpc_id });
            }
            RpcAction::ScanStateSummaryGetInit { rpc_id, query } => {
                let rpc_state = RpcRequestState {
                    req: RpcRequest::ScanStateSummaryGet(query.clone()),
                    status: RpcRequestStatus::Init { time: meta.time() },
                    data: Default::default(),
                };
                state.requests.insert(*rpc_id, rpc_state);

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcAction::ScanStateSummaryLedgerGetInit { rpc_id: *rpc_id });
            }
            RpcAction::ScanStateSummaryLedgerGetInit { rpc_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let transition_frontier = &state.transition_frontier;

                let Some(query) = None.or_else(|| {
                    let req = state.rpc.requests.get(rpc_id)?;
                    match &req.req {
                        RpcRequest::ScanStateSummaryGet(query) => Some(query),
                        _ => None,
                    }
                }) else {
                    return;
                };

                let block = match query {
                    RpcScanStateSummaryGetQuery::ForBestTip => {
                        transition_frontier.best_tip_breadcrumb()
                    }
                    RpcScanStateSummaryGetQuery::ForBlockWithHash(hash) => transition_frontier
                        .best_chain
                        .iter()
                        .rev()
                        .find(|b| b.hash() == hash),
                    RpcScanStateSummaryGetQuery::ForBlockWithHeight(height) => transition_frontier
                        .best_chain
                        .iter()
                        .rev()
                        .find(|b| b.height() == *height),
                };
                let block = match block {
                    Some(v) => v.clone(),
                    None => {
                        dispatcher.push(RpcAction::ScanStateSummaryGetPending {
                            rpc_id: *rpc_id,
                            block: None,
                        });
                        dispatcher.push(RpcAction::ScanStateSummaryGetSuccess {
                            rpc_id: *rpc_id,
                            scan_state: Ok(Vec::new()),
                        });
                        return;
                    }
                };

                dispatcher.push(LedgerReadAction::Init {
                    request: LedgerReadRequest::ScanStateSummary(
                        block.staged_ledger_hashes().clone(),
                    ),
                    callback: LedgerReadInitCallback::RpcScanStateSummaryGetPending {
                        callback: redux::callback!(
                            on_ledger_read_init_rpc_scan_state_summary_get_pending((rpc_id: RequestId<RpcIdType>, block: AppliedBlock)) -> crate::Action{
                                RpcAction::ScanStateSummaryGetPending { rpc_id, block: Some(block) }
                            }
                        ),
                        args: (*rpc_id, block),
                    },
                });
            }
            RpcAction::ScanStateSummaryGetPending { rpc_id, block } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    bug_condition!(
                        "Rpc state not found for RpcAction::ScanStateSummaryGetPending({})",
                        rpc_id
                    );
                    return;
                };
                rpc.status = RpcRequestStatus::Pending { time: meta.time() };
                rpc.data = RpcRequestExtraData::FullBlockOpt(block.clone());
            }
            RpcAction::ScanStateSummaryGetSuccess { rpc_id, scan_state } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    bug_condition!(
                        "Rpc state not found for RpcAction::ScanStateSummaryGetSuccess({})",
                        rpc_id
                    );
                    return;
                };
                rpc.status = RpcRequestStatus::Success { time: meta.time() };
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::ScanStateSummaryGetSuccess {
                    rpc_id: *rpc_id,
                    scan_state: scan_state.clone(),
                });
                // This one's effectful handler reads the request's block, so
                // the entry must stay until it has run: Finish is queued
                // after the effectful action, and the store runs an action's
                // effects before the next queued action.
                dispatcher.push(RpcAction::Finish { rpc_id: *rpc_id });
            }
            RpcAction::SnarkPoolAvailableJobsGet { rpc_id } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::SnarkPoolAvailableJobsGet { rpc_id: *rpc_id });
            }
            RpcAction::SnarkPoolJobGet { rpc_id, job_id } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::SnarkPoolJobGet {
                    rpc_id: *rpc_id,
                    job_id: job_id.clone(),
                });
            }
            RpcAction::SnarkPoolCompletedJobsGet { rpc_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                let jobs = state
                    .snark_pool
                    .completed_snarks_iter()
                    .filter_map(|s| s.to_wire().ok())
                    .collect::<Vec<_>>();

                dispatcher.push(RpcEffectfulAction::SnarkPoolCompletedJobsGet {
                    rpc_id: *rpc_id,
                    jobs,
                });
            }
            RpcAction::SnarkPoolPendingJobsGet { rpc_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                let jobs = state
                    .snark_pool
                    .available_jobs_iter()
                    .cloned()
                    .collect::<Vec<_>>();

                dispatcher.push(RpcEffectfulAction::SnarkPoolPendingJobsGet {
                    rpc_id: *rpc_id,
                    jobs,
                })
            }
            RpcAction::SnarkerConfigGet { rpc_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                let config = state
                    .config
                    .snarker
                    .as_ref()
                    .map(|config| RpcSnarkerConfig {
                        public_key: config.public_key.as_ref().clone(),
                        fee: config.fee.clone(),
                    });

                dispatcher.push(RpcEffectfulAction::SnarkerConfigGet {
                    rpc_id: *rpc_id,
                    config,
                });
            }
            RpcAction::SnarkerJobCommit { rpc_id, job_id } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::SnarkerJobCommit {
                    rpc_id: *rpc_id,
                    job_id: job_id.clone(),
                });
            }
            RpcAction::SnarkerJobSpec { rpc_id, job_id } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::SnarkerJobSpec {
                    rpc_id: *rpc_id,
                    job_id: job_id.clone(),
                });
            }
            RpcAction::SnarkerWorkersGet { rpc_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let worker = &state.external_snark_worker.0;
                let snark_worker = Box::new(super::RpcSnarkWorker {
                    time: Some(worker.timestamp),
                    id: Some("single".into()),
                    status: worker.state.clone().into(),
                });
                dispatcher.push(RpcEffectfulAction::SnarkerWorkersGet {
                    rpc_id: *rpc_id,
                    snark_worker,
                });
            }
            RpcAction::SnarkWorkerControlGet { rpc_id } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::SnarkWorkerControlGet { rpc_id: *rpc_id });
            }
            RpcAction::SnarkWorkerControlSet { rpc_id, setting } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                // Refused unless the operator allowed it at startup: it
                // changes where fees go (P5.4).
                if !state.config.worker_control {
                    dispatcher.push(RpcEffectfulAction::SnarkWorkerControlSet {
                        rpc_id: *rpc_id,
                        error: Some(super::WORKER_CONTROL_NOT_ALLOWED.to_owned()),
                    });
                    return;
                }
                dispatcher.push(crate::ExternalSnarkWorkerAction::Configure {
                    setting: setting.clone(),
                });
                // Answered after the change, with the worker as it is then.
                dispatcher.push(RpcEffectfulAction::SnarkWorkerControlSet {
                    rpc_id: *rpc_id,
                    error: None,
                });
            }
            RpcAction::SnarkWorkIncludedGet { rpc_id } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::SnarkWorkIncludedGet { rpc_id: *rpc_id });
            }
            RpcAction::HealthCheck { rpc_id } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::HealthCheck { rpc_id: *rpc_id });
            }
            RpcAction::ReadinessCheck { rpc_id } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::ReadinessCheck { rpc_id: *rpc_id });
            }
            RpcAction::DiscoveryRoutingTable { rpc_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                let response = state
                    .p2p
                    .ready()
                    .and_then(|p2p| p2p.network.scheduler.discovery_state())
                    .and_then(|discovery_state| {
                        match (&discovery_state.routing_table).try_into() {
                            Ok(resp) => Some(resp),
                            Err(err) => {
                                bug_condition!(
                                    "{:?} error converting routing table into response: {:?}",
                                    err,
                                    action
                                );
                                None
                            }
                        }
                    });

                dispatcher.push(RpcEffectfulAction::DiscoveryRoutingTable {
                    rpc_id: *rpc_id,
                    response,
                });
            }
            RpcAction::DiscoveryBoostrapStats { rpc_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                let response = state
                    .p2p
                    .ready()
                    .and_then(|p2p| p2p.network.scheduler.discovery_state())
                    .and_then(|discovery_state: &crate::p2p::P2pNetworkKadState| {
                        discovery_state.bootstrap_stats().cloned()
                    });

                dispatcher.push(RpcEffectfulAction::DiscoveryBoostrapStats {
                    rpc_id: *rpc_id,
                    response,
                });
            }
            RpcAction::Finish { rpc_id } => {
                state.requests.remove(rpc_id);
            }
            RpcAction::TransactionPool { rpc_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let response = state.transaction_pool.get_all_transactions();
                dispatcher.push(RpcEffectfulAction::TransactionPool {
                    rpc_id: *rpc_id,
                    response,
                });
            }
            RpcAction::LedgerAccountsGetInit {
                rpc_id,
                account_query,
            } => {
                let rpc_state = RpcRequestState {
                    req: RpcRequest::LedgerAccountsGet(account_query.clone()),
                    status: RpcRequestStatus::Init { time: meta.time() },
                    data: Default::default(),
                };
                state.requests.insert(*rpc_id, rpc_state);

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let Some(best_tip) = state.transition_frontier.best_tip() else {
                    return;
                };
                let ledger_hash = match account_query {
                    AccountQuery::StakingEpochIds(..) => best_tip.staking_epoch_ledger_hash(),
                    _ => best_tip.merkle_root_hash(),
                };

                dispatcher.push(LedgerReadAction::Init {
                    request: LedgerReadRequest::AccountsForRpc(
                        *rpc_id,
                        ledger_hash.clone(),
                        account_query.clone(),
                    ),
                    callback: LedgerReadInitCallback::RpcLedgerAccountsGetPending {
                        callback: redux::callback!(
                            on_ledger_read_init_rpc_actions_get_init(rpc_id: RequestId<RpcIdType>) -> crate::Action{
                                RpcAction::LedgerAccountsGetPending { rpc_id }
                            }
                        ),
                        args: *rpc_id,
                    },
                })
            }
            RpcAction::LedgerAccountsGetPending { rpc_id } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Pending { time: meta.time() };
            }
            RpcAction::LedgerAccountsGetSuccess {
                rpc_id,
                account_query,
                accounts,
            } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Success { time: meta.time() };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::LedgerAccountsGetSuccess {
                    rpc_id: *rpc_id,
                    account_query: account_query.clone(),
                    accounts: accounts.clone(),
                });
                // The request is answered: its entry leaves the map when the
                // response above has been sent. The effectful handler does
                // not read the entry, and Finish runs only after it.
                dispatcher.push(RpcAction::Finish { rpc_id: *rpc_id });
            }
            RpcAction::TransactionInjectInit { rpc_id, commands } => {
                let rpc_state = RpcRequestState {
                    req: RpcRequest::TransactionInject(commands.clone()),
                    status: RpcRequestStatus::Init { time: meta.time() },
                    data: Default::default(),
                };
                state.requests.insert(*rpc_id, rpc_state);

                let commands_with_hash = commands
                    .clone()
                    .into_iter()
                    .map(TransactionWithHash::try_new)
                    .collect::<Result<_, _>>();

                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                dispatcher.push(RpcAction::TransactionInjectPending { rpc_id: *rpc_id });

                // Every request gets exactly one answer: refuse here what the
                // pool would otherwise drop without one.
                let refusal = if global_state
                    .transition_frontier
                    .is_bootstrapping(meta.time())
                {
                    // OCaml's answer, whenever it has no transition frontier.
                    // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L351-L366>
                    Some(TRANSACTION_INJECT_BOOTSTRAPPING.to_owned())
                } else if commands.is_empty() {
                    Some("no transaction to submit".to_owned())
                } else {
                    commands_with_hash
                        .as_ref()
                        .err()
                        .map(|e| format!("invalid transaction: {e}"))
                };

                match (refusal, commands_with_hash) {
                    (None, Ok(commands_with_hash)) => {
                        dispatcher.push(TransactionPoolAction::StartVerify {
                            commands: commands_with_hash,
                            from_source: TransactionPoolMessageSource::rpc(*rpc_id),
                        });
                    }
                    (refusal, _) => {
                        dispatcher.push(RpcAction::TransactionInjectFailure {
                            rpc_id: *rpc_id,
                            errors: vec![refusal.unwrap_or_default()],
                        });
                    }
                }
            }
            RpcAction::TransactionInjectPending { rpc_id } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Pending { time: meta.time() };
            }
            RpcAction::TransactionInjectSuccess { rpc_id, response } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Success { time: meta.time() };

                let dispatcher = state_context.into_dispatcher();
                let response = response.clone().into_iter().map(|cmd| cmd.data).collect();
                dispatcher.push(RpcEffectfulAction::TransactionInjectSuccess {
                    rpc_id: *rpc_id,
                    response,
                });
                // A submission gets one answer, this one: its entry leaves the
                // map after the response above is sent, which doesn't read it.
                dispatcher.push(RpcAction::Finish { rpc_id: *rpc_id });
            }
            RpcAction::TransactionInjectRejected { rpc_id, response } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Success { time: meta.time() };

                let dispatcher = state_context.into_dispatcher();
                let response = response
                    .clone()
                    .into_iter()
                    .map(|(cmd, failure)| (cmd.data, failure))
                    .collect();

                dispatcher.push(RpcEffectfulAction::TransactionInjectRejected {
                    rpc_id: *rpc_id,
                    response,
                });
                // As above: answered, so the entry leaves with the response.
                dispatcher.push(RpcAction::Finish { rpc_id: *rpc_id });
            }
            RpcAction::TransactionInjectFailure { rpc_id, errors } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Error {
                    time: meta.time(),
                    error: "Transaction injection failed".to_string(),
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::TransactionInjectFailure {
                    rpc_id: *rpc_id,
                    errors: errors.clone(),
                });
                // As above: answered, so the entry leaves with the response.
                dispatcher.push(RpcAction::Finish { rpc_id: *rpc_id });
            }
            RpcAction::TransitionFrontierUserCommandsGet { rpc_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                let commands = state
                    .transition_frontier
                    .best_chain
                    .iter()
                    .flat_map(|block| block.body().commands_iter().map(|v| v.data.clone()))
                    .collect::<Vec<_>>();

                dispatcher.push(RpcEffectfulAction::TransitionFrontierUserCommandsGet {
                    rpc_id: *rpc_id,
                    commands,
                });
            }
            RpcAction::BestChain { rpc_id, max_length } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                let best_chain = state
                    .transition_frontier
                    .best_chain
                    .iter()
                    .rev()
                    .take(*max_length as usize)
                    .cloned()
                    .rev()
                    .collect();

                dispatcher.push(RpcEffectfulAction::BestChain {
                    rpc_id: *rpc_id,
                    best_chain,
                });
            }
            RpcAction::ConsensusConstantsGet { rpc_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let response = state.config.consensus_constants.clone();
                dispatcher.push(RpcEffectfulAction::ConsensusConstantsGet {
                    rpc_id: *rpc_id,
                    response,
                });
            }
            RpcAction::TransactionNonceGet { rpc_id, account_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let response = super::RpcTransactionNonceGetResponse {
                    bootstrapping: state.transition_frontier.is_bootstrapping(meta.time()),
                    pool_nonce: state
                        .transaction_pool
                        .inferred_nonce(account_id)
                        .map(|nonce| nonce.as_u32()),
                };
                dispatcher.push(RpcEffectfulAction::TransactionNonceGet {
                    rpc_id: *rpc_id,
                    response,
                });
            }
            RpcAction::TransactionStatusGet { rpc_id, tx } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::TransactionStatusGet {
                    rpc_id: *rpc_id,
                    tx: tx.clone(),
                });
            }
            RpcAction::BlockGet { rpc_id, query } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                let find_fn = |block: &&AppliedBlock| match query {
                    GetBlockQuery::Hash(hash) => block.hash() == hash,
                    GetBlockQuery::Height(height) => block.height() == *height,
                };

                let block = state
                    .transition_frontier
                    .best_chain
                    .iter()
                    .find(find_fn)
                    .cloned();

                dispatcher.push(RpcEffectfulAction::BlockGet {
                    rpc_id: *rpc_id,
                    block,
                });
            }
            RpcAction::P2pConnectionIncomingAnswerReady {
                rpc_id,
                answer,
                peer_id,
            } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcAction::P2pConnectionIncomingRespond {
                    rpc_id: *rpc_id,
                    response: answer.clone(),
                });
                dispatcher
                    .push(P2pConnectionIncomingAction::AnswerSendSuccess { peer_id: *peer_id });
            }
            RpcAction::PooledUserCommands { rpc_id, query } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                let PooledCommandsQuery {
                    public_key,
                    hashes,
                    ids,
                } = query;

                let all_transactions = state.transaction_pool.get_all_transactions();

                let mut user_commands: Vec<_> = all_transactions
                    .into_iter()
                    .filter_map(|tx| match tx.data {
                        valid::UserCommand::SignedCommand(signed_command) => Some((
                            tx.hash,
                            MinaBaseSignedCommandStableV2::from(*signed_command),
                        )),
                        valid::UserCommand::ZkAppCommand(_) => None,
                    })
                    .collect();

                if let Some(pk) = public_key {
                    let pk = NonZeroCurvePoint::from(pk.clone());
                    user_commands.retain(|(_, tx)| tx.signer == pk)
                }

                if let Some(hashes) = hashes {
                    user_commands.retain(|(hash, _)| hashes.contains(hash))
                }

                if let Some(ids) = ids {
                    user_commands.retain(|(_, tx)| ids.contains(tx))
                }

                dispatcher.push(RpcEffectfulAction::PooledUserCommands {
                    rpc_id: *rpc_id,
                    user_commands: user_commands.into_iter().map(|(_, tx)| tx).collect(),
                });
            }
            RpcAction::LightBestTipGet { rpc_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let frontier = &state.transition_frontier;
                // Arc clones only: the HTTP side hashes and encodes.
                let response =
                    if frontier.best_tip().is_none() || frontier.is_bootstrapping(meta.time()) {
                        Err(RpcLightError::NotSynced)
                    } else {
                        Ok(frontier
                            .best_chain
                            .iter()
                            .map(|block| block.block.clone())
                            .collect())
                    };
                dispatcher.push(RpcEffectfulAction::LightBestTip {
                    rpc_id: *rpc_id,
                    response,
                });
            }
            RpcAction::LightAccountInit { rpc_id, query } => {
                // A retry after the ledger read quota was full finds the
                // request in Init and already admitted.
                let retry = state.requests.contains_key(rpc_id);
                let refusal = if retry {
                    None
                } else if query.account_ids.is_empty()
                    || query.account_ids.len() > MAX_LIGHT_ACCOUNTS_PER_REQUEST
                {
                    Some(RpcLightError::TooManyAccounts(
                        MAX_LIGHT_ACCOUNTS_PER_REQUEST,
                    ))
                } else if state.light_pending.len() >= MAX_PENDING_LIGHT_REQUESTS
                    || state.light_pending_accounts() + query.account_ids.len()
                        > MAX_PENDING_LIGHT_ACCOUNTS
                {
                    Some(RpcLightError::Busy)
                } else {
                    state.light_pending.insert(*rpc_id, meta.time());
                    None
                };
                state.requests.insert(
                    *rpc_id,
                    RpcRequestState {
                        req: RpcRequest::LightAccount(query.clone()),
                        status: RpcRequestStatus::Init { time: meta.time() },
                        data: Default::default(),
                    },
                );
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let read = match refusal {
                    Some(error) => Err(error),
                    None => light_account_read(global_state, query, meta.time()),
                };
                match read {
                    Ok(read) => dispatcher.push(LedgerReadAction::Init {
                        request: LedgerReadRequest::LightAccount(*rpc_id, Box::new(read)),
                        callback: LedgerReadInitCallback::RpcLightAccountPending {
                            callback: redux::callback!(
                                on_ledger_read_init_rpc_light_account(rpc_id: RequestId<RpcIdType>) -> crate::Action {
                                    RpcAction::LightAccountPending { rpc_id }
                                }
                            ),
                            args: *rpc_id,
                        },
                    }),
                    Err(error) => dispatcher.push(RpcAction::LightAccountSuccess {
                        rpc_id: *rpc_id,
                        response: Err(error),
                    }),
                }
            }
            RpcAction::LightAccountPending { rpc_id } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Pending { time: meta.time() };
            }
            RpcAction::LightAccountSuccess { rpc_id, response } => {
                state.light_pending.remove(rpc_id);
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Success { time: meta.time() };
                state_context
                    .into_dispatcher()
                    .push(RpcEffectfulAction::LightAccountSuccess {
                        rpc_id: *rpc_id,
                        response: response.clone(),
                    });
            }
            RpcAction::LightAccountsExpire => {
                let expired: Vec<_> = state.light_expired(meta.time()).collect();
                let dispatcher = state_context.into_dispatcher();
                for rpc_id in expired {
                    dispatcher.push(RpcAction::LightAccountSuccess {
                        rpc_id,
                        response: Err(RpcLightError::Timeout),
                    });
                }
            }
            RpcAction::GenesisBlock { rpc_id } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let genesis_block = state.genesis_block();
                dispatcher.push(RpcEffectfulAction::GenesisBlock {
                    rpc_id: *rpc_id,
                    genesis_block,
                });
            }
            RpcAction::PooledZkappCommands { rpc_id, query } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                let PooledCommandsQuery {
                    public_key,
                    hashes,
                    ids,
                } = query;

                let all_transactions = state.transaction_pool.get_all_transactions();

                let mut zkapp_commands: Vec<_> = all_transactions
                    .into_iter()
                    .filter_map(|tx| match tx.data {
                        valid::UserCommand::SignedCommand(_) => None,
                        valid::UserCommand::ZkAppCommand(zkapp) => Some((
                            tx.hash,
                            MinaBaseZkappCommandTStableV1WireStableV1::from(&zkapp.zkapp_command),
                        )),
                    })
                    .collect();

                if let Some(pk) = public_key {
                    let pk = NonZeroCurvePoint::from(pk.clone());
                    zkapp_commands.retain(|(_, tx)| tx.fee_payer.body.public_key == pk);
                }

                if let Some(hashes) = hashes {
                    zkapp_commands.retain(|(hash, _)| hashes.contains(hash));
                }

                if let Some(ids) = ids {
                    zkapp_commands.retain(|(_, tx)| ids.contains(tx));
                }

                dispatcher.push(RpcEffectfulAction::PooledZkappCommands {
                    rpc_id: *rpc_id,
                    zkapp_commands: zkapp_commands.into_iter().map(|(_, tx)| tx).collect(),
                });
            }
            RpcAction::ConsensusTimeGet { rpc_id, query } => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let consensus_time = match query {
                    ConsensusTimeQuery::Now => state.consensus_time_now(),
                    ConsensusTimeQuery::BestTip => state.consensus_time_best_tip(),
                };
                mina_core::debug!("consensus_time: {:?}", consensus_time);
                dispatcher.push(RpcEffectfulAction::ConsensusTimeGet {
                    rpc_id: *rpc_id,
                    consensus_time,
                });
            }
            RpcAction::LedgerStatusGetInit {
                rpc_id,
                ledger_hash,
            } => {
                let rpc_state = RpcRequestState {
                    req: RpcRequest::LedgerStatusGet(ledger_hash.clone()),
                    status: RpcRequestStatus::Init { time: meta.time() },
                    data: Default::default(),
                };
                state.requests.insert(*rpc_id, rpc_state);

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let ledger_hash = if let Some(best_tip) = state.transition_frontier.best_tip() {
                    best_tip.merkle_root_hash()
                } else {
                    return;
                };

                dispatcher.push(LedgerReadAction::Init {
                    request: LedgerReadRequest::GetLedgerStatus(*rpc_id, ledger_hash.clone()),
                    callback: LedgerReadInitCallback::RpcLedgerStatusGetPending {
                        callback: redux::callback!(
                            on_ledger_read_init_rpc_actions_get_init(rpc_id: RequestId<RpcIdType>) -> crate::Action{
                                RpcAction::LedgerStatusGetPending { rpc_id }
                            }
                        ),
                        args: *rpc_id,
                    },
                })
            }
            RpcAction::LedgerStatusGetPending { rpc_id } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Pending { time: meta.time() };
            }
            RpcAction::LedgerStatusGetSuccess { rpc_id, response } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Success { time: meta.time() };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::LedgerStatusGetSuccess {
                    rpc_id: *rpc_id,
                    response: response.clone(),
                });
                // As above: answered, so the entry leaves with the response.
                dispatcher.push(RpcAction::Finish { rpc_id: *rpc_id });
            }
            RpcAction::LedgerAccountDelegatorsGetInit {
                rpc_id,
                query,
                account_id,
            } => {
                let rpc_state = RpcRequestState {
                    req: RpcRequest::LedgerAccountDelegatorsGet(query.clone(), account_id.clone()),
                    status: RpcRequestStatus::Init { time: meta.time() },
                    data: Default::default(),
                };
                state.requests.insert(*rpc_id, rpc_state);

                let dispatcher = state_context.into_dispatcher();

                dispatcher.push(LedgerReadAction::Init {
                    request: LedgerReadRequest::GetAccountDelegators(
                        *rpc_id,
                        query.clone(),
                        account_id.clone(),
                    ),
                    callback: LedgerReadInitCallback::RpcLedgerAccountDelegatorsGetPending {
                        callback: redux::callback!(
                            on_ledger_read_init_rpc_actions_get_init(rpc_id: RequestId<RpcIdType>) -> crate::Action{
                                RpcAction::LedgerAccountDelegatorsGetPending { rpc_id }
                            }
                        ),
                        args: *rpc_id,
                    },
                })
            }
            RpcAction::LedgerAccountDelegatorsGetPending { rpc_id } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Pending { time: meta.time() };
            }
            RpcAction::LedgerAccountDelegatorsGetSuccess { rpc_id, response } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Success { time: meta.time() };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::LedgerAccountDelegatorsGetSuccess {
                    rpc_id: *rpc_id,
                    response: response.clone(),
                });
                // As above: answered, so the entry leaves with the response.
                dispatcher.push(RpcAction::Finish { rpc_id: *rpc_id });
            }
            RpcAction::LedgerExportInit { rpc_id, kind } => {
                let rpc_state = RpcRequestState {
                    req: RpcRequest::LedgerExport(*kind),
                    status: RpcRequestStatus::Init { time: meta.time() },
                    data: Default::default(),
                };
                state.requests.insert(*rpc_id, rpc_state);

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let Some(request) = ledger_export_request(state, *kind) else {
                    bug_condition!("LedgerExportInit without a best tip");
                    return;
                };
                dispatcher.push(LedgerReadAction::Init {
                    request: LedgerReadRequest::LedgerExport(*rpc_id, Box::new(request)),
                    callback: LedgerReadInitCallback::RpcLedgerExportPending {
                        callback: redux::callback!(
                            on_ledger_read_init_rpc_ledger_export(rpc_id: RequestId<RpcIdType>) -> crate::Action {
                                RpcAction::LedgerExportPending { rpc_id }
                            }
                        ),
                        args: *rpc_id,
                    },
                })
            }
            RpcAction::LedgerExportPending { rpc_id } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Pending { time: meta.time() };
            }
            RpcAction::LedgerExportSuccess { rpc_id, response } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Success { time: meta.time() };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::LedgerExportSuccess {
                    rpc_id: *rpc_id,
                    response: response.clone(),
                });
                // As above: answered, so the entry leaves with the response.
                dispatcher.push(RpcAction::Finish { rpc_id: *rpc_id });
            }
            RpcAction::HardForkConfigInit { rpc_id, spec } => {
                let rpc_state = RpcRequestState {
                    req: RpcRequest::HardForkConfig(spec.clone()),
                    status: RpcRequestStatus::Init { time: meta.time() },
                    data: Default::default(),
                };
                state.requests.insert(*rpc_id, rpc_state);

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                match hard_fork_export_request(state, spec) {
                    Ok(request) => dispatcher.push(LedgerReadAction::Init {
                        request: LedgerReadRequest::HardForkExport(*rpc_id, Box::new(request)),
                        callback: LedgerReadInitCallback::RpcHardForkConfigPending {
                            callback: redux::callback!(
                                on_ledger_read_init_rpc_hard_fork_config(rpc_id: RequestId<RpcIdType>) -> crate::Action {
                                    RpcAction::HardForkConfigPending { rpc_id }
                                }
                            ),
                            args: *rpc_id,
                        },
                    }),
                    // Answered now: there is no fork block to export.
                    Err(error) => {
                        dispatcher.push(RpcAction::HardForkConfigPending { rpc_id: *rpc_id });
                        dispatcher.push(RpcAction::HardForkConfigSuccess {
                            rpc_id: *rpc_id,
                            response: Err(error),
                        });
                    }
                }
            }
            RpcAction::HardForkConfigPending { rpc_id } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Pending { time: meta.time() };
            }
            RpcAction::HardForkConfigSuccess { rpc_id, response } => {
                let Some(rpc) = state.requests.get_mut(rpc_id) else {
                    return;
                };
                rpc.status = RpcRequestStatus::Success { time: meta.time() };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(RpcEffectfulAction::HardForkConfigSuccess {
                    rpc_id: *rpc_id,
                    response: response.clone(),
                });
                // As above: answered, so the entry leaves with the response.
                dispatcher.push(RpcAction::Finish { rpc_id: *rpc_id });
            }
        }
    }
}

/// The best chain, root first, as snarked and staged ledger hashes, and the
/// protocol states its scan states refer to: what the ledger service needs
/// to rebuild a snarked ledger of the best chain it doesn't hold.
#[allow(clippy::type_complexity)]
fn best_chain_ledgers(
    state: &crate::State,
) -> (
    Vec<(
        mina_p2p_messages::v2::LedgerHash,
        mina_p2p_messages::v2::MinaBaseStagedLedgerHashStableV1,
    )>,
    std::collections::BTreeMap<
        mina_p2p_messages::v2::StateHash,
        mina_p2p_messages::v2::MinaStateProtocolStateValueStableV2,
    >,
) {
    let frontier = &state.transition_frontier;
    let chain = frontier
        .best_chain
        .iter()
        .map(|b| {
            (
                b.snarked_ledger_hash().clone(),
                b.staged_ledger_hashes().clone(),
            )
        })
        .collect();
    let protocol_states = frontier
        .needed_protocol_states
        .iter()
        .map(|(hash, s)| (hash.clone(), s.clone()))
        .chain(
            frontier
                .best_chain
                .iter()
                .map(|b| (b.hash().clone(), b.header().protocol_state.clone())),
        )
        .collect();
    (chain, protocol_states)
}

/// The fork inputs at `spec` and what the ledger service needs to export
/// the fork block's staged ledger and its two epoch ledgers.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L2946-L2987>
fn hard_fork_export_request(
    state: &crate::State,
    spec: &HardForkBlockSpec,
) -> Result<HardForkExportRequest, String> {
    let inputs = super::hard_fork_inputs(
        &state.transition_frontier.best_chain,
        mina_core::constants::stop_slots(),
        mina_core::constants::constraint_constants().block_window_duration_ms,
        spec,
    )?;
    let (chain, protocol_states) = best_chain_ledgers(state);
    Ok(HardForkExportRequest {
        inputs,
        chain_id: state.p2p.ready().map(|p2p| p2p.chain_id.to_hex()),
        chain,
        protocol_states,
    })
}

/// The ledger `kind` names at the best tip, with what the ledger service
/// needs to rebuild it if it is a snarked ledger it doesn't hold.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L680-L760>
fn ledger_export_request(
    state: &crate::State,
    kind: LedgerExportKind,
) -> Option<LedgerExportRequest> {
    let frontier = &state.transition_frontier;
    let best_tip = frontier.best_tip()?;
    let ledger_hash = match kind {
        LedgerExportKind::StagedLedger => best_tip.merkle_root_hash(),
        LedgerExportKind::SnarkedLedger => best_tip.snarked_ledger_hash(),
        LedgerExportKind::StakingEpochLedger => best_tip.staking_epoch_ledger_hash(),
        LedgerExportKind::NextEpochLedger => best_tip.next_epoch_ledger_hash(),
    }
    .clone();
    let (chain, protocol_states) = if kind == LedgerExportKind::StagedLedger {
        Default::default()
    } else {
        let chain = frontier
            .best_chain
            .iter()
            .map(|b| {
                (
                    b.snarked_ledger_hash().clone(),
                    b.staged_ledger_hashes().clone(),
                )
            })
            .collect();
        let protocol_states = frontier
            .needed_protocol_states
            .iter()
            .map(|(hash, s)| (hash.clone(), s.clone()))
            .chain(
                frontier
                    .best_chain
                    .iter()
                    .map(|b| (b.hash().clone(), b.header().protocol_state.clone())),
            )
            .collect();
        (chain, protocol_states)
    };
    Some(LedgerExportRequest {
        kind,
        ledger_hash,
        best_tip: LedgerExportBestTip {
            state_hash: best_tip.hash().clone(),
            blockchain_length: best_tip.height(),
            global_slot_since_genesis: best_tip.global_slot_since_genesis(),
            epoch: best_tip.consensus_state().epoch_count.as_u32(),
        },
        chain,
        protocol_states,
    })
}

/// The ledger a light account query names, at a best-chain block. Refused
/// while the node bootstraps: its best chain is then stale or absent.
/// Catch-up of new blocks does not count, so answers continue across blocks
/// and root moves.
fn light_account_read(
    state: &crate::State,
    query: &RpcLightAccountQuery,
    now: redux::Timestamp,
) -> Result<LightAccountRead, RpcLightError> {
    let frontier = &state.transition_frontier;
    let best_tip = frontier.best_tip().ok_or(RpcLightError::NotSynced)?;
    if frontier.is_bootstrapping(now) {
        return Err(RpcLightError::NotSynced);
    }
    let block = match &query.block {
        None => best_tip,
        Some(hash) => frontier
            .best_chain
            .iter()
            .rev()
            .map(|block| &block.block)
            .find(|block| block.hash() == hash)
            .ok_or_else(|| RpcLightError::BlockNotInBestChain(hash.clone()))?,
    };
    Ok(LightAccountRead {
        ledger: query.ledger,
        ledger_hash: query.ledger.ledger_hash(block).clone(),
        block: LightBlockRef::of(block),
        account_ids: query.account_ids.clone(),
    })
}

pub fn collect_rpc_peers_info(state: &crate::State) -> Vec<RpcPeerInfo> {
    state.p2p.ready().map_or_else(Vec::new, |p2p| {
        p2p.peers
            .iter()
            .map(|(peer_id, state)| {
                let best_tip = state.status.as_ready().and_then(|r| r.best_tip.as_ref());
                let (connection_status, time, incoming, connecting_details) = match &state.status {
                    crate::p2p::P2pPeerStatus::Connecting(c) => match c {
                        crate::p2p::connection::P2pConnectionState::Outgoing(o) => (
                            connection_status_for_attempt(c),
                            o.time().into(),
                            false,
                            Some(format!("{o:?}")),
                        ),
                        crate::p2p::connection::P2pConnectionState::Incoming(i) => (
                            connection_status_for_attempt(c),
                            i.time().into(),
                            true,
                            Some(format!("{i:?}")),
                        ),
                    },
                    crate::p2p::P2pPeerStatus::Disconnecting { time } => (
                        PeerConnectionStatus::Disconnecting,
                        (*time).into(),
                        false,
                        None,
                    ),
                    crate::p2p::P2pPeerStatus::Disconnected { time } => (
                        PeerConnectionStatus::Disconnected,
                        (*time).into(),
                        false,
                        None,
                    ),
                    crate::p2p::P2pPeerStatus::Ready(r) => (
                        PeerConnectionStatus::Connected,
                        r.connected_since.into(),
                        r.is_incoming,
                        None,
                    ),
                };
                RpcPeerInfo {
                    peer_id: *peer_id,
                    connection_status,
                    connecting_details,
                    address: state.dial_opts.as_ref().map(|opts| opts.to_string()),
                    is_libp2p: state.is_libp2p,
                    incoming,
                    best_tip: best_tip.map(|bt| bt.hash.clone()),
                    best_tip_height: best_tip.map(|bt| bt.height()),
                    best_tip_global_slot: best_tip.map(|bt| bt.global_slot_since_genesis()),
                    best_tip_timestamp: best_tip.map(|bt| bt.timestamp().into()),
                    time,
                }
            })
            .collect()
    })
}

fn connection_status_for_attempt(
    connection: &crate::p2p::connection::P2pConnectionState,
) -> PeerConnectionStatus {
    if connection.is_error() {
        PeerConnectionStatus::Disconnected
    } else {
        PeerConnectionStatus::Connecting
    }
}

#[cfg(test)]
mod d3f_tests {
    use super::*;
    use crate::p2p::connection::{
        outgoing::{P2pConnectionOutgoingError, P2pConnectionOutgoingState},
        P2pConnectionState,
    };

    #[test]
    fn terminal_dial_error_is_disconnected_with_reason() {
        let attempt = P2pConnectionState::Outgoing(P2pConnectionOutgoingState::Error {
            time: redux::Timestamp::ZERO,
            error: P2pConnectionOutgoingError::Timeout,
            rpc_id: None,
        });
        assert!(matches!(
            connection_status_for_attempt(&attempt),
            PeerConnectionStatus::Disconnected
        ));
        assert!(format!("{:?}", attempt.as_outgoing().unwrap()).contains("Timeout"));
    }
}
