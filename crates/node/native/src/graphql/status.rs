use super::{api::GraphQLObject, scalars::MinaScalarValue};
use mina_node::bootstrap_status as node;
type StatusInt = super::scalars::CommandNonceScalar;
pub fn int(n: u64) -> StatusInt {
    super::scalars::CommandNonceScalar(node::count(n))
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/mina_graphql/types.ml>
#[derive(Debug, Clone, GraphQLObject)]
#[graphql(scalar = MinaScalarValue, description = "How far a ledger sync has progressed")]
pub struct LedgerSyncProgress {
    #[graphql(description = "How many times the sync has been given a new root to reach")]
    pub targets: StatusInt,
    #[graphql(description = "When the current target was set")]
    pub target_set_at: Option<Time>,
    #[graphql(description = "How many inner nodes the sync is waiting for the children of")]
    pub pending_child_hashes: StatusInt,
    #[graphql(description = "How many account subtrees the sync is waiting for the accounts of")]
    pub pending_contents: StatusInt,
    #[graphql(description = "Whether the ledger has reached the current target")]
    pub synced: bool,
    #[graphql(
        description = "How many answers from peers have added to the ledger or confirmed part of it"
    )]
    pub answers_accepted: StatusInt,
    #[graphql(
        description = "How many answers have arrived for queries the sync no longer needed, such as those for an earlier target"
    )]
    pub answers_ignored: StatusInt,
    #[graphql(description = "How many answers have been invalid, and their queries asked again")]
    pub answers_rejected: StatusInt,
    #[graphql(description = "When an answer was last accepted")]
    pub last_accepted_answer_at: Option<Time>,
    #[graphql(description = "How many accounts the current target holds, once a peer has said")]
    pub num_accounts: Option<StatusInt>,
    #[graphql(
        description = "How many of the current target's accounts the ledger is known to hold, whether received or already present. A sync whose ledger already matched its target can finish before any are counted, so a synced ledger should be read as complete."
    )]
    pub accounts_settled: StatusInt,
    #[graphql(description = "How many accounts peers have sent, over all targets")]
    pub accounts_received: StatusInt,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/mina_graphql/types.ml>
#[derive(Debug, Clone, GraphQLObject)]
#[graphql(scalar = MinaScalarValue, description = "The most recent sync of a ledger from peers")]
pub struct LedgerSync {
    pub ledger: SyncedLedger,
    #[graphql(description = "The root the sync is reaching")]
    pub target: Option<LedgerHash>,
    #[graphql(description = "Whether the sync is still running")]
    pub active: bool,
    #[graphql(description = "For a sync that has ended, its progress when it ended")]
    pub progress: LedgerSyncProgress,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/mina_graphql/types.ml>
#[derive(Debug, Clone, GraphQLObject)]
#[graphql(scalar = MinaScalarValue, description = "What a node is doing to reach the network's chain")]
pub struct BootstrapStatus {
    pub phase: BootstrapPhase,
    pub phase_started_at: Time,
    #[graphql(description = "When the current or most recent bootstrap started")]
    pub bootstrap_started_at: Option<Time>,
    #[graphql(description = "How many attempts the current or most recent bootstrap has started")]
    pub bootstrap_attempts: StatusInt,
    #[graphql(description = "Why that bootstrap's last failed attempt failed")]
    pub last_attempt_failure: Option<String>,
    #[graphql(description = "The most recent sync of each ledger")]
    pub ledger_syncs: Vec<LedgerSync>,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/mina_graphql/types.ml>
#[derive(Debug, Clone, GraphQLObject)]
#[graphql(scalar = MinaScalarValue, description = "Occupancy of the transaction pool. When the pool is full, admitting a new command evicts the command with the lowest fee per weight unit, together with every later-nonce command from the same fee payer")]
pub struct TransactionPoolStatus {
    #[graphql(description = "Number of commands currently in the pool")]
    pub size: StatusInt,
    #[graphql(
        description = "Number of commands the pool holds before it starts evicting commands"
    )]
    pub max_size: StatusInt,
    #[graphql(
        description = "The lowest fee per weight unit of any command in the pool, in nanomina, as a decimal integer or a fraction 'n/d'. Null if the pool is empty. Once the pool is full, a new command must pay strictly more than this to be admitted. Payments and stake delegations have weight 1"
    )]
    pub minimum_fee_per_weight_unit: Option<String>,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/mina_graphql/types.ml>
