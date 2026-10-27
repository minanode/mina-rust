use std::{fmt, ops::RangeBounds, time::Duration};

use ledger::scan_state::scan_state::{transaction_snark::OneOrTwo, AvailableJobMessage};
use mina_core::snark::{Snark, SnarkInfo, SnarkJobCommitment, SnarkJobId};
use mina_p2p_messages::v2;
use redux::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{core::distributed_pool::DistributedPool, p2p::PeerId};

use super::{candidate::SnarkPoolCandidatesState, SnarkPoolConfig};

#[derive(Serialize, Deserialize, Clone)]
pub struct SnarkPoolState {
    #[serde(default)]
    pub admission_stats: super::AdmissionStats,
    #[serde(default)]
    pub(super) account_request_id: u64,
    #[serde(default)]
    pub(super) jobs_tip: Option<v2::StateHash>,
    #[serde(default)]
    pub(super) prover_accounts: std::collections::BTreeMap<
        crate::account::AccountPublicKey,
        (v2::StateHash, super::ProverAccount),
    >,
    config: SnarkPoolConfig,
    pool: DistributedPool<JobState, SnarkJobId>,
    pub candidates: SnarkPoolCandidatesState,
    #[serde(default)]
    pub completed_snarks_received: u64,
    #[serde(default)]
    pub work_coverage: super::WorkCoverageStatus,
    #[serde(default)]
    pub coverage_last_attempt: Option<Timestamp>,
    #[serde(default)]
    pub(super) zkapp_jobs: usize,
    pub(super) last_check_timeouts: Timestamp,
    /// The worker found no job it may take because a block of ours is near
    /// (P5.3, `State::snark_worker_may_take_job`); it looks again once it
    /// may.
    #[serde(default)]
    pub worker_paused_for_block: bool,
    /// The worker key and fee (nanomina) whose proofs the pool last refused
    /// by OCaml's prover rule: warned once per key and fee (RC7-FIXES).
    #[serde(default)]
    pub(super) own_fee_refused: Option<(crate::account::AccountPublicKey, u64)>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JobState {
    pub time: Timestamp,
    pub id: SnarkJobId,
    pub job: OneOrTwo<AvailableJobMessage>,
    pub commitment: Option<JobCommitment>,
    pub snark: Option<SnarkWork>,
    /// Lower order has higher priority to be done as it represents older job.
    pub order: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JobCommitment {
    pub commitment: SnarkJobCommitment,
    pub received_t: Timestamp,
    pub sender: PeerId,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SnarkWork {
    pub work: Snark,
    pub received_t: Timestamp,
    pub sender: PeerId,
}

/// Whether the job is a merge proof job, or a transaction proof job, with particular number of account updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum JobSummary {
    Tx(usize),
    Merge(usize),
}

impl Default for SnarkPoolState {
    fn default() -> Self {
        Self::new()
    }
}

impl SnarkPoolState {
    pub fn new() -> Self {
        Self {
            config: SnarkPoolConfig {},
            admission_stats: Default::default(),
            account_request_id: 0,
            jobs_tip: None,
            prover_accounts: Default::default(),
            pool: Default::default(),
            candidates: SnarkPoolCandidatesState::new(),
            completed_snarks_received: 0,
            work_coverage: Default::default(),
            coverage_last_attempt: None,
            zkapp_jobs: 0,
            last_check_timeouts: Timestamp::ZERO,
            worker_paused_for_block: false,
            own_fee_refused: None,
        }
    }

    /// Our worker's proof was refused by OCaml's prover rule at the best
    /// tip: no block can buy work at this fee from this key, so the worker
    /// proves for nothing. Warns once per key and fee (RC7-FIXES): the
    /// dashboard refuses such a setting, `--run-snarker` can't check it
    /// before the ledger is there. Returns whether it warned.
    pub(super) fn warn_own_fee_refused(
        &mut self,
        work: &Snark,
        account: super::ProverAccount,
        time: Timestamp,
    ) -> bool {
        let fee = work.fee.0.as_u64();
        let Some(problem) = crate::external_snark_worker::fee_rule_problem(fee, account) else {
            return false;
        };
        let key = (work.snarker.clone().into(), fee);
        if self.own_fee_refused.as_ref() == Some(&key) {
            return false;
        }
        mina_core::warn!(time;
            kind = "SnarkWorkerFeeRefused",
            summary = format!("blocks won't buy the SNARK worker's proofs: {problem}"),
            public_key = key.0.to_string(),
            fee = crate::external_snark_worker::format_mina(fee));
        self.own_fee_refused = Some(key);
        true
    }

    pub fn work_is_referenced(&self, work: &Snark, tip: &v2::StateHash) -> bool {
        // Our pool retains the best tip's all_job_pairs, a subset of OCaml's
        // frontier-wide references. Match the full statement, not its job ID.
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool.ml#L280-L317>
        self.jobs_tip.as_ref() == Some(tip)
            && self
                .get(&work.job_id())
                .is_some_and(|job| job.matches_work(work))
    }

    /// Whether the pool may use, offer or serve `work` at `tip`. A stored
    /// work whose cached proofs failed a read is not usable (PC1).
    pub fn work_is_checked(&self, work: &Snark, tip: &v2::StateHash) -> bool {
        !work.proofs.is_unreadable()
            && self.work_is_referenced(work, tip)
            && self
                .prover_accounts
                .get(&work.snarker.clone().into())
                .is_some_and(|(checked_tip, account)| {
                    checked_tip == tip
                        && super::check_prover(
                            work.fee.0.as_u64(),
                            mina_core::constants::constraint_constants().account_creation_fee,
                            *account,
                        )
                        .is_ok()
                })
    }

    /// OCaml compares remote competing fees before prover-account checks.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool_diff.ml#L138-L170>
    /// A stored work whose cached proofs failed a read competes with nothing.
    pub(super) fn work_fee_is_competitive(&self, work: &Snark) -> bool {
        self.get(&work.job_id())
            .and_then(|job| job.snark.as_ref())
            .filter(|stored| !stored.work.proofs.is_unreadable())
            .is_none_or(|stored| work.fee.0.as_u64() < stored.work.fee.0.as_u64())
    }

    /// Stored works whose cached proofs failed a read (PC1): the pool drops
    /// them, and their jobs are open again.
    pub fn unreadable_works_iter(&self) -> impl Iterator<Item = &JobState> {
        self.jobs_iter().filter(|job| {
            job.snark
                .as_ref()
                .is_some_and(|snark| snark.work.proofs.is_unreadable())
        })
    }

    pub(super) fn cache_prover_account(
        &mut self,
        prover: crate::account::AccountPublicKey,
        tip: v2::StateHash,
        account: super::ProverAccount,
    ) {
        if self.prover_accounts.len() >= 1024 && !self.prover_accounts.contains_key(&prover) {
            // Stored work needs these entries for production and forwarding.
            // At capacity, discard only unreferenced accounts, including those
            // of candidates that have not passed proof verification. If all
            // entries are pinned, allow the stored provers plus this candidate;
            // the cache remains bounded by the tracked work, not peer input.
            let pinned = self
                .jobs_iter()
                .filter_map(|job| job.snark.as_ref())
                .map(|snark| crate::account::AccountPublicKey::from(snark.work.snarker.clone()))
                .collect::<std::collections::BTreeSet<_>>();
            self.prover_accounts
                .retain(|prover, _| pinned.contains(prover));
        }
        self.prover_accounts.insert(prover, (tip, account));
    }

    pub fn remove_snark_work(&mut self, id: &SnarkJobId) {
        self.pool.update(id, |job| {
            job.snark = None;
        });
    }

    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }

    pub fn last_index(&self) -> u64 {
        self.pool.last_index()
    }

    pub fn contains(&self, id: &SnarkJobId) -> bool {
        self.pool.contains(id)
    }

    pub fn get(&self, id: &SnarkJobId) -> Option<&JobState> {
        self.pool.get(id)
    }

    pub fn insert(&mut self, job: JobState) {
        self.pool.insert(job)
    }

    pub fn remove(&mut self, id: &SnarkJobId) -> Option<JobState> {
        self.pool.remove(id)
    }

    /// Stores a verified work for its job. Its proofs go to the proof cache
    /// (PC1): only their statements stay decoded, as OCaml's pool keeps a
    /// cache tag for each proof.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool_diff.ml#L52-L64>
    pub fn set_snark_work(&mut self, mut snark: SnarkWork) -> Option<SnarkWork> {
        if !self.get(&snark.work.job_id())?.matches_work(&snark.work) {
            return None;
        }
        snark.work = snark.work.cached();
        self.pool
            .update(&snark.work.job_id(), move |job| job.snark.replace(snark))?
    }

    pub fn set_commitment(&mut self, commitment: JobCommitment) -> Option<JobCommitment> {
        let job_id = commitment.commitment.job_id.clone();
        self.pool
            .update(&job_id, move |job| job.commitment.replace(commitment))?
    }

    pub fn remove_commitment(&mut self, id: &SnarkJobId) -> Option<JobCommitment> {
        self.pool
            .silent_update(id, |job_state| job_state.commitment.take())?
    }

    /// Drops the commitments of `snarker`, a key our worker no longer uses
    /// (P5.4): its jobs are open again. Returns how many there were.
    pub fn release_commitments_of(&mut self, snarker: &v2::NonZeroCurvePoint) -> usize {
        let ids = self
            .jobs_iter()
            .filter(|job| {
                job.commitment
                    .as_ref()
                    .is_some_and(|c| &c.commitment.snarker == snarker)
            })
            .map(|job| job.id.clone())
            .collect::<Vec<_>>();
        for id in &ids {
            self.remove_commitment(id);
        }
        ids.len()
    }

    pub fn retain<F>(&mut self, mut get_new_job_order: F)
    where
        F: FnMut(&JobState) -> Option<usize>,
    {
        self.pool
            .retain_and_update(|_, job| match get_new_job_order(job) {
                None => false,
                Some(order) => {
                    job.order = order;
                    true
                }
            });
    }

    pub fn range<R>(&self, range: R) -> impl '_ + DoubleEndedIterator<Item = (u64, &'_ JobState)>
    where
        R: RangeBounds<u64>,
    {
        self.pool.range(range)
    }

