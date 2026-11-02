mod rpc_state;
use std::collections::BTreeMap;

use crate::p2p::bootstrap::P2pNetworkKadBootstrapStats;
use ledger::{
    scan_state::{
        currency::{Amount, Balance, Fee, Nonce, Slot},
        transaction_logic::{signed_command, signed_command::SignedCommandPayload, valid, Memo},
    },
    transaction_pool::{diff, ValidCommandWithHash},
    Account, AccountId,
};
use mina_core::{
    block::{AppliedBlock, ArcBlockWithHash},
    consensus::{ConsensusConstants, ConsensusTime},
};
use mina_node_account::AccountPublicKey;
use mina_p2p_messages::{
    bigint::BigInt,
    v2::{
        LedgerHash, MinaBaseSignedCommandPayloadBodyStableV2, MinaBaseSignedCommandStableV2,
        MinaBaseTransactionStatusStableV2, MinaBaseUserCommandStableV2,
        MinaBaseZkappCommandTStableV1WireStableV1, MinaTransactionTransactionStableV2,
        SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponse, StateHash, TransactionHash,
        TransactionSnarkWorkTStableV2,
    },
};
pub use rpc_state::*;

mod rpc_actions;
pub use rpc_actions::*;

mod rpc_reducer;
pub use rpc_reducer::collect_rpc_peers_info;

mod rpc_impls;

mod rpc_health;
pub use rpc_health::*;
mod ledger_export;
pub use ledger_export::*;

mod hard_fork;
pub use hard_fork::*;

mod light;
pub use light::*;

mod chain_view;
pub use chain_view::*;
#[cfg(test)]
mod chain_view_tests;
#[cfg(test)]
mod light_tests;

mod heartbeat;
pub use heartbeat::{
    NodeHeartbeat, NodeStatus as HeartbeatNodeStatus, ProducedBlockInfo, SignedNodeHeartbeat,
};

pub use mina_core::requests::{RpcId, RpcIdType};

/// The answer to a transaction submission while the node has no usable
/// transition frontier, OCaml's text.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/mina_graphql/mina_graphql.ml#L365-L366>
pub const TRANSACTION_INJECT_BOOTSTRAPPING: &str = "Daemon is bootstrapping";

use ledger::scan_state::scan_state::{transaction_snark::OneOrTwo, AvailableJobMessage};
use mina_core::snark::SnarkJobId;
use mina_p2p_messages::v2::{CurrencyFeeStableV1, NonZeroCurvePoint};
use redux::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    external_snark_worker::{
        ExternalSnarkWorkerError, ExternalSnarkWorkerWorkError, SnarkWorkSpecError,
    },
    ledger::{
        read::{LedgerReadId, LedgerReadKind, LedgerStatus},
        write::LedgerWriteKind,
    },
    p2p::{
        connection::{
            incoming::P2pConnectionIncomingInitOpts, outgoing::P2pConnectionOutgoingInitOpts,
        },
        PeerId,
    },
    service::Queues,
    snark_pool::{JobCommitment, JobState, JobSummary},
    stats::{
        actions::{ActionStatsForBlock, ActionStatsSnapshot},
        block_producer::{
            BlockProductionAttempt, BlockProductionAttemptWonSlot, VrfEvaluatorStats,
        },
        sync::SyncStatsSnapshot,
    },
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RpcRequest {
    StateGet(Option<String>),
    StatusGet,
    GraphqlStatusGet,
    MemoryDebugGet,
    MetricsGet,
    HeartbeatGet,
    ActionStatsGet(ActionStatsQuery),
    SyncStatsGet(SyncStatsQuery),
    BlockProducerStatsGet,
    VerificationStatsGet(crate::verification::VerificationQuery),
    SnarkWorkCoverageGet,
    /// PC1: the proofs the node holds, by holder.
    ProofMemoryGet,
    MessageProgressGet,
    PeersGet,
    P2pConnectionOutgoing(P2pConnectionOutgoingInitOpts),
    P2pConnectionIncoming(P2pConnectionIncomingInitOpts),
    ScanStateSummaryGet(RpcScanStateSummaryGetQuery),
    SnarkPoolGet,
    SnarkPoolJobGet {
        job_id: SnarkJobId,
    },
    SnarkPoolCompletedJobsGet,
    SnarkPoolPendingJobsGet,
    SnarkerConfig,
    SnarkerJobCommit {
        job_id: SnarkJobId,
    },
    SnarkerJobSpec {
        job_id: SnarkJobId,
    },
    SnarkerWorkers,
    /// The worker as the operator sees it (P5.4).
    SnarkWorkerControlGet,
    /// Starts the worker with a key and fee, re-prices it, or stops it
    /// (`None`), if the node allows it (P5.4).
    SnarkWorkerControlSet(Option<crate::SnarkerSetting>),
    /// Our work that blocks included (P5.4).
    SnarkWorkIncludedGet,
    HealthCheck,
    ReadinessCheck,
    DiscoveryRoutingTable,
    DiscoveryBoostrapStats,
    TransactionPoolGet,
    LedgerAccountsGet(AccountQuery),
    TransactionInject(Vec<MinaBaseUserCommandStableV2>),
    TransitionFrontierUserCommandsGet,
    BestChain(MaxLength),
    ConsensusConstantsGet,
    TransactionStatusGet(MinaBaseUserCommandStableV2),
    GetBlock(GetBlockQuery),
    PooledUserCommands(PooledUserCommandsQuery),
    PooledZkappCommands(PooledZkappsCommandsQuery),
    GenesisBlockGet,
    ConsensusTimeGet(ConsensusTimeQuery),
    LedgerStatusGet(LedgerHash),
    LedgerAccountDelegatorsGet(DelegatorsQuery, AccountId),
    /// What the node knows of a sender's next nonce, for `sendPayment` and
    /// `sendDelegation`.
    TransactionNonceGet(AccountId),
    /// A whole ledger at the best tip, as `mina ledger export` prints it.
    LedgerExport(LedgerExportKind),
    /// The next hard fork's runtime config inputs and ledgers, as OCaml's
    /// `mina advanced generate-hardfork-config` derives them.
    HardForkConfig(HardForkBlockSpec),
    /// LN2: the best chain for a light client's tip check, while synced.
    LightBestTip,
    /// LN2: one account with its Merkle path in a best-chain block's ledger.
    LightAccount(RpcLightAccountQuery),
    /// MINAWEB-RPC1: the best chain's headers and counts, newest first.
    BestChainHeadersGet(RpcBestChainHeadersQuery),
    /// MINAWEB-RPC1: one best-chain block, with what it applied.
    BestChainBlockGet(RpcBestChainBlockQuery),
    /// MINAWEB-RPC1: a sync's progress, ledger by ledger and block by block.
    SyncProgressGet,
    /// What the opt-in uptime, node status and node error reporters send
    /// (R43e.2), sampled at once.
    ReportingSnapshotGet {
        /// Only the uptime reporter changes its local production subscription.
        produced_block_watch: Option<RpcReportingProducedBlockWatch>,
    },
}

