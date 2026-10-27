//! Bounded, observational receipts. These never enable a consensus action and
//! never cause proof work. A passed block proof authenticates the protocol state,
//! not every body/ancestry block fetched while bootstrapping.
use std::collections::{BTreeMap, VecDeque};

use mina_core::block::{prevalidate::BlockPrevalidationError, ArcBlockWithHash, BlockHeader};
use mina_p2p_messages::v2::StateHash;
use redux::Timestamp;
use serde::{Deserialize, Serialize};

pub const RECEIPT_CAPACITY: usize = 1024;
const FAILURE_CAPACITY: usize = 128;
pub const DEFAULT_RECEIPT_PAGE_SIZE: usize = 64;
pub const MAX_RECEIPT_PAGE_SIZE: usize = 128;

/// Newest first, by first observation. A zero limit requests only the summary.
///
/// With `hashes`, receipts are looked up by state hash instead: at most
/// [`MAX_RECEIPT_PAGE_SIZE`] hashes, answered in the order given, and `limit`
/// and `offset` are ignored. A hash without a receipt has none retained: this
/// process never recorded the block, or [`RECEIPT_CAPACITY`] newer blocks
/// evicted it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VerificationQuery {
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: usize,
    // Always written: the recorder stores this query with postcard, which has
    // no field names, so a skipped field would misalign decoding.
    #[serde(default)]
    pub hashes: Vec<StateHash>,
}
impl VerificationQuery {
    pub fn summary() -> Self {
        Self {
            limit: Some(0),
            ..Default::default()
        }
    }

    pub fn page(limit: Option<usize>, offset: usize) -> Self {
        Self {
            limit,
            offset,
            hashes: Vec::new(),
        }
    }

    pub fn by_hash(hashes: Vec<StateHash>) -> Self {
        Self {
            hashes,
            ..Default::default()
        }
    }
}