    /// Whether our worker may commit to the job at `fee` (nanomina), our
    /// configured fee: the selector's rule, [`JobState::is_worth_proving`].
    /// A job with dearer stored work is open, as the pool takes work that is
    /// strictly cheaper than what it holds, our own included (`WorkAdd`).
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool_diff.ml#L133-L157>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool_diff.ml#L185-L208>
    pub fn should_create_commitment(&self, job_id: &SnarkJobId, fee: u64) -> bool {
        self.get(job_id)
            .is_some_and(|job| job.is_worth_proving(fee))
    }

    /// `own_key` is our worker's key: its commitments do not time out (see
    /// [`is_job_commitment_timed_out`]).
    pub fn is_commitment_timed_out(
        &self,
        id: &SnarkJobId,
        time_now: Timestamp,
        own_key: Option<&v2::NonZeroCurvePoint>,
    ) -> bool {
        self.get(id)
            .is_some_and(|job| is_job_commitment_timed_out(job, time_now, own_key))
    }

    pub fn timed_out_commitments_iter<'a>(
        &'a self,
        time_now: Timestamp,
        own_key: Option<&'a v2::NonZeroCurvePoint>,
    ) -> impl Iterator<Item = &'a SnarkJobId> {
        self.jobs_iter()
            .filter(move |job| is_job_commitment_timed_out(job, time_now, own_key))
            .map(|job| &job.id)
    }

    pub fn jobs_iter(&self) -> impl Iterator<Item = &JobState> {
        self.pool.states()
    }

    pub fn available_jobs_iter(&self) -> impl Iterator<Item = &JobState> {
        self.jobs_iter().filter(|job| job.is_available())
    }

    pub fn available_jobs_with_highest_priority(&self, n: usize) -> Vec<&JobState> {
        lowest_order(self.available_jobs_iter(), n)
    }

    /// The jobs our worker would prove for `fee` (nanomina), in no order;
    /// none with a zkApp command if `skip_zkapp` (its prover has no zkApp
    /// circuits, `SnarkerConfig::skip_zkapp_jobs`).
    pub fn jobs_to_prove_iter(
        &self,
        fee: u64,
        skip_zkapp: bool,
    ) -> impl Iterator<Item = &JobState> {
        self.jobs_iter().filter(move |job| {
            job.is_worth_proving(fee) && !(skip_zkapp && job.has_zkapp_command())
        })
    }

    /// OCaml's `seq` selection: the first `n` of [`Self::jobs_to_prove_iter`]
    /// in scan-state order (`order` is the index in `all_work_jobs`).
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector/sequence.ml#L1-L8>
    pub fn jobs_to_prove_with_highest_priority(
        &self,
        fee: u64,
        skip_zkapp: bool,
        n: usize,
    ) -> Vec<&JobState> {
        lowest_order(self.jobs_to_prove_iter(fee, skip_zkapp), n)
    }

    /// zkApp-command base jobs at the current tip.
    pub fn zkapp_jobs(&self) -> usize {
        self.zkapp_jobs
    }

    pub fn completed_snarks_iter(&self) -> impl '_ + Iterator<Item = &'_ Snark> {
        self.jobs_iter()
            .filter_map(|job| job.snark.as_ref())
            .map(|snark| &snark.work)
            .filter(|work| {
                self.jobs_tip
                    .as_ref()
                    .is_some_and(|tip| self.work_is_checked(work, tip))
            })
    }

    /// OCaml's completed-work RPC walks the selector's available jobs in
    /// scan-state order before taking 32.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector/work_lib.ml#L237-L239>
    pub fn completed_snarks_for_rpc(&self) -> Option<Vec<&Snark>> {
        let Some(tip) = self.jobs_tip.as_ref() else {
            return None;
        };
        Some(
            lowest_order(
                self.jobs_iter().filter(|job| {
                    job.snark
                        .as_ref()
                        .is_some_and(|snark| self.work_is_checked(&snark.work, tip))
                }),
                mina_p2p_messages::rpc::MAX_COMPLETED_SNARKS,
            )
            .into_iter()
            .filter_map(|job| job.snark.as_ref().map(|snark| &snark.work))
            .collect(),
        )
    }

    pub(super) fn job_summary(&self, id: &SnarkJobId) -> Option<JobSummary> {
        self.get(id).map(|job| job.summary())
    }

    pub fn candidates_prune(&mut self) {
        self.candidates.retain(|id| {
            let job = self.pool.get(id);
            move |candidate| match job {
                None => false,
                Some(job) => match job.snark.as_ref() {
                    None => true,
                    Some(snark) => &snark.work < candidate,
                },
            }
        });
    }

    pub fn next_commitments_to_send(
        &self,
        index_and_limit: (u64, u8),
    ) -> (Vec<SnarkJobCommitment>, u64, u64) {
        self.pool
            .next_messages_to_send(index_and_limit, |job| job.commitment_msg().cloned())
    }

    pub fn next_snarks_to_send(&self, index_and_limit: (u64, u8)) -> (Vec<SnarkInfo>, u64, u64) {
        self.pool.next_messages_to_send(index_and_limit, |job| {
            let work = &job.snark.as_ref()?.work;
            self.jobs_tip
                .as_ref()
                .filter(|tip| self.work_is_checked(work, tip))?;
            job.snark_msg()
        })
    }

    pub fn resources_usage(&self) -> serde_json::Value {
        let (size, inconsistency) = self.candidates.check();

        serde_json::json!({
            "pool_size": self.pool.len(),
            "admission": self.admission_stats,
            "candidates_size": size,
            "candidates_inconsistency": inconsistency,
        })
    }
}