/// Maximum last-epoch delegators returned per public GraphQL lookup.
/// Keep the public cost multiplier tied to this result/work bound.
pub const MAX_LAST_EPOCH_DELEGATORS: usize = 8;

/// Which delegators [`RpcRequest::LedgerAccountDelegatorsGet`] lists.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum DelegatorsQuery {
    /// Every account that delegates to the key in this ledger: a scan of the
    /// whole ledger on the ledger thread.
    Ledger(LedgerHash),
    /// OCaml's `lastEpochDelegators`: the accounts that delegated to this
    /// node's block producer key in the staking ledger of the epoch before the
    /// root's, kept by the ledger service. `None` until the root has crossed
    /// an epoch while the node ran; empty for any other key. No scan.
    /// An optional public request limit, applied before cloning in ledger order.
    LastEpoch { limit: Option<usize> },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ConsensusTimeQuery {
    Now,
    BestTip,
}

pub type MaxLength = u32;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcInjectPayment {
    fee: u64,
    amount: u64,
    to: AccountPublicKey,
    from: AccountPublicKey,
    memo: String,
    nonce: u32,
    valid_until: u32,
    signature_field: BigInt,
    signature_scalar: BigInt,
}
// MinaBaseUserCommandStableV2
impl TryFrom<RpcInjectPayment> for MinaBaseUserCommandStableV2 {
    type Error = String;

