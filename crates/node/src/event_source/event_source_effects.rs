use crate::p2p::channels::{
    signaling::{
        discovery::P2pChannelsSignalingDiscoveryAction,
        exchange::P2pChannelsSignalingExchangeAction,
    },
    snark::P2pChannelsSnarkAction,
    streaming_rpc::P2pChannelsStreamingRpcAction,
    transaction::P2pChannelsTransactionAction,
};
use mina_snark::user_command_verify::{SnarkUserCommandVerifyAction, SnarkUserCommandVerifyError};
use std::time::Duration;

#[cfg(feature = "p2p-libp2p")]
use crate::p2p::{MioEvent, P2pNetworkSchedulerAction};
use crate::{
    action::CheckTimeoutsAction,
    block_producer::{
        vrf_evaluator::BlockProducerVrfEvaluatorAction, BlockProducerEvent,
        BlockProducerVrfEvaluatorEvent,
    },
    external_snark_worker_effectful::ExternalSnarkWorkerEvent,
    ledger::{read::LedgerReadAction, write::LedgerWriteAction},
    p2p::{
        channels::{
            best_tip::P2pChannelsBestTipAction, rpc::P2pChannelsRpcAction,
            snark_job_commitment::P2pChannelsSnarkJobCommitmentAction, ChannelId,
            P2pChannelsMessageReceivedAction,
        },
        connection::{
            incoming::P2pConnectionIncomingAction, outgoing::P2pConnectionOutgoingAction,
            P2pConnectionErrorResponse, P2pConnectionResponse,
        },
        disconnection::{P2pDisconnectionAction, P2pDisconnectionReason},
        P2pChannelEvent,
    },
    rpc::{RpcAction, RpcRequest},
    rpc_effectful::RpcService,
    snark::{block_verify::SnarkBlockVerifyAction, work_verify::SnarkWorkVerifyAction, SnarkEvent},
    transition_frontier::{
        genesis::TransitionFrontierGenesisAction, persist::STALE_SLOTS_PER_K,
        persist_effectful::TransitionFrontierPersistService,
    },
    BlockProducerAction, ExternalSnarkWorkerAction, Service, Store,
};

use super::{
    Event, EventSourceAction, EventSourceActionWithMeta, LedgerEvent, P2pConnectionEvent, P2pEvent,
};

/// At most this many events are taken in one `ProcessEvents` batch.
pub const PROCESS_EVENTS_MAX: usize = 1024;

/// A `ProcessEvents` batch takes no more events once it has run this long, so
/// that `CheckTimeoutsAction` runs at least about this often under load, as
/// when RPC requests that are expensive for the node thread are taken between
/// events. An event that alone takes this long is logged.
pub const PROCESS_EVENTS_TIME_BUDGET: Duration = Duration::from_millis(100);