fn is_job_commitment_timed_out(
    job: &JobState,
    time_now: Timestamp,
    own_key: Option<&v2::NonZeroCurvePoint>,
) -> bool {
    let Some(commitment) = job.commitment.as_ref() else {
        return false;
    };

    // Our own commitment marks a job our worker has taken: it is proving it,
    // has offered its proof (admission is asynchronous), or failed on it. It
    // lasts as long as the job is in the scan state, as OCaml's selector
    // keeps a job it has handed out in `jobs_scheduled` until the job leaves
    // the best tip's work, with no time limit (its reassignment wait is the
    // partitioner's, per proof handed to a worker, and our in-process worker
    // never drops one). So a slow proof's job is never chosen again before
    // its proof is admitted, and a failed job is never retried.
    // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector/work_lib.ml#L142-L173>
    if own_key == Some(&commitment.commitment.snarker) {
        return false;
    }
    let timeout = job.estimated_duration();
    let passed_time = time_now.checked_sub(commitment.commitment.timestamp());
    let is_timed_out = passed_time.is_some_and(|dur| dur >= timeout);
    let didnt_deliver = job
        .snark
        .as_ref()
        .is_none_or(|snark| snark.work < commitment.commitment);

    is_timed_out && didnt_deliver
}

/// The `n` jobs with the lowest order (highest priority), lowest first.
fn lowest_order<'a>(jobs: impl Iterator<Item = &'a JobState>, n: usize) -> Vec<&'a JobState> {
    let mut jobs = jobs.fold(Vec::with_capacity(n.saturating_add(1)), |mut jobs, job| {
        jobs.push(job);
        if jobs.len() > n {
            jobs.sort_by_key(|job| job.order);
            jobs.pop();
        }
        jobs
    });
    jobs.sort_by_key(|job| job.order);
    jobs
}

