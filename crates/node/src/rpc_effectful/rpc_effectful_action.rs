use crate::{
    external_snark_worker::SnarkWorkId,
    p2p::{bootstrap::P2pNetworkKadBootstrapStats, connection::P2pConnectionResponse},
    rpc::{
        discovery::RpcDiscoveryRoutingTable, AccountQuery, ActionStatsQuery, RpcBestChainResponse,
        RpcConsensusTimeGetResponse, RpcGenesisBlockResponse, RpcGetBlockResponse,
        RpcHardForkExportResponse, RpcLedgerAccountDelegatorsGetResponse, RpcLedgerExportResponse,
        RpcLedgerStatusGetResponse, RpcLightAccountResponse, RpcLightBestTipResponse, RpcPeerInfo,
        RpcPooledUserCommandsResponse, RpcPooledZkappCommandsResponse,
        RpcScanStateSummaryScanStateJob, RpcSnarkPoolCompletedJobsResponse,
        RpcSnarkPoolPendingJobsGetResponse, RpcSnarkerConfig, RpcTransactionInjectFailure,
        RpcTransactionInjectRejected, RpcTransactionInjectSuccess, SyncStatsQuery,
    },
};
use ledger::{
    scan_state::transaction_logic::{valid::UserCommand, zkapp_command::WithHash},
    Account,
};
use mina_core::{consensus::ConsensusConstants, requests::RpcId, snark::SnarkJobId, ActionEvent};
use mina_p2p_messages::v2::{self, MinaBaseUserCommandStableV2};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, ActionEvent)]
pub enum RpcEffectfulAction {
    GlobalStateGet {
        rpc_id: RpcId,
        filter: Option<String>,
    },
    StatusGet {
        rpc_id: RpcId,
    },
    MemoryDebugGet {
        rpc_id: RpcId,
    },
    MetricsGet {
        rpc_id: RpcId,
    },
    HeartbeatGet {
        rpc_id: RpcId,
    },
    ActionStatsGet {
        rpc_id: RpcId,
        query: ActionStatsQuery,
    },
    SyncStatsGet {
        rpc_id: RpcId,
        query: SyncStatsQuery,
    },
    VerificationStatsGet {
        rpc_id: RpcId,
        query: crate::verification::VerificationQuery,
    },
    SnarkWorkCoverageGet {
        rpc_id: RpcId,
    },
    ProofMemoryGet {
        rpc_id: RpcId,
    },
    BlockProducerStatsGet {
        rpc_id: RpcId,
    },