pub fn event_source_effects<S: Service>(store: &mut Store<S>, action: EventSourceActionWithMeta) {
    let (action, meta) = action.split();
    match action {
        EventSourceAction::ProcessEvents => {
            // This action gets continously called until there are no more
            // events available.
            //
            // Retrieve and process at most `PROCESS_EVENTS_MAX` events at a
            // time, for at most about `PROCESS_EVENTS_TIME_BUDGET`, and
            // dispatch `CheckTimeoutsAction` in between
            // `EventSourceProcessEventsAction` calls so that we make sure
            // that action gets called even if we are continously flooded with
            // events.
            let started = store.service.monotonic_time();
            let mut last = started;
            for _ in 0..PROCESS_EVENTS_MAX {
                let Some(event) = store.service.next_event() else {
                    break;
                };
                let kind = event.kind();
                store.dispatch(EventSourceAction::NewEvent { event });
                let now = store.service.monotonic_time();
                let took = now.saturating_duration_since(last);
                if took >= PROCESS_EVENTS_TIME_BUDGET {
                    // With RPC requests taken between events, the longest
                    // event bounds how long a request waits.
                    mina_core::log::debug!(
                        meta.time();
                        kind = "EventSourceSlowEvent",
                        summary = format!("{kind} event took {} ms", took.as_millis()),
                        event = kind,
                        took_ms = took.as_millis() as u64,
                    );
                }
                last = now;
                if now.saturating_duration_since(started) >= PROCESS_EVENTS_TIME_BUDGET {
                    break;
                }
            }
            store.dispatch(CheckTimeoutsAction {});
        }
        // "Translate" event into the corresponding action and dispatch it.
        EventSourceAction::NewEvent { event } => match event {
            Event::P2p(e) => match e {
                #[cfg(not(feature = "p2p-libp2p"))]
                P2pEvent::MioEvent(_) => {}
                #[cfg(feature = "p2p-libp2p")]
                P2pEvent::MioEvent(e) => match e {
                    MioEvent::InterfaceDetected(ip) => {
                        store.dispatch(P2pNetworkSchedulerAction::InterfaceDetected { ip });
                    }
                    MioEvent::InterfaceExpired(ip) => {
                        store.dispatch(P2pNetworkSchedulerAction::InterfaceExpired { ip });
                    }
                    MioEvent::ListenerReady { listener } => {
                        store.dispatch(P2pNetworkSchedulerAction::ListenerReady { listener });
                    }
                    MioEvent::ListenerError { listener, error } => {
                        store
                            .dispatch(P2pNetworkSchedulerAction::ListenerError { listener, error });
                    }
                    MioEvent::IncomingConnectionIsReady { listener } => {
                        store.dispatch(P2pNetworkSchedulerAction::IncomingConnectionIsReady {
                            listener,
                        });
                    }
                    MioEvent::IncomingConnectionDidAccept(addr, result) => {
                        store.dispatch(P2pNetworkSchedulerAction::IncomingDidAccept {
                            addr,
                            result,
                        });
                    }
                    MioEvent::OutgoingConnectionDidConnect(addr, result) => {
                        store.dispatch(P2pNetworkSchedulerAction::OutgoingDidConnect {
                            addr,
                            result,
                        });
                    }
                    MioEvent::IncomingDataIsReady(addr) => {
                        store.dispatch(P2pNetworkSchedulerAction::IncomingDataIsReady { addr });
                    }
                    MioEvent::IncomingDataDidReceive(addr, result) => {
                        store.dispatch(P2pNetworkSchedulerAction::IncomingDataDidReceive {
                            addr,
                            result,
                        });
                    }
                    MioEvent::OutgoingDataDidSend(_, _result) => {}
                    MioEvent::ConnectionDidClose(addr, result) => {
                        if let Err(e) = result {
                            store.dispatch(P2pNetworkSchedulerAction::Error {
                                addr,
                                error: crate::p2p::P2pNetworkConnectionError::MioError(e),
                            });
                        } else {
                            store.dispatch(P2pNetworkSchedulerAction::Error {
                                addr,
                                error: crate::p2p::P2pNetworkConnectionError::RemoteClosed,
                            });
                        }
                    }
                    MioEvent::ConnectionDidCloseOnDemand(addr) => {
                        store.dispatch(P2pNetworkSchedulerAction::Prune { addr });
                    }
                },
                P2pEvent::Connection(e) => match e {
                    P2pConnectionEvent::OfferSdpReady(peer_id, res) => match res {
                        Err(error) => {
                            store.dispatch(P2pConnectionOutgoingAction::OfferSdpCreateError {
                                peer_id,
                                error,
                            });
                        }
                        Ok(sdp) => {
                            store.dispatch(P2pConnectionOutgoingAction::OfferSdpCreateSuccess {
                                peer_id,
                                sdp,
                            });
                        }
                    },
                    P2pConnectionEvent::AnswerSdpReady(peer_id, res) => match res {
                        Err(error) => {
                            store.dispatch(P2pConnectionIncomingAction::AnswerSdpCreateError {
                                peer_id,
                                error,
                            });
                        }
                        Ok(sdp) => {
                            store.dispatch(P2pConnectionIncomingAction::AnswerSdpCreateSuccess {
                                peer_id,
                                sdp,
                            });
                        }
                    },
                    P2pConnectionEvent::AnswerReceived(peer_id, res) => match res {
                        P2pConnectionResponse::Accepted(answer) => {
                            store.dispatch(P2pConnectionOutgoingAction::AnswerRecvSuccess {
                                peer_id,
                                answer,
                            });
                        }
                        P2pConnectionResponse::Rejected(reason) => {
                            store.dispatch(P2pConnectionOutgoingAction::AnswerRecvError {
                                peer_id,
                                error: P2pConnectionErrorResponse::Rejected(reason),
                            });
                        }
                        P2pConnectionResponse::SignalDecryptionFailed => {
                            store.dispatch(P2pConnectionOutgoingAction::AnswerRecvError {
                                peer_id,
                                error: P2pConnectionErrorResponse::SignalDecryptionFailed,
                            });
                        }
                        P2pConnectionResponse::InternalError => {
                            store.dispatch(P2pConnectionOutgoingAction::AnswerRecvError {
                                peer_id,
                                error: P2pConnectionErrorResponse::InternalError,
                            });
                        }
                    },
                    P2pConnectionEvent::Finalized(peer_id, res) => match res {
                        Err(error) => {
                            store.dispatch(P2pConnectionOutgoingAction::FinalizeError {
                                peer_id,
                                error: error.clone(),
                            });
                            store.dispatch(P2pConnectionIncomingAction::FinalizeError {
                                peer_id,
                                error,
                            });
                        }
                        Ok(auth) => {
                            let _ = store.dispatch(P2pConnectionOutgoingAction::FinalizeSuccess {
                                peer_id,
                                remote_auth: Some(auth.clone()),
                            }) || store.dispatch(
                                P2pConnectionIncomingAction::FinalizeSuccess {
                                    peer_id,
                                    remote_auth: auth.clone(),
                                },
                            );
                        }
                    },
                    P2pConnectionEvent::Closed(peer_id) => {
                        store.dispatch(P2pDisconnectionAction::PeerClosed { peer_id });
                        store.dispatch(P2pDisconnectionAction::Finish { peer_id });
                    }
                },
                P2pEvent::Channel(e) => match e {
                    P2pChannelEvent::Opened(peer_id, chan_id, res) => match res {
                        Err(err) => {
                            // A peer without one of our channels, routine (O1).
                            mina_core::log::debug!(meta.time(); kind = "P2pChannelEvent::Opened", peer_id = peer_id.to_string(), error = err);
                            // TODO(binier): dispatch error action.
                        }
                        Ok(_) => match chan_id {
                            ChannelId::SignalingDiscovery => {
                                store.dispatch(P2pChannelsSignalingDiscoveryAction::Ready {
                                    peer_id,
                                });
                            }
                            ChannelId::SignalingExchange => {
                                store.dispatch(P2pChannelsSignalingExchangeAction::Ready {
                                    peer_id,
                                });
                            }
                            ChannelId::BestTipPropagation => {
                                store.dispatch(P2pChannelsBestTipAction::Ready { peer_id });
                            }
                            ChannelId::TransactionPropagation => {
                                store.dispatch(P2pChannelsTransactionAction::Ready { peer_id });
                            }
                            ChannelId::SnarkPropagation => {
                                store.dispatch(P2pChannelsSnarkAction::Ready { peer_id });
                            }
                            ChannelId::SnarkJobCommitmentPropagation => {
                                store.dispatch(P2pChannelsSnarkJobCommitmentAction::Ready {
                                    peer_id,
                                });
                            }
                            ChannelId::Rpc => {
                                store.dispatch(P2pChannelsRpcAction::Ready { peer_id });
                            }
                            ChannelId::StreamingRpc => {
                                store.dispatch(P2pChannelsStreamingRpcAction::Ready { peer_id });
                            }
                        },
                    },
                    P2pChannelEvent::Sent(peer_id, _, _, res) => {
                        if let Err(err) = res {
                            let reason = P2pDisconnectionReason::P2pChannelSendFailed(err);
                            store.dispatch(P2pDisconnectionAction::Init { peer_id, reason });
                        }
                    }
                    P2pChannelEvent::Received(peer_id, res) => match res {
                        Err(err) => {
                            let reason = P2pDisconnectionReason::P2pChannelReceiveFailed(err);
                            store.dispatch(P2pDisconnectionAction::Init { peer_id, reason });
                        }
                        Ok(message) => {
                            store.dispatch(P2pChannelsMessageReceivedAction { peer_id, message });
                        }
                    },
                    P2pChannelEvent::Closed(peer_id, chan_id) => {
                        let reason = P2pDisconnectionReason::P2pChannelClosed(chan_id);
                        store.dispatch(P2pDisconnectionAction::Init { peer_id, reason });
                    }
                },
            },
            Event::Ledger(event) => match event {
                LedgerEvent::Verification(update) => {
                    store.dispatch(update.action());
                }
                LedgerEvent::Write(response) => {
                    store.dispatch(LedgerWriteAction::Success { response });
                }
                LedgerEvent::Read(id, response) => {
                    store.dispatch(LedgerReadAction::Success { id, response });
                }
            },
            Event::Snark(event) => match event {
                SnarkEvent::BlockVerifyMeasured {
                    req_id,
                    result,
                    elapsed_ms,
                    skipped,
                    verifier_key,
                } => {
                    if let Some(job) = store
                        .state()
                        .snark
                        .block_verify
                        .jobs
                        .get(req_id)
                        .filter(|job| job.is_pending())
                    {
                        use crate::verification::{BlockIdentity, CheckStatus, VerificationUpdate};
                        let block = job.block();
                        let update = VerificationUpdate::Proof {
                            block: BlockIdentity::header(
                                block.hash_ref().clone(),
                                block.header_ref(),
                            ),
                            status: if skipped {
                                CheckStatus::Skipped
                            } else if result.is_ok() {
                                CheckStatus::Passed
                            } else {
                                CheckStatus::Failed
                            },
                            elapsed_ms: (!skipped).then_some(elapsed_ms),
                            verifier_key: (!skipped).then_some(verifier_key),
                        };
                        store.dispatch(update.action());
                    }
                    match result {
                        Err(error) => {
                            store.dispatch(SnarkBlockVerifyAction::Error { req_id, error });
                        }
                        Ok(()) => {
                            store.dispatch(SnarkBlockVerifyAction::Success { req_id });
                        }
                    }
                }
                SnarkEvent::BlockVerify(req_id, result) => match result {
                    Err(error) => {
                        store.dispatch(SnarkBlockVerifyAction::Error { req_id, error });
                    }
                    Ok(()) => {
                        store.dispatch(SnarkBlockVerifyAction::Success { req_id });
                    }
                },
                SnarkEvent::WorkVerify(req_id, result) => match result {
                    Err(error) => {
                        store.dispatch(SnarkWorkVerifyAction::Error { req_id, error });
                    }
                    Ok(()) => {
                        store.dispatch(SnarkWorkVerifyAction::Success { req_id });
                    }
                },
                SnarkEvent::UserCommandVerify(req_id, result) => match result {
                    Ok(commands) => {
                        store.dispatch(SnarkUserCommandVerifyAction::Success { req_id, commands });
                    }
                    Err(error) => {
                        store.dispatch(SnarkUserCommandVerifyAction::Error {
                            req_id,
                            error: SnarkUserCommandVerifyError::Invalid(error),
                        });
                    }
                },
            },
            Event::Rpc(rpc_id, e) => match *e {
                RpcRequest::StateGet(filter) => {
                    store.dispatch(RpcAction::GlobalStateGet { rpc_id, filter });
                }
                RpcRequest::StatusGet => {
                    store.dispatch(RpcAction::StatusGet { rpc_id });
                }
                RpcRequest::MemoryDebugGet => {
                    store.dispatch(RpcAction::MemoryDebugGet { rpc_id });
                }
                RpcRequest::MetricsGet => {
                    store.dispatch(RpcAction::MetricsGet { rpc_id });
                }
                RpcRequest::HeartbeatGet => {
                    store.dispatch(RpcAction::HeartbeatGet { rpc_id });
                }
                RpcRequest::ActionStatsGet(query) => {
                    store.dispatch(RpcAction::ActionStatsGet { rpc_id, query });
                }
                RpcRequest::SyncStatsGet(query) => {
                    store.dispatch(RpcAction::SyncStatsGet { rpc_id, query });
                }
                RpcRequest::VerificationStatsGet(query) => {
                    store.dispatch(RpcAction::VerificationStatsGet { rpc_id, query });
                }
                RpcRequest::SnarkWorkCoverageGet => {
                    store.dispatch(RpcAction::SnarkWorkCoverageGet { rpc_id });
                }
                RpcRequest::ProofMemoryGet => {
                    store.dispatch(RpcAction::ProofMemoryGet { rpc_id });
                }
                RpcRequest::BlockProducerStatsGet => {
                    store.dispatch(RpcAction::BlockProducerStatsGet { rpc_id });
                }
                RpcRequest::PeersGet => {
                    store.dispatch(RpcAction::PeersGet { rpc_id });
                }
                RpcRequest::MessageProgressGet => {
                    store.dispatch(RpcAction::MessageProgressGet { rpc_id });
                }
                RpcRequest::P2pConnectionOutgoing(opts) => {
                    store.dispatch(RpcAction::P2pConnectionOutgoingInit { rpc_id, opts });
                }
                RpcRequest::P2pConnectionIncoming(opts) => {
                    store.dispatch(RpcAction::P2pConnectionIncomingInit { rpc_id, opts });
                }
                RpcRequest::ScanStateSummaryGet(query) => {
                    store.dispatch(RpcAction::ScanStateSummaryGetInit { rpc_id, query });
                }
                RpcRequest::SnarkPoolGet => {
                    store.dispatch(RpcAction::SnarkPoolAvailableJobsGet { rpc_id });
                }
                RpcRequest::SnarkPoolJobGet { job_id } => {
                    store.dispatch(RpcAction::SnarkPoolJobGet { rpc_id, job_id });
                }
                RpcRequest::SnarkPoolCompletedJobsGet => {
                    store.dispatch(RpcAction::SnarkPoolCompletedJobsGet { rpc_id });
                }
                RpcRequest::SnarkPoolPendingJobsGet => {
                    store.dispatch(RpcAction::SnarkPoolPendingJobsGet { rpc_id });
                }
                RpcRequest::SnarkerConfig => {
                    store.dispatch(RpcAction::SnarkerConfigGet { rpc_id });
                }
                RpcRequest::SnarkerJobCommit { job_id } => {
                    store.dispatch(RpcAction::SnarkerJobCommit { rpc_id, job_id });
                }
                RpcRequest::SnarkerJobSpec { job_id } => {
                    store.dispatch(RpcAction::SnarkerJobSpec { rpc_id, job_id });
                }
                RpcRequest::SnarkerWorkers => {
                    store.dispatch(RpcAction::SnarkerWorkersGet { rpc_id });
                }
                RpcRequest::SnarkWorkerControlGet => {
                    store.dispatch(RpcAction::SnarkWorkerControlGet { rpc_id });
                }
                RpcRequest::SnarkWorkerControlSet(setting) => {
                    store.dispatch(RpcAction::SnarkWorkerControlSet { rpc_id, setting });
                }
                RpcRequest::SnarkWorkIncludedGet => {
                    store.dispatch(RpcAction::SnarkWorkIncludedGet { rpc_id });
                }
                RpcRequest::HealthCheck => {
                    store.dispatch(RpcAction::HealthCheck { rpc_id });
                }
                RpcRequest::ReadinessCheck => {
                    store.dispatch(RpcAction::ReadinessCheck { rpc_id });
                }
                RpcRequest::DiscoveryRoutingTable => {
                    store.dispatch(RpcAction::DiscoveryRoutingTable { rpc_id });
                }
                RpcRequest::DiscoveryBoostrapStats => {
                    store.dispatch(RpcAction::DiscoveryBoostrapStats { rpc_id });
                }
                RpcRequest::TransactionPoolGet => {
                    store.dispatch(RpcAction::TransactionPool { rpc_id });
                }
                RpcRequest::LedgerAccountsGet(account_query) => {
                    store.dispatch(RpcAction::LedgerAccountsGetInit {
                        rpc_id,
                        account_query,
                    });
                }
                RpcRequest::TransactionInject(commands) => {
                    store.dispatch(RpcAction::TransactionInjectInit { rpc_id, commands });
                }
                RpcRequest::TransitionFrontierUserCommandsGet => {
                    store.dispatch(RpcAction::TransitionFrontierUserCommandsGet { rpc_id });
                }
                RpcRequest::BestChain(max_length) => {
                    store.dispatch(RpcAction::BestChain { rpc_id, max_length });
                }
                RpcRequest::ConsensusConstantsGet => {
                    store.dispatch(RpcAction::ConsensusConstantsGet { rpc_id });
                }
                RpcRequest::TransactionStatusGet(tx) => {
                    store.dispatch(RpcAction::TransactionStatusGet { rpc_id, tx });
                }
                RpcRequest::TransactionNonceGet(account_id) => {
                    store.dispatch(RpcAction::TransactionNonceGet { rpc_id, account_id });
                }
                RpcRequest::GetBlock(query) => {
                    store.dispatch(RpcAction::BlockGet { rpc_id, query });
                }
                RpcRequest::PooledUserCommands(query) => {
                    store.dispatch(RpcAction::PooledUserCommands { rpc_id, query });
                }
                RpcRequest::PooledZkappCommands(query) => {
                    store.dispatch(RpcAction::PooledZkappCommands { rpc_id, query });
                }
                RpcRequest::ConsensusTimeGet(query) => {
                    store.dispatch(RpcAction::ConsensusTimeGet { rpc_id, query });
                }
                RpcRequest::GenesisBlockGet => {
                    store.dispatch(RpcAction::GenesisBlock { rpc_id });
                }
                RpcRequest::LedgerStatusGet(ledger_hash) => {
                    store.dispatch(RpcAction::LedgerStatusGetInit {
                        rpc_id,
                        ledger_hash,
                    });
                }
                RpcRequest::LedgerAccountDelegatorsGet(query, account_id) => {
                    store.dispatch(RpcAction::LedgerAccountDelegatorsGetInit {
                        rpc_id,
                        query,
                        account_id,
                    });
                }
                RpcRequest::LightBestTip => {
                    store.dispatch(RpcAction::LightBestTipGet { rpc_id });
                }
                RpcRequest::LightAccount(query) => {
                    store.dispatch(RpcAction::LightAccountInit { rpc_id, query });
                }
                RpcRequest::LedgerExport(kind) => {
                    if !store.dispatch(RpcAction::LedgerExportInit { rpc_id, kind }) {
                        // Answered now, not left waiting for a best tip.
                        store
                            .service
                            .respond_ledger_export(
                                rpc_id,
                                Err("the node has no best tip yet".to_owned()),
                            )
                            .ok();
                    }
                }
                RpcRequest::HardForkConfig(spec) => {
                    if !store.dispatch(RpcAction::HardForkConfigInit { rpc_id, spec }) {
                        store
                            .service
                            .respond_hard_fork_config(
                                rpc_id,
                                Err("the node has no best tip yet".to_owned()),
                            )
                            .ok();
                    }
                }
            },
            Event::ExternalSnarkWorker(e) => match e {
                ExternalSnarkWorkerEvent::Started => {
                    store.dispatch(ExternalSnarkWorkerAction::Started);
                }
                ExternalSnarkWorkerEvent::Killed => {
                    store.dispatch(ExternalSnarkWorkerAction::Killed);
                }
                ExternalSnarkWorkerEvent::WorkResult(result) => {
                    store.dispatch(ExternalSnarkWorkerAction::WorkResult { result });
                }
                ExternalSnarkWorkerEvent::WorkError(error) => {
                    store.dispatch(ExternalSnarkWorkerAction::WorkError { error });
                }
                ExternalSnarkWorkerEvent::WorkCancelled => {
                    store.dispatch(ExternalSnarkWorkerAction::WorkCancelled);
                }
                ExternalSnarkWorkerEvent::Error(error) => {
                    store.dispatch(ExternalSnarkWorkerAction::Error {
                        error,
                        permanent: false,
                    });
                }
            },
            Event::BlockProducerEvent(e) => match e {
                BlockProducerEvent::VrfEvaluator(vrf_e) => match vrf_e {
                    BlockProducerVrfEvaluatorEvent::Evaluated(vrf_output_with_hash) => {
                        store.dispatch(
                            BlockProducerVrfEvaluatorAction::ProcessSlotEvaluationSuccess {
                                vrf_output: vrf_output_with_hash.evaluation_result,
                                staking_ledger_hash: vrf_output_with_hash.staking_ledger_hash,
                            },
                        );
                    }
                },
                BlockProducerEvent::BlockProve(block_hash, res) => {
                    let action = block_prove_result_action(store.state(), block_hash, res);
                    store.dispatch(action);
                }
            },
            Event::GenesisLoad(res) => match res {
                Err(err) => todo!("error while trying to load genesis config/ledger. - {err}"),
                Ok(data) => {
                    store.dispatch(TransitionFrontierGenesisAction::LedgerLoadSuccess {
                        data: Box::new(data),
                    });
                }
            },
            Event::GenesisLedgersLoad(res) => {
                use crate::transition_frontier::TransitionFrontierAction;
                match res {
                    Ok(()) => store.dispatch(TransitionFrontierAction::GenesisLedgersLoadSuccess),
                    Err(error) => {
                        store.dispatch(TransitionFrontierAction::GenesisLedgersLoadError { error })
                    }
                };
            }
            Event::Persist(event) => {
                use crate::{
                    persist::event::PersistEvent,
                    transition_frontier::persist::TransitionFrontierPersistAction,
                };
                match event {
                    PersistEvent::Opened(opened) => {
                        store.dispatch(TransitionFrontierPersistAction::OpenSuccess {
                            opened: Box::new(opened),
                        });
                    }
                    PersistEvent::OpenFailed { error } => {
                        store.dispatch(TransitionFrontierPersistAction::OpenError { error });
                    }
                    PersistEvent::ChainPut(Ok(done)) => {
                        store.dispatch(TransitionFrontierPersistAction::ChainPutSuccess {
                            done: Box::new(done),
                        });
                    }
                    PersistEvent::ChainPut(Err(error)) => {
                        store.dispatch(TransitionFrontierPersistAction::ChainPutError { error });
                    }
                    PersistEvent::Checkpoint(Ok(done)) => {
                        store.dispatch(TransitionFrontierPersistAction::CheckpointSuccess {
                            done: Box::new(done),
                        });
                    }
                    PersistEvent::Checkpoint(Err(error)) => {
                        store.dispatch(TransitionFrontierPersistAction::CheckpointError { error });
                    }
                    PersistEvent::Summary(summary) => {
                        store.dispatch(TransitionFrontierPersistAction::StoreSummary {
                            store: Box::new(summary),
                        });
                    }
                    PersistEvent::Loaded(result) => {
                        use crate::persist::event::PersistLoadResult;
                        // Even a result queued just before cancellation acknowledges
                        // that the worker is drained. Never replay it afterwards.
                        if matches!(store.state().transition_frontier.persist.restore(),
                            Some(crate::transition_frontier::persist::PersistRestoreState::Cancelling { .. })) {
                            store.dispatch(TransitionFrontierPersistAction::LoadCancelled);
                            return;
                        }
                        match *result {
                            PersistLoadResult::Restored(restored) => {
                                let now = store.state().cur_global_slot();
                                let stale_slots = store.state().genesis_block().map(|genesis| {
                                    genesis
                                        .constants()
                                        .k
                                        .as_u32()
                                        .saturating_mul(STALE_SLOTS_PER_K)
                                });
                                let tip_slot = restored.tip().global_slot();
                                if now.zip(stale_slots).is_some_and(|(now, stale_slots)| {
                                    now > tip_slot.saturating_add(stale_slots)
                                }) {
                                    // The load was useful when requested but catch-up can no
                                    // longer reach its tip. Keep the checked snarked ledgers as
                                    // copy origins and clear the successful load's attempt.
                                    store.service.persist_restore_done();
                                    store.dispatch(TransitionFrontierPersistAction::LoadLedgersOnly {
                                        reason: format!("stored tip at slot {tip_slot} became stale while loading"),
                                        ledgers: restored.ledgers.len() as u32,
                                    });
                                } else {
                                    store.dispatch(TransitionFrontierPersistAction::LoadSuccess {
                                        restored: Box::new(restored),
                                    });
                                }
                            }
                            PersistLoadResult::LedgersOnly {
                                reason, ledgers, ..
                            } => {
                                store.dispatch(TransitionFrontierPersistAction::LoadLedgersOnly {
                                    reason,
                                    ledgers: ledgers.len() as u32,
                                });
                            }
                            PersistLoadResult::Absent { reason } => {
                                store.dispatch(TransitionFrontierPersistAction::LoadAbsent {
                                    reason,
                                });
                            }
                            PersistLoadResult::Discarded { reason } => {
                                store.dispatch(TransitionFrontierPersistAction::LoadDiscarded {
                                    reason,
                                });
                            }
                            PersistLoadResult::Error { error } => {
                                store
                                    .dispatch(TransitionFrontierPersistAction::LoadError { error });
                            }
                            // The node gave up waiting already.
                            PersistLoadResult::Cancelled => {
                                store.dispatch(TransitionFrontierPersistAction::LoadCancelled);
                            }
                        }
                    }
                }
            }
        },
        EventSourceAction::WaitTimeout => {
            store.dispatch(CheckTimeoutsAction {});
        }
        EventSourceAction::WaitForEvents => {}
    }
}

