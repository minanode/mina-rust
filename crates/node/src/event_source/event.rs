use serde::{Deserialize, Serialize};

pub use crate::{
    block_producer::BlockProducerEvent,
    external_snark_worker_effectful::ExternalSnarkWorkerEvent,
    ledger::LedgerEvent,
    p2p::{P2pConnectionEvent, P2pEvent},
    rpc::{RpcId, RpcRequest},
    snark::SnarkEvent,
};

use crate::{
    p2p::{channels::ChannelId, P2pChannelEvent},
    transition_frontier::genesis::GenesisConfigLoaded,
};

#[derive(derive_more::From, Serialize, Deserialize, Debug, Clone)]
pub enum Event {
    P2p(P2pEvent),
    Ledger(LedgerEvent),
    Snark(SnarkEvent),
    Rpc(RpcId, Box<RpcRequest>),
    ExternalSnarkWorker(ExternalSnarkWorkerEvent),
    BlockProducerEvent(BlockProducerEvent),

    GenesisLoad(Result<GenesisConfigLoaded, String>),
    /// The genesis ledgers deferred at startup are loaded (I5d2).
    GenesisLedgersLoad(Result<(), String>),
    /// From the frontier store thread (R20 D3).
    Persist(crate::persist::event::PersistEvent),
}