    MessageProgressGet {
        rpc_id: RpcId,
    },
    PeersGet {
        rpc_id: RpcId,
        peers: Vec<RpcPeerInfo>,
    },
    P2pConnectionOutgoingError {
        rpc_id: RpcId,
        error: String,
    },
    P2pConnectionOutgoingSuccess {
        rpc_id: RpcId,
    },
    P2pConnectionIncomingRespond {
        rpc_id: RpcId,
        response: P2pConnectionResponse,
    },
    P2pConnectionIncomingError {
        rpc_id: RpcId,
        error: String,
    },
    P2pConnectionIncomingSuccess {
        rpc_id: RpcId,
    },
    ScanStateSummaryGetSuccess {
        rpc_id: RpcId,
        scan_state: Result<Vec<Vec<RpcScanStateSummaryScanStateJob>>, String>,
    },
    SnarkPoolAvailableJobsGet {
        rpc_id: RpcId,
    },
    SnarkPoolJobGet {
        job_id: SnarkWorkId,
        rpc_id: RpcId,
    },
    SnarkPoolCompletedJobsGet {
        rpc_id: RpcId,
        jobs: RpcSnarkPoolCompletedJobsResponse,
    },
    SnarkPoolPendingJobsGet {
        rpc_id: RpcId,
        jobs: RpcSnarkPoolPendingJobsGetResponse,
    },
    SnarkerConfigGet {
        rpc_id: RpcId,
        config: Option<RpcSnarkerConfig>,
    },
    SnarkerJobCommit {
        rpc_id: RpcId,
        job_id: SnarkJobId,
    },
    SnarkerJobSpec {
        rpc_id: RpcId,
        job_id: SnarkJobId,
    },
    /// The worker's state, not the whole worker: its log of included
    /// work would be cloned for every request (P5.4).
    SnarkerWorkersGet {
        rpc_id: RpcId,
        snark_worker: Box<crate::rpc::RpcSnarkWorker>,
    },
    SnarkWorkerControlGet {
        rpc_id: RpcId,
    },
    /// Answers a worker change: the worker as it is now, or `error`.
    SnarkWorkerControlSet {
        rpc_id: RpcId,
        error: Option<String>,
    },
    SnarkWorkIncludedGet {
        rpc_id: RpcId,
    },
    HealthCheck {
        rpc_id: RpcId,
    },
    ReadinessCheck {
        rpc_id: RpcId,
    },
    DiscoveryRoutingTable {
        rpc_id: RpcId,
        response: Option<RpcDiscoveryRoutingTable>,
    },
    DiscoveryBoostrapStats {
        rpc_id: RpcId,
        response: Option<P2pNetworkKadBootstrapStats>,
    },
    TransactionPool {
        rpc_id: RpcId,
        response: Vec<WithHash<UserCommand, v2::TransactionHash>>,
    },
    LedgerAccountsGetSuccess {
        rpc_id: RpcId,
        accounts: Vec<Account>,
        account_query: AccountQuery,
    },
    TransactionInjectSuccess {
        rpc_id: RpcId,
        response: RpcTransactionInjectSuccess,
    },
    TransactionInjectRejected {
        rpc_id: RpcId,
        response: RpcTransactionInjectRejected,
    },
    TransactionInjectFailure {
        rpc_id: RpcId,
        errors: RpcTransactionInjectFailure,
    },
    TransitionFrontierUserCommandsGet {
        rpc_id: RpcId,
        commands: Vec<MinaBaseUserCommandStableV2>,
    },
    BestChain {
        rpc_id: RpcId,
        best_chain: RpcBestChainResponse,
    },
    ConsensusConstantsGet {
        rpc_id: RpcId,
        response: ConsensusConstants,
    },
    TransactionStatusGet {
        rpc_id: RpcId,
        tx: MinaBaseUserCommandStableV2,
    },
    TransactionNonceGet {
        rpc_id: RpcId,
        response: crate::rpc::RpcTransactionNonceGetResponse,
    },
    BlockGet {
        rpc_id: RpcId,
        block: RpcGetBlockResponse,
    },
    PooledUserCommands {
        rpc_id: RpcId,
        user_commands: RpcPooledUserCommandsResponse,
    },
    PooledZkappCommands {
        rpc_id: RpcId,
        zkapp_commands: RpcPooledZkappCommandsResponse,
    },
    GenesisBlock {
        rpc_id: RpcId,
        genesis_block: RpcGenesisBlockResponse,
    },
    ConsensusTimeGet {
        rpc_id: RpcId,
        consensus_time: RpcConsensusTimeGetResponse,
    },
    LedgerStatusGetSuccess {
        rpc_id: RpcId,
        response: RpcLedgerStatusGetResponse,
    },
    LedgerExportSuccess {
        rpc_id: RpcId,
        response: RpcLedgerExportResponse,
    },
    #[action_event(level = trace)]
    LightBestTip {
        rpc_id: RpcId,
        response: RpcLightBestTipResponse,
    },
    #[action_event(level = trace)]
    LightAccountSuccess {
        rpc_id: RpcId,
        response: RpcLightAccountResponse,
    },
    HardForkConfigSuccess {
        rpc_id: RpcId,
        response: RpcHardForkExportResponse,
    },
    LedgerAccountDelegatorsGetSuccess {
        rpc_id: RpcId,
        response: RpcLedgerAccountDelegatorsGetResponse,
    },
}

impl redux::EnablingCondition<crate::State> for RpcEffectfulAction {
    fn is_enabled(&self, _state: &crate::State, _time: redux::Timestamp) -> bool {
        true
    }
}