// Shared by the event loop and producer state-machine tests, including the Err path.
pub(crate) fn block_prove_result_action(
    state: &crate::State,
    block_hash: mina_p2p_messages::v2::StateHash,
    result: Result<std::sync::Arc<mina_p2p_messages::v2::MinaBaseProofStableV2>, String>,
) -> crate::Action {
    let result = match &state.transition_frontier.genesis {
        crate::transition_frontier::genesis::TransitionFrontierGenesisState::ProvePending {
            genesis_hash,
            deadline,
            ..
        } if genesis_hash == &block_hash && state.time() >= *deadline => {
            Err("genesis proof deadline passed before delivery".to_owned())
        }
        _ => result,
    };
    let proof = match result {
        // Preserve the explicit proof-level-none interop behavior. The verifier
        // service also observes this flag; normal production never substitutes a proof.
        Err(_) if state.config.skip_proof_verification => {
            ledger::dummy::dummy_blockchain_proof().clone()
        }
        Err(error) => {
            mina_core::error!(state.time(); message = "Block proof generation failed",
                block_hash = block_hash.to_string(), error = error);
            if state
                .transition_frontier
                .genesis
                .prove_pending_block_hash()
                .as_ref()
                == Some(&block_hash)
            {
                return TransitionFrontierGenesisAction::ProveError {
                    error: format!("block {block_hash}: {error}"),
                }
                .into();
            }
            return BlockProducerAction::BlockProveError { block_hash }.into();
        }
        Ok(proof) => proof,
    };
    if state
        .transition_frontier
        .genesis
        .prove_pending_block_hash()
        .is_some_and(|hash| hash == block_hash)
    {
        TransitionFrontierGenesisAction::ProveSuccess { proof }.into()
    } else {
        BlockProducerAction::BlockProofVerify { block_hash, proof }.into()
    }
}