    fn try_from(value: RpcInjectPayment) -> Result<Self, Self::Error> {
        let invalid = |what: &str| format!("invalid {what}");
        let signature = mina_signer::Signature {
            rx: value
                .signature_field
                .try_into()
                .map_err(|_| invalid("signature field"))?,
            s: value
                .signature_scalar
                .try_into()
                .map_err(|_| invalid("signature scalar"))?,
        };
        let sc = signed_command::SignedCommand {
            payload: SignedCommandPayload::create(
                Fee::from_u64(value.fee),
                value.from.clone().try_into().map_err(|_| invalid("from"))?,
                Nonce::from_u32(value.nonce),
                Some(Slot::from_u32(value.valid_until)),
                Memo::create_from_str_checked(&value.memo).map_err(|e| e.to_string())?,
                signed_command::Body::Payment(signed_command::PaymentPayload {
                    receiver_pk: value.to.try_into().map_err(|_| invalid("to"))?,
                    amount: Amount::from_u64(value.amount),
                }),
            ),
            signer: value.from.try_into().map_err(|_| invalid("from"))?,
            signature,
        };

        Ok(MinaBaseUserCommandStableV2::SignedCommand(sc.into()))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum ActionStatsQuery {
    SinceStart,
    ForLatestBlock,
    ForBlockWithId(u64),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct SyncStatsQuery {
    pub limit: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RpcScanStateSummaryGetQuery {
    /// GraphQL `scanState`: only the block hash and the tree counters.
    Graphql(Option<StateHash>),
    ForBestTip,
    ForBlockWithHash(StateHash),
    ForBlockWithHeight(u32),
}

impl RpcScanStateSummaryGetQuery {
    /// Whether the answer needs only the tree counters, not the jobs.
    pub fn is_metrics_only(&self) -> bool {
        matches!(self, Self::Graphql(_))
    }

    /// The ledger read this query needs for a block's staged ledger.
    pub fn ledger_read(
        &self,
        staged_ledger_hash: mina_p2p_messages::v2::MinaBaseStagedLedgerHashStableV1,
    ) -> crate::ledger::read::LedgerReadRequest {
        use crate::ledger::read::LedgerReadRequest;
        if self.is_metrics_only() {
            LedgerReadRequest::ScanStateMetrics(staged_ledger_hash)
        } else {
            LedgerReadRequest::ScanStateSummary(staged_ledger_hash)
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum ActionStatsResponse {
    SinceStart { stats: ActionStatsSnapshot },
    ForBlock(ActionStatsForBlock),
}

#[derive(Serialize, Deserialize, Debug, Clone, strum_macros::Display)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum PeerConnectionStatus {
    Disconnecting,
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcPeerInfo {
    pub peer_id: PeerId,
    // TODO(openapi): generate ToSchema in Base58CheckOfBinProt macro
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub best_tip: Option<StateHash>,
    pub best_tip_height: Option<u32>,
    pub best_tip_global_slot: Option<u32>,
    pub best_tip_timestamp: Option<u64>,
    pub connection_status: PeerConnectionStatus,
    pub connecting_details: Option<String>,
    pub address: Option<String>,
    pub incoming: bool,
    pub is_libp2p: bool,
    pub time: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcScanStateSummary {
    pub trees: Vec<(u64, u64, u64)>,
    pub block: RpcScanStateSummaryBlock,
    pub scan_state: Vec<Vec<RpcScanStateSummaryScanStateJob>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcScanStateSummaryBlock {
    pub hash: StateHash,
    pub height: u32,
    pub global_slot: u32,
    pub transactions: Vec<RpcScanStateSummaryBlockTransaction>,
    pub completed_works: Vec<SnarkJobId>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcScanStateSummaryBlockTransaction {
    /// None if hashing fails.
    pub hash: Option<TransactionHash>,
    pub kind: RpcScanStateSummaryBlockTransactionKind,
    pub status: MinaBaseTransactionStatusStableV2,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum RpcScanStateSummaryBlockTransactionKind {
    Payment,
    StakeDelegation,
    Zkapp,
    FeeTransfer,
    Coinbase,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "status")]
pub enum RpcScanStateSummaryScanStateJob {
    Empty,
    Todo {
        job_id: SnarkJobId,
        bundle_job_id: SnarkJobId,
        job: RpcScanStateSummaryScanStateJobKind,
        seq_no: u64,
    },
    Pending {
        job_id: SnarkJobId,
        bundle_job_id: SnarkJobId,
        job: Box<RpcScanStateSummaryScanStateJobKind>,
        seq_no: u64,
        commitment: Option<Box<JobCommitment>>,
        snark: Option<Box<RpcSnarkPoolJobSnarkWork>>,
    },
    Done {
        job_id: SnarkJobId,
        bundle_job_id: SnarkJobId,
        job: Box<RpcScanStateSummaryScanStateJobKind>,
        seq_no: u64,
        /// Who proved the job and for what fee. Mesa scan states no longer
        /// record this, so it is unknown for scan states received from peers.
        snark: Option<Box<RpcSnarkPoolJobSnarkWorkDone>>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind")]
pub enum RpcScanStateSummaryScanStateJobKind {
    Base(RpcScanStateSummaryBlockTransaction),
    Merge,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum RpcScanStateSummaryScanStateJobStatus {
    Todo,
    Done,
}

#[derive(Serialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcSnarkPoolJobSummary {
    pub time: Timestamp,
    pub id: SnarkJobId,
    pub commitment: Option<JobCommitment>,
    pub snark: Option<RpcSnarkPoolJobSnarkWork>,
}

#[derive(Serialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcSnarkPoolJobFull {
    pub time: Timestamp,
    pub id: SnarkJobId,
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub job: OneOrTwo<AvailableJobMessage>,
    pub commitment: Option<JobCommitment>,
    pub snark: Option<RpcSnarkPoolJobSnarkWork>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcSnarkPoolJobSnarkWork {
    pub snarker: NonZeroCurvePoint,
    pub fee: CurrencyFeeStableV1,
    pub received_t: Timestamp,
    pub sender: PeerId,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcSnarkPoolJobSnarkWorkDone {
    pub snarker: NonZeroCurvePoint,
    pub fee: CurrencyFeeStableV1,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind")]
pub enum RpcSnarkerJobCommitResponse {
    Ok,
    JobNotFound,
    JobTaken,
    SnarkerBusy,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind")]
pub enum RpcSnarkerJobSpecResponse {
    Ok(Box<SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponse>),
    Err(SnarkWorkSpecError),
    JobNotFound,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcMessageProgressResponse {
    pub messages_stats: BTreeMap<PeerId, MessagesStats>,
    pub staking_ledger_sync: Option<LedgerSyncProgress>,
    pub next_epoch_ledger_sync: Option<LedgerSyncProgress>,
    pub root_ledger_sync: Option<RootLedgerSyncProgress>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MessagesStats {
    pub current_request: Option<CurrentMessageProgress>,
    pub responses: BTreeMap<String, usize>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LedgerSyncProgress {
    pub fetched: u64,
    pub estimation: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RootLedgerSyncProgress {
    pub fetched: u64,
    pub estimation: u64,
    pub staged: Option<RootStagedLedgerSyncProgress>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RootStagedLedgerSyncProgress {
    pub fetched: u64,
    pub total: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CurrentMessageProgress {
    pub name: String,
    pub received_bytes: usize,
    pub total_bytes: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone, thiserror::Error)]
pub enum RpcStateGetError {
    #[error("failed to parse filter expression: {0}")]
    FilterError(String),
}

pub type RpcStateGetResponse = Result<serde_json::Value, RpcStateGetError>;
pub type RpcStatusGetResponse = Option<RpcNodeStatus>;
/// Bounded status projection: no peer, resource or ledger scans.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcGraphqlStatusResponse {
    pub bootstrap: crate::bootstrap_status::BootstrapStatus,
    pub pool_size: usize,
    pub pool_max_size: usize,
    pub pool_min_fee: Option<String>,
}
impl RpcGraphqlStatusResponse {
    pub fn from_state(state: &crate::State) -> Self {
        Self {
            bootstrap: state.transition_frontier.bootstrap.clone(),
            pool_size: state.transaction_pool.size(),
            pool_max_size: state.transaction_pool.max_size(),
            pool_min_fee: state.transaction_pool.minimum_fee_per_weight_unit(),
        }
    }
}
/// Cheap operator telemetry snapshot, assembled on the node thread without
/// traversing the full state or performing I/O.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcMetricsGetResponse {
    pub chain_id: Option<String>,
    pub uptime_secs: u64,
    pub sync_phase: String,
    pub ready: bool,
    pub best_tip_height: Option<u32>,
    pub best_tip_global_slot: Option<u32>,
    pub highest_peer_tip_height: Option<u32>,
    pub peers_connecting: usize,
    pub peers_connected: usize,
    pub peers_disconnecting: usize,
    pub peers_disconnected: usize,
    pub transaction_pool_size: usize,
    pub transaction_pool_diff_received: u64,
    pub transaction_pool_diff_broadcasted: u64,
    pub transactions_added_to_pool: u64,
    pub snark_pool_jobs: usize,
    pub snark_pool_completed: usize,
    pub snark_pool_diff_received: u64,
    pub snark_pool_diff_broadcasted: u64,
    pub block_production_delay_buckets: [u64; 7],
    pub block_production_delay_sum_ms: u64,
    pub snark_work_received_total: u64,
    pub verification: crate::verification::VerificationSnapshot,
    pub block_producer_slots_won: u64,
    pub block_producer_blocks_produced: u64,
    pub block_producer_blocks_failed: u64,
    pub snark_worker_proofs_made: u64,
    pub snark_worker_proofs_failed: u64,
    pub snark_worker_proofs_sold: u64,
    pub ledger_sync_fetched: Option<u64>,
    pub ledger_sync_estimated: Option<u64>,
}
pub type RpcHeartbeatGetResponse = Option<SignedNodeHeartbeat>;
pub type RpcReportingSnapshotGetResponse = RpcReportingSnapshot;

/// One consistent sample of the node for the opt-in reporters (R43e.2).
/// Uptime, node status and node error reports read nothing else from the
/// state machine, so each report is built from a single instant.
#[derive(Serialize, Debug, Clone)]
pub struct RpcReportingSnapshot {
    pub time: Timestamp,
    /// The chain ID, in hex, once the P2P layer is up.
    pub chain_id: Option<String>,
    /// Our libp2p peer ID (`12D3KooW…`), once the P2P layer is up: OCaml's
    /// reporters need the node to be a gossip peer.
    pub peer_id: Option<String>,
    /// Peers with a ready connection.
    pub peer_count: usize,
    /// The best tip, with its proofs, once the node participates: not while
    /// it bootstraps, when OCaml has no transition frontier (and this node's
    /// holds the genesis block).
    pub best_tip: Option<ArcBlockWithHash>,
    /// The greatest height of the blocks received from peers whose proofs
    /// were verified, among those still held as candidates: OCaml's
    /// `max_blocklength_observed` counts gossip blocks that pass initial
    /// validation, not genesis or our own. The reporter keeps the maximum
    /// across samples, as that gauge only grows.
    pub max_observed_block_height: Option<u32>,
    /// The same over all received candidate blocks, verified or not.
    pub max_observed_unvalidated_block_height: Option<u32>,
    /// `Bootstrap`, `Catchup` or `Synced` once a sync has started, in
    /// OCaml's sense: the first sync, catching up from genesis included, is
    /// a bootstrap.
    pub sync_phase: Option<String>,
    pub block_producer: Option<AccountPublicKey>,
    /// When this node next produces a block, if it knows.
    pub next_production_time: Option<Timestamp>,
    /// The first local completion for the active uptime waiter, retained
    /// independently of later local completions and frontier changes.
    pub produced_block: Option<RpcReportingProducedBlock>,
    /// The first configured external address.
    pub external_ip: Option<String>,
    /// Which SNARK job the uptime service would prove for `best_tip`, when
    /// there is one.
    pub uptime_job: Option<RpcReportingUptimeJob>,
    /// The selected witness, captured with the block rather than looked up
    /// again after a tip change. Only the reporter requests this snapshot.
    pub uptime_spec: Option<
        Box<mina_p2p_messages::v2::SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single>,
    >,
    pub snark_work_fee: Option<mina_p2p_messages::v2::CurrencyFeeStableV1>,
}

/// The job OCaml's uptime service picks to prove for a best-tip submission:
/// from the best tip's scan-state jobs, in order, the first Transition whose
/// target second-pass ledger is the block's staged ledger hash.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/uptime_service/uptime_service.ml#L224-L328>
#[derive(Serialize, Debug, Clone, PartialEq)]
pub enum RpcReportingUptimeJob {
    /// OCaml's `all_work_pairs` error: a job's protocol state is not in the
    /// frontier. Here also when the SNARK pool's jobs are another block's.
    /// OCaml sends nothing then.
    Unavailable(String),
    /// No jobs (`Ok []`): OCaml sends the block alone.
    NoJobs,
    /// No Transition job ends at the block's ledger: OCaml sends the block
    /// alone.
    NoMatchingTransition,
    /// The job to prove: its pair in the SNARK pool and its place in it.
    Transition {
        pair: mina_core::snark::SnarkJobId,
        index: usize,
        /// A zkApp command, proved as segments rather than as one
        /// transaction.
        zkapp: bool,
    },
}

/// Control the single uptime subscription in state-machine action order.
/// Cancellation is scoped so an interrupted iteration cannot clear a new one.
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum RpcReportingProducedBlockWatch {
    Start { subscription: u64 },
    Cancel { subscription: u64 },
}

#[derive(Serialize, Debug, Clone)]
pub struct RpcReportingProducedBlock {
    pub block: ArcBlockWithHash,
    /// The waiter that owns this first completion.
    pub subscription: u64,
    /// Start of the block's won slot.
    pub slot_time: Timestamp,
}
pub type RpcActionStatsGetResponse = Option<ActionStatsResponse>;
pub type RpcSyncStatsGetResponse = Option<Vec<SyncStatsSnapshot>>;
pub type RpcVerificationStatsGetResponse = crate::verification::VerificationSnapshot;
pub type RpcBlockProducerStatsGetResponse = Option<RpcBlockProducerStats>;
pub type RpcPeersGetResponse = Vec<RpcPeerInfo>;
pub type RpcP2pConnectionOutgoingResponse = Result<(), String>;
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct RpcScanStateSummaryData {
    pub jobs: Vec<Vec<RpcScanStateSummaryScanStateJob>>,
    pub trees: Vec<(u64, u64, u64)>,
}

pub type RpcScanStateSummaryGetResponse = Result<RpcScanStateSummary, String>;
pub type RpcSnarkPoolGetResponse = Vec<RpcSnarkPoolJobSummary>;
pub type RpcSnarkPoolCompletedJobsResponse = Vec<TransactionSnarkWorkTStableV2>;
pub type RpcSnarkPoolPendingJobsGetResponse = Vec<JobState>;
pub type RpcSnarkPoolJobGetResponse = Option<RpcSnarkPoolJobFull>;
pub type RpcSnarkerConfigGetResponse = Option<RpcSnarkerConfig>;
pub type RpcTransactionPoolResponse = Vec<ValidCommandWithHash>;
pub type RpcLedgerSlimAccountsResponse = Vec<AccountSlim>;
pub type RpcLedgerAccountsResponse = Vec<Account>;
pub type RpcTransitionFrontierUserCommandsResponse = Vec<MinaBaseUserCommandStableV2>;
pub type RpcBestChainResponse = Vec<AppliedBlock>;
pub type RpcConsensusConstantsGetResponse = ConsensusConstants;
pub type RpcTransactionStatusGetResponse = TransactionStatus;

/// What the node knows of a sender's next nonce (OCaml's
/// `get_current_nonce` takes the rest from the ledger).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RpcTransactionNonceGetResponse {
    /// The node has no usable frontier: OCaml answers "Daemon is
    /// bootstrapping".
    pub bootstrapping: bool,
    /// The expected target nonce of the last command the pool holds from the
    /// sender, if it holds any.
    pub pool_nonce: Option<u32>,
}
pub type RpcPooledUserCommandsResponse = Vec<MinaBaseSignedCommandStableV2>;
pub type RpcPooledZkappCommandsResponse = Vec<MinaBaseZkappCommandTStableV1WireStableV1>;
pub type RpcGenesisBlockResponse = Option<ArcBlockWithHash>;
pub type RpcConsensusTimeGetResponse = Option<ConsensusTime>;
pub type RpcLedgerStatusGetResponse = Option<LedgerStatus>;
pub type RpcLedgerAccountDelegatorsGetResponse = Option<Vec<Account>>;

#[derive(Serialize, Deserialize, Debug, Clone, strum_macros::Display)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum TransactionStatus {
    Pending,
    Included,
    Unknown,
}

// TODO(adonagy): rework this to handle all the possible user commands (enum..)
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcTransactionInjectedPayment {
    pub amount: Amount,
    pub fee: Fee,
    // pub fee_token: TokenId,
    pub from: AccountPublicKey,
    pub to: AccountPublicKey,
    pub hash: String, // TODO(adonagy)
    // pub id: String, // TODO(adonagy)
    pub is_delegation: bool,
    pub memo: String, // TODO(adonagy)
    // pub memo: Memo, // TODO(adonagy)
    pub nonce: Nonce,
}

// TODO(adonagy): remove this, not needed anymore
// #[derive(Serialize, Deserialize, Debug, Clone)]
// pub enum RpcTransactionInjectedCommand {
//     Payment(valid::UserCommand),
//     Delegation(valid::UserCommand),
//     Zkapp(valid::UserCommand),
// }

pub type RpcTransactionInjectSuccess = Vec<valid::UserCommand>;
pub type RpcTransactionInjectRejected = Vec<(valid::UserCommand, diff::Error)>;
/// Errors
pub type RpcTransactionInjectFailure = Vec<String>;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum RpcTransactionInjectResponse {
    Success(RpcTransactionInjectSuccess),
    Rejected(RpcTransactionInjectRejected),
    Failure(RpcTransactionInjectFailure),
}

// impl From<ValidCommandWithHash> for RpcTransactionInjectedCommand {
//     fn from(value: ValidCommandWithHash) -> Self {
//         match value.data {
//             transaction_logic::valid::UserCommand::SignedCommand(ref signedcmd) => {
//                 match signedcmd.payload.body {
//                     transaction_logic::signed_command::Body::Payment(_) => {
//                         Self::Payment(value.data.clone())
//                     }
//                     transaction_logic::signed_command::Body::StakeDelegation(_) => {
//                         Self::Delegation(value.data.clone())
//                     }
//                 }
//             }
//             transaction_logic::valid::UserCommand::ZkAppCommand(_) => {
//                 Self::Zkapp(value.data.clone())
//             }
//         }
//     }
// }

// impl From<ValidCommandWithHash> for RpcTransactionInjectedCommand {
//     fn from(value: ValidCommandWithHash) -> Self {
//         match value.data {
//             transaction_logic::valid::UserCommand::SignedCommand(signedcmd) => {
//                 match signedcmd.payload.body {
//                     transaction_logic::signed_command::Body::Payment(ref payment) => {
//                         Self::RpcPayment(RpcTransactionInjectedPayment {
//                             amount: payment.amount,
//                             fee: signedcmd.fee(),
//                             // fee_token: signedcmd.fee_token(),
//                             from: signedcmd.fee_payer_pk().clone().into(),
//                             to: payment.receiver_pk.clone().into(),
//                             hash: value.hash.to_string(),
//                             is_delegation: false,
//                             // memo: signedcmd.payload.common.memo.clone(),
//                             memo: signedcmd.payload.common.memo.to_string(),
//                             nonce: signedcmd.nonce(),
//                         })
//                     }
//                     transaction_logic::signed_command::Body::StakeDelegation(_) => {
//                         todo!("inject stake delegation")
//                     }
//                 }
//             }
//             transaction_logic::valid::UserCommand::ZkAppCommand(_) => {
//                 Self::Zkapp(value.data.clone())
//             }
//         }
//     }
// }

#[derive(Serialize, Debug, Clone)]
pub struct AccountSlim {
    pub public_key: AccountPublicKey,
    pub balance: Balance,
    pub nonce: Nonce,
}

impl From<Account> for AccountSlim {
    fn from(value: Account) -> Self {
        Self {
            public_key: AccountPublicKey::from(value.public_key),
            balance: value.balance,
            nonce: value.nonce,
        }
    }
}

/// Comprehensive node status returned by the `/status` RPC endpoint.
///
/// Contains information about the node's current state including sync status,
/// peer connections, resource usage, and block production configuration.
#[derive(Serialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcNodeStatus {
    pub bootstrap: crate::bootstrap_status::BootstrapStatus,
    /// Configured network name; chain_id identifies the exact chain.
    pub network: String,
    /// Seconds since node state initialization.
    pub uptime_secs: u64,
    /// Hex-encoded network chain identifier.
    pub chain_id: Option<String>,
    /// Transition frontier state including best tip and sync status.
    pub transition_frontier: RpcNodeStatusTransitionFrontier,
    /// Ledger state and pending operations.
    pub ledger: RpcNodeStatusLedger,
    /// SNARK pool statistics.
    pub snark_pool: RpcNodeStatusSnarkPool,
    /// Transaction pool statistics.
    pub transaction_pool: RpcNodeStatusTransactionPool,
    /// Current block production attempt, if any.
    pub current_block_production_attempt: Option<BlockProductionAttempt>,
    /// Previous block production attempt, if any.
    pub previous_block_production_attempt: Option<BlockProductionAttempt>,
    /// Connected peer information.
    pub peers: Vec<RpcPeerInfo>,
    /// Resource usage statistics.
    pub resources_status: RpcNodeStatusResources,
    /// Service queue statistics.
    pub service_queues: Queues,
    /// Network configuration information.
    pub network_info: RpcNodeStatusNetworkInfo,
    /// Block producer public key, if configured.
    pub block_producer: Option<AccountPublicKey>,
    /// Coinbase receiver public key, if configured.
    pub coinbase_receiver: Option<AccountPublicKey>,
    /// When the block producer produces next (OCaml's
    /// `daemonStatus.nextBlockProduction`); `None` without a block producer
    /// or a best tip.
    pub next_block_production: Option<RpcNextBlockProduction>,
}

/// OCaml's `Next_producer_timing`, as GraphQL shows it.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/daemon_rpcs/types.ml#L175-L197>
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcNextBlockProduction {
    /// The best tip's slot, which the answer was worked out from.
    pub generated_from_consensus_at: RpcConsensusSlot,
    /// The won slot the producer produces in next, once the VRF evaluator
    /// has found one; `None` while it evaluates or when no slot is left in
    /// the evaluated epochs.
    pub next_won_slot: Option<RpcConsensusSlot>,
}

/// A slot in OCaml's `Consensus_time` terms, with its slot since genesis.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcConsensusSlot {
    pub epoch: u32,
    /// The slot within the epoch.
    pub slot: u32,
    /// The global slot since the hard fork.
    pub global_slot: u32,
    pub global_slot_since_genesis: u32,
    /// When the slot starts and ends, in milliseconds since the Unix epoch.
    pub start_time_ms: u64,
    pub end_time_ms: u64,
}

impl RpcConsensusSlot {
    /// OCaml's `Consensus_time.of_global_slot` of a global slot since the hard
    /// fork: its epoch and slot in it, and its start and end times, from the
    /// genesis timestamp and the slot duration.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/epoch.ml#L21-L41>
    pub fn new(
        global_slot: u32,
        global_slot_since_genesis: u32,
        slots_per_epoch: u32,
        genesis_timestamp_ms: u64,
        slot_duration_ms: u64,
    ) -> Option<Self> {
        let start_time_ms = u64::from(global_slot)
            .checked_mul(slot_duration_ms)?
            .checked_add(genesis_timestamp_ms)?;
        Some(Self {
            epoch: global_slot.checked_div(slots_per_epoch)?,
            slot: global_slot.checked_rem(slots_per_epoch)?,
            global_slot,
            global_slot_since_genesis,
            start_time_ms,
            end_time_ms: start_time_ms.checked_add(slot_duration_ms)?,
        })
    }
}

/// Network configuration information for the node.
#[derive(Serialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcNodeStatusNetworkInfo {
    /// IP address the node is bound to.
    pub bind_ip: String,
    /// External IP address, if known.
    pub external_ip: Option<String>,
    /// Client port for incoming connections.
    pub client_port: Option<u16>,
    /// libp2p port for P2P communication.
    pub libp2p_port: Option<u16>,
}

/// Ledger state and pending operations.
#[derive(Serialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcNodeStatusLedger {
    /// Number of alive ledger masks after the last commit.
    pub alive_masks_after_last_commit: usize,
    /// Pending ledger write operations with timestamps.
    pub pending_writes: Vec<(LedgerWriteKind, redux::Timestamp)>,
    /// Pending ledger read operations with IDs, kinds, and timestamps.
    pub pending_reads: Vec<(LedgerReadId, LedgerReadKind, redux::Timestamp)>,
}

/// Resource usage statistics for the node.
#[derive(Serialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcNodeStatusResources {
    /// Memory allocated for P2P operations in bytes.
    pub p2p_malloc_size: usize,
    /// Transition frontier resource usage.
    pub transition_frontier: serde_json::Value,
    /// SNARK pool resource usage.
    pub snark_pool: serde_json::Value,
    /// Reserved for compatibility; the opt-in diagnostic has its own route.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_debug: Option<serde_json::Value>,
}

/// Transition frontier state including best tip and sync information.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcNodeStatusTransitionFrontier {
    /// Last ledger fault and whether a fresh sync has committed successfully.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_recovery: Option<crate::transition_frontier::LedgerRecovery>,
    /// Current root of the applied best chain.
    #[serde(default)]
    pub root: Option<RpcNodeStatusTransitionFrontierBlockSummary>,
    /// Current best tip block summary.
    pub best_tip: Option<RpcNodeStatusTransitionFrontierBlockSummary>,
    /// Synchronization status.
    pub sync: RpcNodeStatusTransitionFrontierSync,
}

/// Synchronization status information.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcNodeStatusTransitionFrontierSync {
    /// Timestamp of the last sync update.
    pub time: Option<redux::Timestamp>,
    /// Current sync status (e.g., "Synced", "Bootstrap", "Catchup").
    pub status: String,
    /// Current sync phase.
    pub phase: String,
    /// Target block for synchronization, if syncing.
    pub target: Option<RpcNodeStatusTransitionFrontierBlockSummary>,
}

/// Summary of a block in the transition frontier.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcNodeStatusTransitionFrontierBlockSummary {
    /// Block timestamp as Unix milliseconds (not the last sync time).
    #[serde(default)]
    pub timestamp_ms: Option<u64>,
    /// State hash of the block.
    pub hash: StateHash,
    /// Block height.
    pub height: u32,
    /// Global slot number.
    pub global_slot: u32,
    /// The block's own timestamp, from its protocol state (nanoseconds since
    /// the Unix epoch), so that a client can show the block's age rather than
    /// the time of the node's last sync event.
    #[serde(default)]
    pub timestamp: Option<redux::Timestamp>,
}

/// Transaction pool statistics.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcNodeStatusTransactionPool {
    pub max_size: usize,
    pub minimum_fee_per_weight_unit: Option<String>,
    /// Total number of transactions in the pool.
    pub transactions: usize,
    /// Number of transactions ready for propagation.
    pub transactions_for_propagation: usize,
    /// Number of transaction candidates being processed.
    pub transaction_candidates: usize,
}

/// SNARK pool statistics.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcNodeStatusSnarkPool {
    /// Total number of SNARK jobs in the pool.
    pub total_jobs: usize,
    /// Number of completed SNARK proofs.
    pub snarks: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RpcBlockProducerStats {
    pub current_time: redux::Timestamp,
    pub current_global_slot: Option<u32>,
    pub current_epoch: Option<u32>,
    pub epoch_start: Option<u32>,
    pub epoch_end: Option<u32>,
    pub public_key: AccountPublicKey,
    pub attempts: Vec<BlockProductionAttempt>,
    pub future_won_slots: Vec<BlockProductionAttemptWonSlot>,
    pub current_epoch_vrf_stats: Option<VrfEvaluatorStats>,
    pub vrf_stats: BTreeMap<u32, VrfEvaluatorStats>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcSnarkerConfig {
    pub public_key: NonZeroCurvePoint,
    pub fee: CurrencyFeeStableV1,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcSnarkWorker {
    pub time: Option<Timestamp>,
    pub id: Option<String>,
    pub status: RpcSnarkWorkerStatus,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind")]
pub enum RpcSnarkWorkerStatus {
    None,
    Starting,
    Idle,
    Working {
        job_id: SnarkJobId,
        summary: JobSummary,
    },
    WorkReady {
        job_id: SnarkJobId,
    },
    WorkError {
        job_id: SnarkJobId,
        error: ExternalSnarkWorkerWorkError,
    },
    Cancelling {
        job_id: SnarkJobId,
    },
    Cancelled {
        job_id: SnarkJobId,
    },
    Error {
        error: ExternalSnarkWorkerError,
        permanent: bool,
    },
    Killing,
}

pub type RpcSnarkerWorkersResponse = Vec<RpcSnarkWorker>;
pub type RpcSnarkWorkerControlGetResponse = crate::external_snark_worker::RpcSnarkWorkerControl;
/// The worker after the change, or why there was none.
pub type RpcSnarkWorkerControlSetResponse =
    Result<crate::external_snark_worker::RpcSnarkWorkerControl, String>;
pub type RpcProofMemoryGetResponse = crate::proof_memory::ProofMemoryReport;
pub type RpcSnarkWorkIncludedGetResponse = crate::external_snark_worker::RpcIncludedWork;

/// Why a worker change is refused when the node doesn't allow it.
pub const WORKER_CONTROL_NOT_ALLOWED: &str =
    "worker control is off: start the node with --allow-worker-control (never with a public API)";

impl From<&MinaTransactionTransactionStableV2> for RpcScanStateSummaryBlockTransactionKind {
    fn from(value: &MinaTransactionTransactionStableV2) -> Self {
        match value {
            MinaTransactionTransactionStableV2::Command(v) => (&**v).into(),
            MinaTransactionTransactionStableV2::FeeTransfer(_) => Self::FeeTransfer,
            MinaTransactionTransactionStableV2::Coinbase(_) => Self::Coinbase,
        }
    }
}

impl From<&MinaBaseUserCommandStableV2> for RpcScanStateSummaryBlockTransactionKind {
    fn from(value: &MinaBaseUserCommandStableV2) -> Self {
        match value {
            MinaBaseUserCommandStableV2::SignedCommand(v) => match &v.payload.body {
                MinaBaseSignedCommandPayloadBodyStableV2::Payment(_) => Self::Payment,
                MinaBaseSignedCommandPayloadBodyStableV2::StakeDelegation(_) => {
                    Self::StakeDelegation
                }
            },
            MinaBaseUserCommandStableV2::ZkappCommand(_) => Self::Zkapp,
        }
    }
}

pub type RpcHealthCheckResponse = Result<(), String>;
pub type RpcReadinessCheckResponse = Result<(), String>;

pub type RpcDiscoveryRoutingTableResponse = Option<discovery::RpcDiscoveryRoutingTable>;
pub type RpcDiscoveryBoostrapStatsResponse = Option<P2pNetworkKadBootstrapStats>;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum GetBlockQuery {
    Hash(StateHash),
    Height(u32),
}

pub type RpcGetBlockResponse = Option<AppliedBlock>;
pub type RpcBestChainHeadersGetResponse = RpcBestChainHeaders;
pub type RpcSyncProgressGetResponse = RpcSyncProgress;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PooledCommandsQuery<ID> {
    pub public_key: Option<AccountPublicKey>,
    pub hashes: Option<Vec<TransactionHash>>,
    pub ids: Option<Vec<ID>>,
}

pub type PooledUserCommandsQuery = PooledCommandsQuery<MinaBaseSignedCommandStableV2>;
pub type PooledZkappsCommandsQuery = PooledCommandsQuery<MinaBaseZkappCommandTStableV1WireStableV1>;

pub mod discovery {
    use crate::p2p::{
        libp2p_identity::DecodingError, ConnectionType, P2pNetworkKadBucket, P2pNetworkKadDist,
        P2pNetworkKadEntry, P2pNetworkKadKey, P2pNetworkKadRoutingTable, PeerId,
    };
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct RpcDiscoveryRoutingTable {
        this_key: P2pNetworkKadKey,
        buckets: Vec<RpcKBucket>,
    }

    impl TryFrom<&P2pNetworkKadRoutingTable> for RpcDiscoveryRoutingTable {
        type Error = DecodingError;

        fn try_from(value: &P2pNetworkKadRoutingTable) -> Result<Self, Self::Error> {
            let mut buckets = Vec::new();

            for (i, b) in value.buckets.iter().enumerate() {
                buckets.push((b, P2pNetworkKadDist::from(i), &value.this_key).try_into()?);
            }

            Ok(RpcDiscoveryRoutingTable {
                this_key: value.this_key,
                buckets,
            })
        }
    }

    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct RpcKBucket {
        max_dist: P2pNetworkKadDist,
        entries: Vec<RpcEntry>,
    }

    impl<const K: usize>
        TryFrom<(
            &P2pNetworkKadBucket<K>,
            P2pNetworkKadDist,
            &P2pNetworkKadKey,
        )> for RpcKBucket
    {
        type Error = DecodingError;

        fn try_from(
            (bucket, max_dist, this_key): (
                &P2pNetworkKadBucket<K>,
                P2pNetworkKadDist,
                &P2pNetworkKadKey,
            ),
        ) -> Result<Self, Self::Error> {
            let mut entries = Vec::new();

            for entry in bucket.iter() {
                entries.push((entry, this_key).try_into()?);
            }
            Ok(RpcKBucket { max_dist, entries })
        }
    }

    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct RpcEntry {
        peer_id: PeerId,
        libp2p: crate::p2p::libp2p_identity::PeerId,
        key: P2pNetworkKadKey,
        dist: P2pNetworkKadDist,
        addrs: Vec<crate::p2p::multiaddr::Multiaddr>,
        connection: ConnectionType,
    }

    impl TryFrom<(&P2pNetworkKadEntry, &P2pNetworkKadKey)> for RpcEntry {
        type Error = DecodingError;

        fn try_from(
            (value, this_key): (&P2pNetworkKadEntry, &P2pNetworkKadKey),
        ) -> Result<Self, Self::Error> {
            Ok(RpcEntry {
                peer_id: value.peer_id,
                libp2p: value.peer_id.try_into()?,
                key: value.key,
                dist: this_key.distance(&value.key),
                addrs: value.addresses().clone(),
                connection: value.connection,
            })
        }
    }
}

#[cfg(test)]
mod consensus_slot_tests {
    use super::RpcConsensusSlot;

    /// The public OCaml devnet node's `daemonStatus` for its best tip at
    /// 2026-10-21 22:38 UTC (`consensusTimeBestTip` and
    /// `globalSlotSinceGenesisBestTip`; genesis timestamp
    /// 2026-08-19T10:00:00-08:00, 90 s slots, 7140 slots an epoch).
    #[test]
    fn consensus_slot_as_ocaml_devnet() {
        let genesis_ms = 1_787_162_400_000;
        let slot = RpcConsensusSlot::new(60660, 920220, 7140, genesis_ms, 90_000).unwrap();
        assert_eq!(
            slot,
            RpcConsensusSlot {
                epoch: 8,
                slot: 3540,
                global_slot: 60660,
                global_slot_since_genesis: 920220,
                start_time_ms: 1_792_621_800_000,
                end_time_ms: 1_792_621_890_000,
            }
        );
        assert_eq!(RpcConsensusSlot::new(1, 1, 0, genesis_ms, 90_000), None);
    }
}