impl fmt::Debug for SnarkPoolState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SnarkPoolState")
            .field("pool", &self.pool)
            .finish()
    }
}

impl JobState {
    /// A job ID only indexes endpoint ledger hashes. Work must match every
    /// statement field and the One/Two shape (the SOK digest is verified separately).
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool.ml#L354-L369>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool.ml#L461-L475>
    pub fn matches_work(&self, work: &Snark) -> bool {
        job_statement(&self.job).is_some_and(|statement| statement == work.statement())
    }

    pub fn is_available(&self) -> bool {
        self.commitment.is_none() && self.snark.is_none()
    }

    /// OCaml's selector takes a job that is not scheduled and has no stored
    /// work as cheap as `fee` (nanomina): it undercuts dearer work. Here a
    /// commitment, ours or a Rust peer's, counts as scheduled. zkApp command
    /// base jobs are taken like any other (P5.2e), as OCaml takes them.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector/work_lib.ml#L158-L173>
    pub fn is_worth_proving(&self, fee: u64) -> bool {
        self.commitment.is_none()
            && self
                .snark
                .as_ref()
                .is_none_or(|stored| fee < stored.work.fee.0.as_u64())
    }

    /// The proofs our worker makes for this bundle, one after another: one
    /// for an ordinary base job or a merge; for a zkApp command one per
    /// segment and one per merge of two.
    pub fn proofs_to_make(&self) -> usize {
        use ledger::proofs::snark_worker::JobKind;
        let proofs = |job: &AvailableJobMessage| match job {
            AvailableJobMessage::Base(base) => {
                JobKind::of_transaction(&base.transaction_with_status.data).proofs()
            }
            AvailableJobMessage::Merge { .. } => 1,
        };
        match &self.job {
            OneOrTwo::One(job) => proofs(job),
            OneOrTwo::Two((first, second)) => proofs(first).saturating_add(proofs(second)),
        }
    }

