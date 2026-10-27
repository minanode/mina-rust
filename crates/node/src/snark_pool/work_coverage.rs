use std::collections::BTreeMap;

use ledger::scan_state::transaction_logic::valid;
use mina_core::{
    block::ArcBlockWithHash,
    snark::{Snark, SnarkJobId},
};
use mina_p2p_messages::v2;
use redux::Timestamp;
use serde::{Deserialize, Serialize};

/// Work is evaluated on the ledger worker; the node thread only snapshots
/// bounded pool inputs and serves this cached result. No proofs are verified
/// or transactions applied on the node thread for this diagnostic.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WorkCoverageRequest {
    pub time: Timestamp,
    pub parent: ArcBlockWithHash,
    pub slot: u32,
    pub coinbase_receiver: v2::NonZeroCurvePoint,
    pub receiver_source: String,
    pub completed_snarks: BTreeMap<SnarkJobId, Snark>,
    pub transactions: Vec<valid::UserCommand>,
}

impl PartialEq for WorkCoverageRequest {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WorkCoverage {
    pub sampled_at: Timestamp,
    pub parent_hash: v2::StateHash,
    pub parent_height: u32,
    pub slot_since_genesis: u32,
    pub coinbase_receiver: v2::NonZeroCurvePoint,
    pub receiver_source: String,
    /// Work bundles (each contains one or two proofs), in scan-state order.
    pub jobs_needed: usize,
    pub jobs_available: usize,
    pub available_prefix: usize,
    pub first_missing_job: Option<SnarkJobId>,
    pub transactions_considered: usize,
    pub expected_transactions: usize,
    pub expected_works: usize,
    pub expected_coinbase_nanomina: u64,
    pub coinbase_can_be_included: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WorkCoverageStatus {
    pub snapshot: Option<WorkCoverage>,
    pub error: Option<String>,
    pub pending: bool,
    /// Computed at read time; false if the chain advanced since the sample.
    pub matches_current_tip: bool,
    pub fetch_requests: u64,
    pub fetched_works: u64,
}

pub const COVERAGE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
pub const MAX_COVERAGE_WORKS: usize = 1024;
pub const MAX_COVERAGE_TRANSACTIONS: usize = 256;

pub fn coverage_counts<T>(jobs: &[T], available: impl Fn(&T) -> bool) -> (usize, usize) {
    let mut count = 0;
    let mut prefix = 0;
    for (index, job) in jobs.iter().enumerate() {
        if available(job) {
            count += 1;
            if index == prefix {
                prefix += 1;
            }
        }
    }
    (count, prefix)
}