impl Event {
    /// A short name for the kind of event, cheap enough to take for every
    /// event, for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::P2p(P2pEvent::Connection(_)) => "P2pConnection",
            Self::P2p(P2pEvent::Channel(event)) => match event {
                P2pChannelEvent::Opened(..) => "P2pChannelOpened",
                P2pChannelEvent::Sent(..) => "P2pChannelSent",
                P2pChannelEvent::Received(_, Err(_)) => "P2pChannelReceivedError",
                P2pChannelEvent::Received(_, Ok(msg)) => match msg.channel_id() {
                    ChannelId::SignalingDiscovery => "P2pChannelReceived(SignalingDiscovery)",
                    ChannelId::SignalingExchange => "P2pChannelReceived(SignalingExchange)",
                    ChannelId::BestTipPropagation => "P2pChannelReceived(BestTipPropagation)",
                    ChannelId::TransactionPropagation => {
                        "P2pChannelReceived(TransactionPropagation)"
                    }
                    ChannelId::SnarkPropagation => "P2pChannelReceived(SnarkPropagation)",
                    ChannelId::SnarkJobCommitmentPropagation => {
                        "P2pChannelReceived(SnarkJobCommitmentPropagation)"
                    }
                    ChannelId::Rpc => "P2pChannelReceived(Rpc)",
                    ChannelId::StreamingRpc => "P2pChannelReceived(StreamingRpc)",
                },
                P2pChannelEvent::Closed(..) => "P2pChannelClosed",
            },
            Self::P2p(P2pEvent::MioEvent(_)) => "P2pMio",
            Self::Ledger(LedgerEvent::Verification(_)) => "LedgerVerification",
            Self::Ledger(LedgerEvent::Write(_)) => "LedgerWrite",
            Self::Ledger(LedgerEvent::Read(..)) => "LedgerRead",
            Self::Snark(SnarkEvent::BlockVerify(..)) => "SnarkBlockVerify",
            Self::Snark(SnarkEvent::BlockVerifyMeasured { .. }) => "SnarkBlockVerifyMeasured",
            Self::Snark(SnarkEvent::WorkVerify(..)) => "SnarkWorkVerify",
            Self::Snark(SnarkEvent::UserCommandVerify(..)) => "SnarkUserCommandVerify",
            Self::Rpc(..) => "Rpc",
            Self::ExternalSnarkWorker(_) => "ExternalSnarkWorker",
            Self::BlockProducerEvent(_) => "BlockProducer",
            Self::GenesisLoad(_) => "GenesisLoad",
            Self::GenesisLedgersLoad(_) => "GenesisLedgersLoad",
            Self::Persist(_) => "Persist",
        }
    }
}

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::P2p(v) => v.fmt(f),
            Self::Ledger(v) => v.fmt(f),
            Self::Snark(v) => v.fmt(f),
            Self::Rpc(id, req) => {
                write!(f, "Rpc, {id}, ")?;
                match req.as_ref() {
                    RpcRequest::StateGet(filter) => write!(f, "StateGet, {filter:?}"),
                    RpcRequest::StatusGet => write!(f, "StatusGet"),
                    RpcRequest::MemoryDebugGet => write!(f, "MemoryDebugGet"),
                    RpcRequest::MetricsGet => write!(f, "MetricsGet"),
                    RpcRequest::HeartbeatGet => write!(f, "HeartbeatGet"),
                    RpcRequest::ActionStatsGet(query) => write!(f, "ActionStatsGet, {query:?}"),
                    RpcRequest::SyncStatsGet(query) => write!(f, "SyncStatsGet, {query:?}"),
                    RpcRequest::VerificationStatsGet(_) => write!(f, "VerificationStatsGet"),
                    RpcRequest::SnarkWorkCoverageGet => write!(f, "SnarkWorkCoverageGet"),
                    RpcRequest::ProofMemoryGet => write!(f, "ProofMemoryGet"),
                    RpcRequest::BlockProducerStatsGet => write!(f, "BlockProducerStatsGet"),
                    RpcRequest::PeersGet => write!(f, "PeersGet"),
                    RpcRequest::MessageProgressGet => write!(f, "MessageProgressGet"),
                    RpcRequest::P2pConnectionOutgoing(opts) => {
                        write!(f, "P2pConnectionOutgoing, {opts}")
                    }
                    RpcRequest::P2pConnectionIncoming(opts) => {
                        write!(f, "P2pConnectionIncoming, {}", opts.peer_id)
                    }
                    RpcRequest::ScanStateSummaryGet(query) => {
                        write!(f, "ScanStateSummaryGet, {query:?}")
                    }
                    RpcRequest::SnarkPoolGet => write!(f, "SnarkPoolGet"),
                    RpcRequest::SnarkPoolJobGet { job_id } => {
                        write!(f, "SnarkPoolJobGet, {job_id}")
                    }
                    RpcRequest::SnarkPoolCompletedJobsGet => write!(f, "SnarkPoolCompletedJobsGet"),
                    RpcRequest::SnarkPoolPendingJobsGet => write!(f, "SnarkPoolPendingJobsGet"),
                    RpcRequest::SnarkerConfig => write!(f, "SnarkerConfig"),
                    RpcRequest::SnarkerJobCommit { job_id } => {
                        write!(f, "SnarkerJobCommit, {job_id}")
                    }
                    RpcRequest::SnarkerJobSpec { job_id } => write!(f, "SnarkerJobSpec, {job_id}"),
                    RpcRequest::SnarkerWorkers => write!(f, "SnarkerWorkers"),
                    RpcRequest::SnarkWorkerControlGet => write!(f, "SnarkWorkerControlGet"),
                    RpcRequest::SnarkWorkerControlSet(setting) => write!(
                        f,
                        "SnarkWorkerControlSet, {}",
                        if setting.is_some() { "on" } else { "off" }
                    ),
                    RpcRequest::SnarkWorkIncludedGet => write!(f, "SnarkWorkIncludedGet"),
                    RpcRequest::HealthCheck => write!(f, "HealthCheck"),
                    RpcRequest::ReadinessCheck => write!(f, "ReadinessCheck"),
                    RpcRequest::DiscoveryRoutingTable => write!(f, "DiscoveryRoutingTable"),
                    RpcRequest::DiscoveryBoostrapStats => write!(f, "DiscoveryBoostrapStats"),
                    RpcRequest::TransactionPoolGet => write!(f, "TransactionPool"),
                    RpcRequest::LedgerAccountsGet(account_query) => {
                        write!(f, "LedgerAccountsGet, {account_query:?}")
                    }
                    RpcRequest::TransactionInject(..) => write!(f, "TransactionInject"),
                    RpcRequest::TransitionFrontierUserCommandsGet => {
                        write!(f, "TransitionFrontierUserCommandsGet")
                    }
                    RpcRequest::BestChain(..) => write!(f, "BestChain"),
                    RpcRequest::ConsensusConstantsGet => write!(f, "ConsensusConstantsGet"),
                    RpcRequest::TransactionStatusGet(..) => write!(f, "TransactionStatusGet"),
                    RpcRequest::TransactionNonceGet(..) => write!(f, "TransactionNonceGet"),
                    RpcRequest::GetBlock(..) => write!(f, "GetBlock"),
                    RpcRequest::PooledUserCommands(..) => write!(f, "PooledUserCommands"),
                    RpcRequest::PooledZkappCommands(..) => write!(f, "PooledZkappCommands"),
                    RpcRequest::GenesisBlockGet => write!(f, "GenesisBlock"),
                    RpcRequest::ConsensusTimeGet(..) => write!(f, "ConsensusTimeGet"),
                    RpcRequest::LedgerStatusGet(..) => write!(f, "LedgerStatusGet"),
                    RpcRequest::LedgerAccountDelegatorsGet(..) => {
                        write!(f, "LedgerAccountDelegatorsGet")
                    }
                    RpcRequest::LedgerExport(kind) => write!(f, "LedgerExport, {kind}"),
                    RpcRequest::LightBestTip => write!(f, "LightBestTip"),
                    RpcRequest::LightAccount(query) => {
                        write!(f, "LightAccount, {}", query.ledger)
                    }
                    RpcRequest::HardForkConfig(spec) => write!(f, "HardForkConfig, {spec:?}"),
                }
            }
            Self::ExternalSnarkWorker(event) => {
                write!(f, "ExternalSnarkWorker, ")?;

                match event {
                    ExternalSnarkWorkerEvent::Started => write!(f, "Started"),
                    ExternalSnarkWorkerEvent::Killed => write!(f, "Killed"),
                    ExternalSnarkWorkerEvent::WorkResult(_) => write!(f, "WorkResult"),
                    ExternalSnarkWorkerEvent::WorkError(_) => write!(f, "WorkError"),
                    ExternalSnarkWorkerEvent::WorkCancelled => write!(f, "WorkCancelled"),
                    ExternalSnarkWorkerEvent::Error(_) => write!(f, "Error"),
                }
            }
            Self::BlockProducerEvent(event) => event.fmt(f),
            Self::GenesisLoad(res) => {
                write!(f, "GenesisLoad, ")?;
                match res {
                    Err(_) => {
                        write!(f, "Err")
                    }
                    Ok(data) => {
                        write!(f, "Ok, {}", data.genesis_ledger_hash)
                    }
                }
            }
            Self::GenesisLedgersLoad(res) => match res {
                Ok(()) => write!(f, "GenesisLedgersLoad, Ok"),
                Err(error) => write!(f, "GenesisLedgersLoad, Err: {error}"),
            },
            Self::Persist(event) => {
                use crate::persist::event::PersistEvent;
                write!(f, "Persist, ")?;
                match event {
                    PersistEvent::Opened(opened) => write!(f, "Opened, {}", opened.outcome),
                    PersistEvent::OpenFailed { error } => write!(f, "OpenFailed, {error}"),
                    PersistEvent::ChainPut(Ok(done)) => write!(f, "ChainPut, Ok, {}", done.tip),
                    PersistEvent::ChainPut(Err(error)) => {
                        write!(f, "ChainPut, Err, {}", error.error)
                    }
                    PersistEvent::Checkpoint(Ok(done)) => {
                        write!(f, "Checkpoint, Ok, {}", done.root)
                    }
                    PersistEvent::Checkpoint(Err(error)) => {
                        write!(f, "Checkpoint, Err, {}", error.error)
                    }
                    PersistEvent::Summary(store) => {
                        write!(f, "Summary, generation {}", store.generation)
                    }
                    PersistEvent::Loaded(result) => {
                        use crate::persist::event::PersistLoadResult;
                        write!(f, "Loaded, ")?;
                        match result.as_ref() {
                            PersistLoadResult::Restored(restored) => write!(
                                f,
                                "Restored, {} blocks to {}",
                                restored.blocks.len(),
                                restored.tip().hash()
                            ),
                            PersistLoadResult::LedgersOnly { reason, .. } => {
                                write!(f, "LedgersOnly, {reason}")
                            }
                            PersistLoadResult::Absent { reason } => write!(f, "Absent, {reason}"),
                            PersistLoadResult::Discarded { reason } => {
                                write!(f, "Discarded, {reason}")
                            }
                            PersistLoadResult::Error { error } => write!(f, "Error, {error}"),
                            PersistLoadResult::Cancelled => write!(f, "Cancelled"),
                        }
                    }
                }
            }
        }
    }
}