    /// How long our worker may take on this bundle before it is cancelled:
    /// OCaml's reassignment wait for each proof it makes. OCaml's work
    /// partitioner hands each single job, zkApp segment and merge of
    /// segments to a worker on its own, and reassigns only one that is not
    /// back within the wait, so a 16-segment command (31 proofs) is never
    /// held to one 420 s window.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_partitioner/work_partitioner.ml#L56-L86>
    pub fn work_timeout(&self) -> Duration {
        crate::external_snark_worker::WORK_REASSIGNMENT_WAIT
            .saturating_mul(u32::try_from(self.proofs_to_make()).unwrap_or(u32::MAX))
            .max(self.estimated_duration())
    }

    /// Whether a base job of this bundle is a zkApp command.
    pub fn has_zkapp_command(&self) -> bool {
        use mina_p2p_messages::v2::{
            MinaBaseUserCommandStableV2 as UserCommand, MinaTransactionTransactionStableV2,
        };
        let is_zkapp = |job: &AvailableJobMessage| match job {
            AvailableJobMessage::Base(base) => matches!(
                &base.transaction_with_status.data,
                MinaTransactionTransactionStableV2::Command(command)
                    if matches!(command.as_ref(), UserCommand::ZkappCommand(_))
            ),
            AvailableJobMessage::Merge { .. } => false,
        };
        match &self.job {
            OneOrTwo::One(job) => is_zkapp(job),
            OneOrTwo::Two((first, second)) => is_zkapp(first) || is_zkapp(second),
        }
    }