#[derive(Debug, Clone, GraphQLObject)]
#[graphql(scalar = MinaScalarValue, description = "Free space and outstanding snark work in one tree of the scan state")]
pub struct ScanStateTree {
    #[graphql(description = "Number of transaction slots still free in this tree")]
    pub available_space: StatusInt,
    #[graphql(description = "Number of base jobs in this tree that are yet to be done")]
    pub base_jobs_todo: StatusInt,
    #[graphql(description = "Number of merge jobs in this tree that are yet to be done")]
    pub merge_jobs_todo: StatusInt,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/mina_graphql/types.ml>
#[derive(Debug, Clone, GraphQLObject)]
#[graphql(scalar = MinaScalarValue, description = "Status of the scan state as of a block")]
pub struct ScanState {
    #[graphql(description = "State hash of the block whose scan state this is")]
    pub state_hash: StateHash,
    #[graphql(description = "Status of each tree in the scan state, oldest first")]
    pub trees: Vec<ScanStateTree>,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/mina_graphql/types.ml#L345-L400>
#[derive(Debug, Clone, super::api::GraphQLEnum)]
#[graphql(scalar = MinaScalarValue, description = "The step a node is in on its way to reaching the network's chain")]
pub enum BootstrapPhase {
    #[graphql(description = "Initialisation has not begun")]
    Starting,
    #[graphql(description = "The chain has not started yet")]
    WaitingForGenesis,
    #[graphql(description = "Connecting to enough peers to begin")]
    WaitingForPeers,
    #[graphql(
        description = "Asking peers for their best tip, while loading the transition frontier from disk"
    )]
    FetchingBestTip,
    #[graphql(description = "Bootstrap: syncing the root's snarked ledger")]
    SyncingRootLedger,
    #[graphql(description = "Bootstrap: downloading the root's scan state and pending coinbases")]
    DownloadingScanState,
    #[graphql(
        description = "Bootstrap: building the root's staged ledger, which checks the scan state against the snarked ledger"
    )]
    BuildingStagedLedger,
    #[graphql(description = "Syncing the staking and next epoch ledgers")]
    SyncingEpochLedgers,
    #[graphql(description = "Bootstrap: loading a transition frontier at the new root")]
    LoadingFrontier,
    #[graphql(description = "The transition frontier is running")]
    Participating,
}
impl From<node::BootstrapPhase> for BootstrapPhase {
    fn from(value: node::BootstrapPhase) -> Self {
        match value {
            node::BootstrapPhase::Starting => Self::Starting,
            node::BootstrapPhase::WaitingForGenesis => Self::WaitingForGenesis,
            node::BootstrapPhase::WaitingForPeers => Self::WaitingForPeers,
            node::BootstrapPhase::FetchingBestTip => Self::FetchingBestTip,
            node::BootstrapPhase::SyncingRootLedger => Self::SyncingRootLedger,
            node::BootstrapPhase::DownloadingScanState => Self::DownloadingScanState,
            node::BootstrapPhase::BuildingStagedLedger => Self::BuildingStagedLedger,
            node::BootstrapPhase::SyncingEpochLedgers => Self::SyncingEpochLedgers,
            node::BootstrapPhase::LoadingFrontier => Self::LoadingFrontier,
            node::BootstrapPhase::Participating => Self::Participating,
        }
    }
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/mina_graphql/types.ml#L345-L400>
#[derive(Debug, Clone, super::api::GraphQLEnum)]
#[graphql(scalar = MinaScalarValue, description = "A ledger that a node syncs from its peers")]
pub enum SyncedLedger {
    #[graphql(description = "The snarked ledger at the root of the transition frontier")]
    RootSnarked,
    #[graphql(description = "The staking epoch ledger")]
    StakingEpoch,
    #[graphql(description = "The next epoch ledger")]
    NextEpoch,
}
impl From<node::SyncedLedger> for SyncedLedger {
    fn from(value: node::SyncedLedger) -> Self {
        match value {
            node::SyncedLedger::RootSnarked => Self::RootSnarked,
            node::SyncedLedger::StakingEpoch => Self::StakingEpoch,
            node::SyncedLedger::NextEpoch => Self::NextEpoch,
        }
    }
}
impl From<node::LedgerSyncProgress> for LedgerSyncProgress {
    fn from(value: node::LedgerSyncProgress) -> Self {
        Self {
            targets: super::scalars::CommandNonceScalar(value.targets),
            target_set_at: value.target_set_at.map(Into::into),
            pending_child_hashes: super::scalars::CommandNonceScalar(value.pending_child_hashes),
            pending_contents: super::scalars::CommandNonceScalar(value.pending_contents),
            synced: value.synced,
            answers_accepted: super::scalars::CommandNonceScalar(value.answers_accepted),
            answers_ignored: super::scalars::CommandNonceScalar(value.answers_ignored),
            answers_rejected: super::scalars::CommandNonceScalar(value.answers_rejected),
            last_accepted_answer_at: value.last_accepted_answer_at.map(Into::into),
            num_accounts: value.num_accounts.map(super::scalars::CommandNonceScalar),
            accounts_settled: super::scalars::CommandNonceScalar(value.accounts_settled),
            accounts_received: super::scalars::CommandNonceScalar(value.accounts_received),
        }
    }
}
impl From<node::LedgerSync> for LedgerSync {
    fn from(value: node::LedgerSync) -> Self {
        Self {
            ledger: value.ledger.into(),
            target: value.target.map(Into::into),
            active: value.active,
            progress: value.progress.into(),
        }
    }
}
impl From<node::BootstrapStatus> for BootstrapStatus {
    fn from(value: node::BootstrapStatus) -> Self {
        Self {
            phase: value.phase.into(),
            phase_started_at: value.phase_started_at.into(),
            bootstrap_started_at: value.bootstrap_started_at.map(Into::into),
            bootstrap_attempts: super::scalars::CommandNonceScalar(value.bootstrap_attempts),
            last_attempt_failure: value.last_attempt_failure,
            ledger_syncs: value.ledger_syncs.into_iter().map(Into::into).collect(),
        }
    }
}

// OCaml Time has no schema description.
// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/graphql_basic_scalars/graphql_basic_scalars.ml#L105-L114>
#[derive(Debug, Clone, super::api::GraphQLScalar)]
#[graphql(name = "Time", transparent, scalar = MinaScalarValue)]
pub struct Time(pub String);
impl From<u64> for Time {
    fn from(ns: u64) -> Self {
        let t = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ns))
            .expect("u64 nanoseconds fit Time");
        Self(format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}Z",
            t.year(),
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute(),
            t.second(),
            t.microsecond()
        ))
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/graphql_lib/mina_base/graphql_scalars.ml#L18-L26>
#[derive(Debug, Clone, super::api::GraphQLScalar)]
#[graphql(transparent, description = "Base58Check-encoded state hash", scalar = MinaScalarValue)]
pub struct StateHash(pub String);
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/graphql_lib/mina_base/graphql_scalars.ml#L48-L56>
#[derive(Debug, Clone, super::api::GraphQLScalar)]
#[graphql(transparent, description = "Base58Check-encoded ledger hash", scalar = MinaScalarValue)]
pub struct LedgerHash(pub String);
impl From<String> for LedgerHash {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[derive(Debug, Clone, GraphQLObject)]
#[graphql(scalar = MinaScalarValue)]
pub struct Applied {
    pub applied: bool,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/mina_graphql/types.ml#L3011-L3023>
#[derive(Debug, Clone, super::api::GraphQLScalar)]
#[graphql(transparent, scalar = MinaScalarValue, to_output_with = VersionedPrecomputedBlock::to_output, description = "Block encoded in precomputed block format, of any version the archive reads")]
pub struct VersionedPrecomputedBlock(pub ArchiveJson);
impl VersionedPrecomputedBlock {
    fn to_output(&self) -> MinaScalarValue {
        juniper::ToScalarValue::to_scalar_value(&self.0)
    }
}

// Juniper's usual scalar derive accepts only scalar JSON. OCaml deliberately
// accepts any JSON here, including lists and objects, without decoding blocks.
#[derive(Debug, Clone)]
pub struct ArchiveJson(super::api::InputValue);
impl juniper::FromInputValue<MinaScalarValue> for ArchiveJson {
    type Error = String;
    fn from_input_value(v: &super::api::InputValue) -> Result<Self, String> {
        Ok(Self(v.clone()))
    }
}
impl juniper::ToInputValue<MinaScalarValue> for ArchiveJson {
    fn to_input_value(&self) -> super::api::InputValue {
        self.0.clone()
    }
}
impl<'a> juniper::FromScalarValue<'a, MinaScalarValue> for ArchiveJson {
    type Error = String;
    fn from_scalar_value(v: &'a MinaScalarValue) -> Result<Self, String> {
        Ok(Self(super::api::InputValue::Scalar(v.clone())))
    }
}
impl juniper::ToScalarValue<MinaScalarValue> for ArchiveJson {
    fn to_scalar_value(&self) -> MinaScalarValue {
        // Input-only scalar. This implementation is required by Juniper.
        MinaScalarValue::String(self.0.to_string())
    }
}
impl juniper::ParseScalarValue<MinaScalarValue> for ArchiveJson {
    fn from_str(token: juniper::ScalarToken<'_>) -> juniper::ParseScalarResult<MinaScalarValue> {
        match token {
            juniper::ScalarToken::String(_) => {
                <String as juniper::ParseScalarValue<MinaScalarValue>>::from_str(token)
            }
            juniper::ScalarToken::Int(_) => {
                <i32 as juniper::ParseScalarValue<MinaScalarValue>>::from_str(token)
            }
            _ => <f64 as juniper::ParseScalarValue<MinaScalarValue>>::from_str(token),
        }
    }
}
