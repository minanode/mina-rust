use crate::p2p::PeerId;
use ledger::{
    transaction_pool::{diff, ValidCommandWithHash},
    Account, AccountId,
};
use mina_core::{block::AppliedBlock, snark::SnarkJobId, ActionEvent};
use mina_node_account::AccountPublicKey;
use mina_p2p_messages::v2::{LedgerHash, MinaBaseUserCommandStableV2, TokenIdKeyHash};
use serde::{Deserialize, Serialize};

use crate::{
    external_snark_worker::SnarkWorkId,
    p2p::connection::{
        incoming::P2pConnectionIncomingInitOpts,
        outgoing::{P2pConnectionOutgoingError, P2pConnectionOutgoingInitOpts},
        P2pConnectionResponse,
    },
};

use super::{
    ActionStatsQuery, ConsensusTimeQuery, DelegatorsQuery, GetBlockQuery, HardForkBlockSpec,
    LedgerExportKind, PooledUserCommandsQuery, PooledZkappsCommandsQuery,
    RpcHardForkExportResponse, RpcId, RpcLedgerAccountDelegatorsGetResponse,
    RpcLedgerExportResponse, RpcLedgerStatusGetResponse, RpcLightAccountQuery,
    RpcLightAccountResponse, RpcScanStateSummaryGetQuery, RpcScanStateSummaryScanStateJob,
    SyncStatsQuery,
};

#[derive(Serialize, Deserialize, Debug, Clone, ActionEvent)]
pub enum RpcAction {
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