    pub fn commitment_msg(&self) -> Option<&SnarkJobCommitment> {
        self.commitment.as_ref().map(|v| &v.commitment)
    }

    pub fn snark_msg(&self) -> Option<SnarkInfo> {
        self.snark.as_ref().map(|v| v.work.info())
    }

    pub fn summary(&self) -> JobSummary {
        use mina_p2p_messages::v2::{
            MinaBaseUserCommandStableV2 as UserCommand, MinaTransactionTransactionStableV2,
        };
        let account_updates = |job: &_| match job {
            AvailableJobMessage::Base(base) => match &base.transaction_with_status.data {
                MinaTransactionTransactionStableV2::Command(command) => match command.as_ref() {
                    UserCommand::ZkappCommand(zkapp) => zkapp.account_updates.len(),
                    UserCommand::SignedCommand(_) => 1,
                },
                _ => 1,
            },
            AvailableJobMessage::Merge { .. } => 1,
        };
        let account_updates = match &self.job {
            OneOrTwo::One(job) => account_updates(job),
            OneOrTwo::Two((job1, job2)) => account_updates(job1)
                .checked_add(account_updates(job2))
                .expect("overflow"),
        };

        if matches!(
            self.job,
            OneOrTwo::One(AvailableJobMessage::Base(_))
                | OneOrTwo::Two((AvailableJobMessage::Base(_), _))
        ) {
            JobSummary::Tx(account_updates)
        } else {
            JobSummary::Merge(account_updates)
        }
    }

    pub fn estimated_duration(&self) -> Duration {
        self.summary().estimated_duration()
    }
}

/// Derive the requested statement, including the merge of both child statements.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transaction_snark_scan_state/transaction_snark_scan_state.ml#L725-L732>
pub(super) fn job_statement(
    job: &OneOrTwo<AvailableJobMessage>,
) -> Option<v2::TransactionSnarkWorkStatementStableV2> {
    use ledger::scan_state::scan_state::transaction_snark::Statement;
    let statement = |job: &AvailableJobMessage| match job {
        AvailableJobMessage::Base(base) => Some(base.statement.clone()),
        AvailableJobMessage::Merge { left, right } => {
            let left = Statement::<()>::try_from(&left.0.statement).ok()?;
            let right = Statement::<()>::try_from(&right.0.statement).ok()?;
            Some((&left.merge(&right).ok()?).into())
        }
    };
    Some(match job {
        OneOrTwo::One(job) => v2::TransactionSnarkWorkStatementStableV2::One(statement(job)?),
        OneOrTwo::Two((first, second)) => {
            v2::TransactionSnarkWorkStatementStableV2::Two((statement(first)?, statement(second)?))
        }
    })
}

impl AsRef<SnarkJobId> for JobState {
    fn as_ref(&self) -> &SnarkJobId {
        &self.id
    }
}

impl JobSummary {
    pub fn estimated_duration(&self) -> Duration {
        const BASE: Duration = Duration::from_secs(10);
        const MAX_LATENCY: Duration = Duration::from_secs(10);

        let (JobSummary::Tx(n) | JobSummary::Merge(n)) = self;
        BASE.saturating_mul(*n as u32).saturating_add(MAX_LATENCY)
    }
}