/// Present only in an answer to a lookup by hash, so that a client can tell
/// it from an older node's default page, which ignores `hashes`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptLookup {
    /// Distinct hashes looked up, after the [`MAX_RECEIPT_PAGE_SIZE`] bound.
    pub requested: usize,
    /// Of those, the ones with a retained receipt: all of `receipts`.
    pub found: usize,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CheckStatus {
    #[default]
    NotRun,
    Pending,
    Passed,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckReceipt {
    pub status: CheckStatus,
    pub checked_at: Option<u64>,
    pub elapsed_ms: Option<f64>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Arrival {
    Gossip,
    Catchup,
    Bootstrap,
    /// Best-tip channel merges requested and pushed tips (including WebRTC).
    BestTip,
    Local,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockIdentity {
    pub state_hash: StateHash,
    pub height: u32,
}
impl BlockIdentity {
    pub fn new(block: &ArcBlockWithHash) -> Self {
        Self::header(block.hash().clone(), block.header())
    }
    pub fn header(state_hash: StateHash, header: &BlockHeader) -> Self {
        Self {
            state_hash,
            height: header
                .protocol_state
                .body
                .consensus_state
                .blockchain_length
                .as_u32(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationReceipt {
    pub block: BlockIdentity,
    pub network: String,
    pub arrival: Arrival,
    pub received_at: u64,
    pub applied: bool,
    /// Whether a BlockApply success was counted (bootstrap restoration is not one).
    #[serde(default)]
    pub applied_counted: bool,
    pub block_proof: CheckReceipt,
    pub delta_chain: CheckReceipt,
    pub body_reference: CheckReceipt,
    /// SHA-256 of the embedded network-specific blockchain verifier index JSON.
    pub verifier_key: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum VerificationUpdate {
    Body {
        block: BlockIdentity,
        passed: bool,
        elapsed_ms: f64,
    },
    Headers {
        block: BlockIdentity,
        arrival: Arrival,
        delta: CheckStatus,
        body: CheckStatus,
    },
    Proof {
        block: BlockIdentity,
        status: CheckStatus,
        elapsed_ms: Option<f64>,
        verifier_key: Option<String>,
    },
}

impl VerificationUpdate {
    /// Reflects the existing short-circuit order in prevalidate_block. No check
    /// is re-run, and an earlier error cannot imply that a later check passed.
    pub fn headers(
        block: &ArcBlockWithHash,
        arrival: Arrival,
        result: &Result<(), BlockPrevalidationError>,
    ) -> Self {
        use BlockPrevalidationError::*;
        use CheckStatus::*;
        let (delta, body) = match result {
            Ok(()) => (Passed, Passed),
            Err(InvalidDeltaBlockChainProof) => (Failed, NotRun),
            Err(InvalidBodyReference { .. }) => (Passed, Failed),
            Err(BodyProofUnavailable { .. }) => (Passed, NotRun),
            Err(InvalidProtocolVersion | MismatchedProtocolVersion | ConsantsMismatch) => {
                (Passed, NotRun)
            }
            Err(_) => (NotRun, NotRun),
        };
        Self::Headers {
            block: BlockIdentity::new(block),
            arrival,
            delta,
            body,
        }
    }
    pub fn action(self) -> crate::TransitionFrontierAction {
        crate::TransitionFrontierAction::VerificationRecorded {
            update: Box::new(self),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationFailure {
    pub block: BlockIdentity,
    pub check: String,
    pub checked_at: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VerificationState {
    records: BTreeMap<StateHash, VerificationReceipt>,
    order: VecDeque<StateHash>,
    failures: VecDeque<VerificationFailure>,
    proof_times: VecDeque<f64>,
    // Maintained on successful proof arrival, never sorted by readers.
    sorted_proof_times: Vec<f64>,
    verified: u64,
    failed: u64,
    #[serde(default)]
    proof_failed: u64,
    #[serde(default)]
    applied: u64,
    #[serde(default)]
    proof_time_count: u64,
    #[serde(default)]
    proof_time_sum_seconds: f64,
    #[serde(default)]
    proof_time_buckets: [u64; 8],
    last_verified: Option<VerificationReceipt>,
    pub has_synced: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationSnapshot {
    pub capacity: usize,
    pub verified_since_start: u64,
    pub failures_since_start: u64,
    #[serde(default)]
    pub proof_failures_since_start: u64,
    #[serde(default)]
    pub applied_since_start: u64,
    #[serde(default)]
    pub proof_time_count: u64,
    #[serde(default)]
    pub proof_time_sum_seconds: f64,
    /// Cumulative counts at 0.05, 0.1, 0.25, 0.5, 1, 2, 5 and 10 seconds.
    #[serde(default)]
    pub proof_time_buckets: [u64; 8],
    pub recent_failures: Vec<VerificationFailure>,
    pub recent_failure_count: usize,
    pub median_proof_time_ms: Option<f64>,
    pub last_verified: Option<VerificationReceipt>,
    pub receipts: Vec<VerificationReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lookup: Option<ReceiptLookup>,
}

impl VerificationState {
    pub fn fetch_arrival(&self) -> Arrival {
        if self.has_synced {
            Arrival::Catchup
        } else {
            Arrival::Bootstrap
        }
    }

    fn entry(
        &mut self,
        block: BlockIdentity,
        arrival: Arrival,
        now: u64,
    ) -> &mut VerificationReceipt {
        let hash = block.state_hash.clone();
        if !self.records.contains_key(&hash) {
            if self.order.len() == RECEIPT_CAPACITY {
                if let Some(old) = self.order.pop_front() {
                    self.records.remove(&old);
                }
            }
            self.order.push_back(hash.clone());
        }
        self.records
            .entry(hash)
            .or_insert_with(|| VerificationReceipt {
                block,
                network: mina_core::NetworkConfig::global().name.to_owned(),
                arrival,
                received_at: now,
                applied: false,
                applied_counted: false,
                block_proof: Default::default(),
                delta_chain: Default::default(),
                body_reference: Default::default(),
                verifier_key: None,
            })
    }

    fn failure(&mut self, block: &BlockIdentity, check: &str, now: u64) {
        self.failed = self.failed.saturating_add(1);
        if self.failures.len() == FAILURE_CAPACITY {
            self.failures.pop_front();
        }
        self.failures.push_back(VerificationFailure {
            block: block.clone(),
            check: check.to_owned(),
            checked_at: now,
        });
    }

    pub fn record(&mut self, update: &VerificationUpdate, time: Timestamp) {
        let now = (u64::from(time) / 1_000_000);
        match update {
            VerificationUpdate::Body {
                block,
                passed,
                elapsed_ms,
            } => {
                if !passed {
                    self.failure(block, "bodyReference", now);
                }
                let entry = self.entry(block.clone(), Arrival::Local, now);
                if *passed || entry.body_reference.status != CheckStatus::Passed {
                    entry.body_reference = CheckReceipt {
                        status: if *passed {
                            CheckStatus::Passed
                        } else {
                            CheckStatus::Failed
                        },
                        checked_at: Some(now),
                        elapsed_ms: Some(*elapsed_ms),
                    };
                }
            }
            VerificationUpdate::Headers {
                block,
                arrival,
                delta,
                body,
            } => {
                if *delta == CheckStatus::Failed {
                    self.failure(block, "deltaChain", now);
                }
                if *body == CheckStatus::Failed {
                    self.failure(block, "bodyReference", now);
                }
                let entry = self.entry(block.clone(), *arrival, now);
                // A rejected peer payload with the same protocol-state hash must
                // not erase the evidence for the accepted payload. Failures live
                // separately above; no receipt ever promotes a failed check.
                for (check, status) in [
                    (&mut entry.delta_chain, delta),
                    (&mut entry.body_reference, body),
                ] {
                    if *status != CheckStatus::NotRun && check.status != CheckStatus::Passed {
                        *check = CheckReceipt {
                            status: *status,
                            checked_at: Some(now),
                            elapsed_ms: None,
                        };
                    }
                }
            }
            VerificationUpdate::Proof {
                block,
                status,
                elapsed_ms,
                verifier_key,
            } => {
                if *status == CheckStatus::Failed {
                    self.proof_failed = self.proof_failed.saturating_add(1);
                    self.failure(block, "blockProof", now);
                }
                let entry = self.entry(block.clone(), Arrival::BestTip, now);
                if entry.block_proof.status == CheckStatus::Passed {
                    return;
                }
                entry.block_proof = CheckReceipt {
                    status: *status,
                    checked_at: Some(now),
                    elapsed_ms: *elapsed_ms,
                };
                entry.verifier_key = verifier_key.clone();
                if *status == CheckStatus::Passed {
                    self.last_verified = Some(entry.clone());
                    self.verified = self.verified.saturating_add(1);
                    if let Some(ms) = elapsed_ms {
                        if ms.is_finite() && *ms >= 0.0 {
                            let seconds = *ms / 1000.0;
                            self.proof_time_count = self.proof_time_count.saturating_add(1);
                            self.proof_time_sum_seconds += seconds;
                            for (count, bound) in self
                                .proof_time_buckets
                                .iter_mut()
                                .zip([0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0])
                            {
                                if seconds <= bound {
                                    *count = count.saturating_add(1);
                                }
                            }
                        }
                        if self.proof_times.len() == RECEIPT_CAPACITY {
                            if let Some(old) = self.proof_times.pop_front() {
                                let index = self
                                    .sorted_proof_times
                                    .binary_search_by(|v| v.total_cmp(&old))
                                    .unwrap();
                                self.sorted_proof_times.remove(index);
                            }
                        }
                        self.proof_times.push_back(*ms);
                        let index = self
                            .sorted_proof_times
                            .partition_point(|v| v.total_cmp(ms).is_lt());
                        self.sorted_proof_times.insert(index, *ms);
                    }
                }
            }
        }
    }

    /// Returns true only for the first successful BlockApply of this retained block.
    pub fn applied(
        &mut self,
        block: &ArcBlockWithHash,
        time: Timestamp,
        body_checked: bool,
    ) -> bool {
        let newly_applied = body_checked
            && !self
                .records
                .get(block.hash())
                .is_some_and(|entry| entry.applied_counted);
        if newly_applied {
            self.applied = self.applied.saturating_add(1);
        }
        let arrival = if block.is_genesis() {
            Arrival::Local
        } else {
            self.fetch_arrival()
        };
        let entry = self.entry(
            BlockIdentity::new(block),
            arrival,
            (u64::from(time) / 1_000_000),
        );
        entry.applied = true;
        entry.applied_counted |= body_checked;
        // Success from block_apply implies its mandatory body check ran. A
        // reconstructed bootstrap root does not pass through block_apply.
        if body_checked && entry.body_reference.status != CheckStatus::Passed {
            entry.body_reference = CheckReceipt {
                status: CheckStatus::Passed,
                checked_at: Some((u64::from(time) / 1_000_000)),
                elapsed_ms: None,
            };
        }
        let body_reference = entry.body_reference.clone();
        if let Some(last) = self
            .last_verified
            .as_mut()
            .filter(|v| &v.block.state_hash == block.hash())
        {
            last.applied = true;
            last.applied_counted |= body_checked;
            if body_checked {
                last.body_reference = body_reference;
            }
        }
        newly_applied
    }

    /// Constant-time summary: no receipt/failure traversal or timing allocation/sort.
    /// Only the single last-verified receipt is copied.
    pub fn summary(&self) -> VerificationSnapshot {
        let times = &self.sorted_proof_times;
        let median = if times.is_empty() {
            None
        } else {
            let n = times.len();
            Some((times[(n - 1) / 2] + times[n / 2]) / 2.0)
        };
        VerificationSnapshot {
            capacity: RECEIPT_CAPACITY,
            verified_since_start: self.verified,
            failures_since_start: self.failed,
            proof_failures_since_start: self.proof_failed,
            applied_since_start: self.applied,
            proof_time_count: self.proof_time_count,
            proof_time_sum_seconds: self.proof_time_sum_seconds,
            proof_time_buckets: self.proof_time_buckets,
            recent_failures: Vec::new(),
            recent_failure_count: self.failures.len(),
            median_proof_time_ms: median,
            last_verified: self.last_verified.clone(),
            receipts: Vec::new(),
            lookup: None,
        }
    }

    pub fn snapshot(&self, query: VerificationQuery) -> VerificationSnapshot {
        let mut snapshot = self.summary();
        if !query.hashes.is_empty() {
            // O(log n) per hash, each distinct hash answered once.
            let mut seen = std::collections::BTreeSet::new();
            let requested: Vec<&StateHash> = query
                .hashes
                .iter()
                .take(MAX_RECEIPT_PAGE_SIZE)
                .filter(|hash| seen.insert(*hash))
                .collect();
            snapshot.receipts = requested
                .iter()
                .filter_map(|hash| self.records.get(*hash).cloned())
                .collect();
            snapshot.lookup = Some(ReceiptLookup {
                requested: requested.len(),
                found: snapshot.receipts.len(),
            });
            return snapshot;
        }
        let limit = query
            .limit
            .unwrap_or(DEFAULT_RECEIPT_PAGE_SIZE)
            .min(MAX_RECEIPT_PAGE_SIZE);
        if limit > 0 {
            snapshot.recent_failures = self.failures.iter().rev().take(limit).cloned().collect();
            snapshot.receipts = self
                .order
                .iter()
                .rev()
                .skip(query.offset)
                .take(limit)
                .filter_map(|hash| self.records.get(hash).cloned())
                .collect();
        }
        snapshot
    }
}

#[cfg(test)]
mod tests;