    // Stats
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
    },

    P2pConnectionOutgoingInit {
        rpc_id: RpcId,
        opts: P2pConnectionOutgoingInitOpts,
    },
    P2pConnectionOutgoingPending {
        rpc_id: RpcId,
    },
    P2pConnectionOutgoingError {
        rpc_id: RpcId,
        error: P2pConnectionOutgoingError,
    },
    P2pConnectionOutgoingSuccess {
        rpc_id: RpcId,
    },

    P2pConnectionIncomingInit {
        rpc_id: RpcId,
        opts: P2pConnectionIncomingInitOpts,
    },
    P2pConnectionIncomingPending {
        rpc_id: RpcId,
    },
    P2pConnectionIncomingRespond {
        rpc_id: RpcId,
        response: P2pConnectionResponse,
    },
    P2pConnectionIncomingAnswerReady {
        rpc_id: RpcId,
        peer_id: PeerId,
        answer: P2pConnectionResponse,
    },
    P2pConnectionIncomingError {
        rpc_id: RpcId,
        error: String,
    },
    P2pConnectionIncomingSuccess {
        rpc_id: RpcId,
    },

    ScanStateSummaryGetInit {
        rpc_id: RpcId,
        query: RpcScanStateSummaryGetQuery,
    },
    ScanStateSummaryLedgerGetInit {
        rpc_id: RpcId,
    },
    ScanStateSummaryGetPending {
        rpc_id: RpcId,
        block: Option<AppliedBlock>,
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
    },
    SnarkPoolPendingJobsGet {
        rpc_id: RpcId,
    },
    SnarkerConfigGet {
        rpc_id: RpcId,
    },
    SnarkerJobCommit {
        rpc_id: RpcId,
        job_id: SnarkJobId,
    },
    SnarkerJobSpec {
        rpc_id: RpcId,
        job_id: SnarkJobId,
    },

    SnarkerWorkersGet {
        rpc_id: RpcId,
    },
    SnarkWorkerControlGet {
        rpc_id: RpcId,
    },
    SnarkWorkerControlSet {
        rpc_id: RpcId,
        setting: Option<crate::SnarkerSetting>,
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
    },
    DiscoveryBoostrapStats {
        rpc_id: RpcId,
    },

    TransactionPool {
        rpc_id: RpcId,
    },
    #[action_event(level = debug)]
    LedgerAccountsGetInit {
        rpc_id: RpcId,
        account_query: AccountQuery,
    },
    #[action_event(level = debug)]
    LedgerAccountsGetPending {
        rpc_id: RpcId,
    },
    #[action_event(level = debug)]
    LedgerAccountsGetSuccess {
        rpc_id: RpcId,
        accounts: Vec<Account>,
        account_query: AccountQuery,
    },
    #[action_event(level = debug)]
    TransactionInjectInit {
        rpc_id: RpcId,
        commands: Vec<MinaBaseUserCommandStableV2>,
    },
    #[action_event(level = debug)]
    TransactionInjectPending {
        rpc_id: RpcId,
    },
    #[action_event(level = info)]
    TransactionInjectSuccess {
        rpc_id: RpcId,
        response: Vec<ValidCommandWithHash>,
    },
    #[action_event(level = info)]
    TransactionInjectRejected {
        rpc_id: RpcId,
        response: Vec<(ValidCommandWithHash, diff::Error)>,
    },
    #[action_event(level = warn)]
    TransactionInjectFailure {
        rpc_id: RpcId,
        errors: Vec<String>,
    },
    #[action_event(level = debug)]
    TransitionFrontierUserCommandsGet {
        rpc_id: RpcId,
    },

    BestChain {
        rpc_id: RpcId,
        max_length: u32,
    },
    ConsensusConstantsGet {
        rpc_id: RpcId,
    },

    TransactionStatusGet {
        rpc_id: RpcId,
        tx: MinaBaseUserCommandStableV2,
    },
    TransactionNonceGet {
        rpc_id: RpcId,
        account_id: AccountId,
    },

    BlockGet {
        rpc_id: RpcId,
        query: GetBlockQuery,
    },
    ConsensusTimeGet {
        rpc_id: RpcId,
        query: ConsensusTimeQuery,
    },
    LedgerStatusGetInit {
        rpc_id: RpcId,
        ledger_hash: LedgerHash,
    },
    LedgerStatusGetPending {
        rpc_id: RpcId,
    },
    LedgerStatusGetSuccess {
        rpc_id: RpcId,
        response: RpcLedgerStatusGetResponse,
    },
    #[action_event(level = debug)]
    LedgerAccountDelegatorsGetInit {
        rpc_id: RpcId,
        query: DelegatorsQuery,
        account_id: AccountId,
    },
    #[action_event(level = debug)]
    LedgerAccountDelegatorsGetPending {
        rpc_id: RpcId,
    },
    #[action_event(level = debug)]
    LedgerAccountDelegatorsGetSuccess {
        rpc_id: RpcId,
        response: RpcLedgerAccountDelegatorsGetResponse,
    },

    #[action_event(level = debug)]
    LedgerExportInit {
        rpc_id: RpcId,
        kind: LedgerExportKind,
    },
    #[action_event(level = debug)]
    LedgerExportPending {
        rpc_id: RpcId,
    },
    #[action_event(level = info)]
    LedgerExportSuccess {
        rpc_id: RpcId,
        response: RpcLedgerExportResponse,
    },

    #[action_event(level = info)]
    HardForkConfigInit {
        rpc_id: RpcId,
        spec: HardForkBlockSpec,
    },
    #[action_event(level = info)]
    HardForkConfigPending {
        rpc_id: RpcId,
    },
    #[action_event(level = info)]
    HardForkConfigSuccess {
        rpc_id: RpcId,
        response: RpcHardForkExportResponse,
    },

    PooledUserCommands {
        rpc_id: RpcId,
        query: PooledUserCommandsQuery,
    },
    PooledZkappCommands {
        rpc_id: RpcId,
        query: PooledZkappsCommandsQuery,
    },
    GenesisBlock {
        rpc_id: RpcId,
    },

    #[action_event(level = trace)]
    LightBestTipGet {
        rpc_id: RpcId,
    },
    #[action_event(level = trace)]
    LightAccountInit {
        rpc_id: RpcId,
        query: RpcLightAccountQuery,
    },
    #[action_event(level = trace)]
    LightAccountPending {
        rpc_id: RpcId,
    },
    #[action_event(level = trace)]
    LightAccountSuccess {
        rpc_id: RpcId,
        response: RpcLightAccountResponse,
    },
    /// Answers light requests the ledger read quota held back past
    /// [`super::LIGHT_REQUEST_TIMEOUT`] and drops them.
    #[action_event(level = trace)]
    LightAccountsExpire,

    Finish {
        rpc_id: RpcId,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AccountQuery {
    All,
    SinglePublicKey(AccountPublicKey),
    MultipleIds(Vec<AccountId>),
    PubKeyWithTokenId(AccountPublicKey, TokenIdKeyHash),
    /// The account that owns a token: the one whose id derives it.
    TokenOwner(TokenIdKeyHash),
    /// Accounts of the best tip's staking epoch ledger, the one its epoch's
    /// stake comes from, rather than of its ledger.
    StakingEpochIds(Vec<AccountId>),
}

impl redux::EnablingCondition<crate::State> for RpcAction {
    fn is_enabled(&self, state: &crate::State, time: redux::Timestamp) -> bool {
        match self {
            RpcAction::GlobalStateGet { .. } => true,
            RpcAction::StatusGet { .. } => true,
            RpcAction::MemoryDebugGet { .. } => true,
            RpcAction::MetricsGet { .. } => true,
            RpcAction::HeartbeatGet { .. } => true,
            RpcAction::ActionStatsGet { .. } => true,
            RpcAction::SyncStatsGet { .. } => true,
            RpcAction::BlockProducerStatsGet { .. } => true,
            RpcAction::VerificationStatsGet { .. } => true,
            RpcAction::SnarkWorkCoverageGet { .. } => true,
            RpcAction::ProofMemoryGet { .. } => true,
            RpcAction::MessageProgressGet { .. } => true,
            RpcAction::PeersGet { .. } => true,
            RpcAction::P2pConnectionOutgoingInit { rpc_id, .. } => {
                !state.rpc.requests.contains_key(rpc_id)
            }
            RpcAction::P2pConnectionOutgoingPending { rpc_id } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_init()),
            RpcAction::P2pConnectionOutgoingError { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_pending()),
            RpcAction::P2pConnectionOutgoingSuccess { rpc_id } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_pending()),
            RpcAction::P2pConnectionIncomingInit { rpc_id, .. } => {
                !state.rpc.requests.contains_key(rpc_id)
            }
            RpcAction::P2pConnectionIncomingPending { rpc_id } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_init()),
            RpcAction::P2pConnectionIncomingRespond { rpc_id, .. }
            | RpcAction::P2pConnectionIncomingAnswerReady { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_init() || v.status.is_pending()),
            RpcAction::P2pConnectionIncomingError { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_init() || v.status.is_pending()),
            RpcAction::P2pConnectionIncomingSuccess { rpc_id } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_pending()),
            RpcAction::ScanStateSummaryGetInit { .. } => true,
            RpcAction::ScanStateSummaryLedgerGetInit { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_init()),
            RpcAction::ScanStateSummaryGetPending { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_init()),
            RpcAction::ScanStateSummaryGetSuccess { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_pending()),
            RpcAction::SnarkPoolAvailableJobsGet { .. } => true,
            RpcAction::SnarkPoolJobGet { .. } => true,
            RpcAction::SnarkPoolCompletedJobsGet { .. } => true,
            RpcAction::SnarkPoolPendingJobsGet { .. } => true,
            RpcAction::SnarkerConfigGet { .. } => true,
            RpcAction::SnarkerJobCommit { .. } => true,
            RpcAction::SnarkerJobSpec { .. } => true,
            RpcAction::SnarkerWorkersGet { .. } => true,
            RpcAction::SnarkWorkerControlGet { .. } => true,
            RpcAction::SnarkWorkerControlSet { .. } => true,
            RpcAction::SnarkWorkIncludedGet { .. } => true,
            RpcAction::HealthCheck { .. } => true,
            RpcAction::ReadinessCheck { .. } => true,
            RpcAction::DiscoveryRoutingTable { .. } => true,
            RpcAction::DiscoveryBoostrapStats { .. } => true,
            RpcAction::TransactionPool { .. } => true,
            RpcAction::ConsensusConstantsGet { .. } => true,
            RpcAction::BestChain { .. } => state.transition_frontier.best_tip().is_some(),
            RpcAction::TransactionStatusGet { .. } => true,
            RpcAction::TransactionNonceGet { .. } => true,
            RpcAction::PooledUserCommands { .. } => true,
            RpcAction::PooledZkappCommands { .. } => true,
            RpcAction::GenesisBlock { .. } => true,
            RpcAction::LedgerAccountsGetInit { .. } => {
                state.transition_frontier.best_tip().is_some()
            }
            RpcAction::LedgerAccountsGetPending { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_init()),
            RpcAction::LedgerAccountsGetSuccess { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_pending()),

            RpcAction::TransactionInjectInit { .. } => true,
            RpcAction::TransactionInjectPending { rpc_id } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_init()),
            RpcAction::TransactionInjectSuccess { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_pending()),
            RpcAction::TransactionInjectRejected { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_pending()),
            RpcAction::TransactionInjectFailure { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_pending()),
            RpcAction::TransitionFrontierUserCommandsGet { .. } => true,
            RpcAction::BlockGet { .. } => true,
            RpcAction::ConsensusTimeGet { .. } => true,
            RpcAction::LedgerStatusGetInit { .. } => state.transition_frontier.best_tip().is_some(),
            RpcAction::LedgerStatusGetPending { rpc_id } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_init()),
            RpcAction::LedgerStatusGetSuccess { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_pending()),
            RpcAction::LedgerAccountDelegatorsGetInit { .. } => {
                state.transition_frontier.best_tip().is_some()
            }
            RpcAction::LedgerAccountDelegatorsGetPending { rpc_id } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_init()),
            RpcAction::LedgerAccountDelegatorsGetSuccess { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_pending()),
            RpcAction::LedgerExportInit { .. } => state.transition_frontier.best_tip().is_some(),
            RpcAction::LedgerExportPending { rpc_id } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_init()),
            RpcAction::LedgerExportSuccess { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_pending()),
            RpcAction::HardForkConfigInit { .. } => state.transition_frontier.best_tip().is_some(),
            RpcAction::HardForkConfigPending { rpc_id } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_init()),
            RpcAction::HardForkConfigSuccess { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_pending()),
            RpcAction::LightBestTipGet { .. } => true,
            // A retry after the ledger read quota was full finds it in Init.
            RpcAction::LightAccountInit { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_none_or(|v| v.status.is_init()),
            RpcAction::LightAccountPending { rpc_id } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_init()),
            // From Init too: a query that names no ledger is answered at once.
            RpcAction::LightAccountSuccess { rpc_id, .. } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| !v.status.is_finished()),
            RpcAction::LightAccountsExpire => state.rpc.light_expired(time).next().is_some(),
            RpcAction::Finish { rpc_id } => state
                .rpc
                .requests
                .get(rpc_id)
                .is_some_and(|v| v.status.is_finished()),
        }
    }
}
