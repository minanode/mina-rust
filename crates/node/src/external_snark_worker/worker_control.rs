//! Starting, stopping and re-pricing the SNARK worker while the node runs
//! (P5.4), and what the operator sees of it.
//!
//! The worker's key and fee are `state.config.snarker`. A change
//! ([`super::ExternalSnarkWorkerAction::Configure`]) sets them; the worker,
//! if it runs with another key or fee, is killed and starts again with the
//! new ones. Our work is offered only for the key and fee the worker has
//! when the work is admitted ([`State::snark_worker_authorizes`]): a proof
//! made for a stopped worker, a former key or a former fee is dropped, also
//! one whose admission was under way at the change. The job such a proof was
//! for is released, as are all the commitments of a key no longer used, so
//! the worker can take the job again.

use std::collections::VecDeque;

use mina_core::snark::SnarkJobId;
use redux::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    account::AccountPublicKey,
    snark_pool::{check_prover, AdmissionError, JobSummary, ProverAccount},
    SnarkerConfig, SnarkerSetting, SnarkerStrategy, State,
};

/// Outcomes kept for the operator, the newest.
pub const MAX_RECENT_OUTCOMES: usize = 20;

/// The job the worker is proving.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WorkerJob {
    pub job_id: SnarkJobId,
    pub summary: JobSummary,
    /// The bundle proves a zkApp command.
    pub zkapp: bool,
    pub submitted: Timestamp,
}

/// How a job the worker took ended.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WorkerOutcome {
    pub job_id: SnarkJobId,
    /// What the job was, if it was the worker's current job.
    pub summary: Option<JobSummary>,
    pub zkapp: bool,
    pub time: Timestamp,
    /// From submission to outcome, if it was the worker's current job.
    pub duration_ms: Option<u64>,
    /// The fee the proof commits to, in nanomina; `None` if it failed.
    pub fee: Option<u64>,
    /// Offered to the pool: proved for the worker's current key.
    pub offered: bool,
    pub error: Option<String>,
}

pub(super) fn push_outcome(recent: &mut VecDeque<WorkerOutcome>, outcome: WorkerOutcome) {
    recent.push_back(outcome);
    let excess = recent.len().saturating_sub(MAX_RECENT_OUTCOMES);
    recent.drain(..excess);
}

/// Why work at `fee` (nanomina) for a prover with `account` would be
/// refused by OCaml's snark pool, and by ours, in the operator's words.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool.ml#L370-L424>
pub fn fee_rule_problem(fee: u64, account: ProverAccount) -> Option<String> {
    let creation_fee = mina_core::constants::constraint_constants().account_creation_fee;
    match check_prover(fee, creation_fee, account) {
        Ok(()) => None,
        Err(AdmissionError::InsufficientFee) => Some(format!(
            "this account doesn't exist yet: fee must be at least {} MINA, or fund it first",
            format_mina(creation_fee)
        )),
        Err(AdmissionError::ProverCannotReceive) => Some(
            "this account can't receive fees: its access and receive permissions must be None"
                .to_owned(),
        ),
        Err(error) => Some(format!("{error:?}")),
    }
}

/// Nanomina as MINA, without trailing zeros: `1`, `0.004`.
pub fn format_mina(nanomina: u64) -> String {
    let whole = nanomina / 1_000_000_000;
    let fraction = nanomina % 1_000_000_000;
    if fraction == 0 {
        return whole.to_string();
    }
    let fraction = format!("{fraction:09}");
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}

/// The worker as the operator sees it (`GET /snarker/control`).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcSnarkWorkerControl {
    /// The node was started with `--allow-worker-control` and no public API.
    pub allowed: bool,
    pub enabled: bool,
    pub public_key: Option<AccountPublicKey>,
    /// In nanomina.
    pub fee: Option<u64>,
    /// The fee OCaml asks of a prover without an account, in nanomina.
    pub account_creation_fee: u64,
    pub worker: crate::rpc::RpcSnarkWorkerStatus,
    /// The job being proved.
    pub job: Option<WorkerJob>,
    /// The latest outcomes, newest first.
    pub recent: Vec<WorkerOutcome>,
    /// The worker found no job it may take: a block of ours is near.
    pub paused_for_block: bool,
    /// The best tip's height, if the node has one.
    pub best_tip_height: Option<u32>,
    /// zkApp command jobs at the best tip.
    pub zkapp_jobs: usize,
    /// Why the setting saved in the work directory was not used at startup.
    #[serde(default)]
    pub setting_warning: Option<String>,
    /// Proofs our worker returned since the node started, also ones dropped
    /// after a stop or a fee change: O2's `mina_snark_worker_proofs_made_total`.
    #[serde(default)]
    pub proofs_made: u64,
    /// How the worker picks its next job (`--snarker-strategy`): the running
    /// worker's, or the one a worker started from the dashboard would use.
    #[serde(default)]
    pub strategy: SnarkerStrategy,
}

impl State {
    /// [`super::ExternalSnarkWorkerAction::Configure`] on the global state:
    /// the worker's key and fee, the log's keys and our commitments. A
    /// worker that runs with another setting is killed: it starts again
    /// with the new one once it has stopped (`Start` on the next
    /// `CheckTimeouts`). A worker stuck in an error may start again.
    pub(crate) fn snark_worker_configure(
        &mut self,
        setting: Option<&SnarkerSetting>,
        now: Timestamp,
        dispatcher: &mut redux::Dispatcher<crate::Action, State>,
    ) {
        use super::{ExternalSnarkWorkerAction, ExternalSnarkWorkerState};
        use crate::external_snark_worker_effectful::ExternalSnarkWorkerEffectfulAction;

        let previous = self.config.snarker.take();
        let strategy = self.config.snarker_strategy;
        let next = setting.map(|setting| {
            let mut config = SnarkerConfig::with_setting(setting, previous.as_ref(), strategy);
            config.block_first_lead = self.config.block_first_lead;
            config
        });
        let changed = previous.as_ref().map(SnarkerConfig::setting) != setting.cloned();
        // The job being proved is abandoned: its proof, made for the former
        // setting, won't be offered. Release it, or our commitment, which
        // never times out, would keep it from the worker for good.
        if let (true, Some(previous), Some(job)) =
            (changed, &previous, &self.external_snark_worker.0.job)
        {
            let ours = self
                .snark_pool
                .get(&job.job_id)
                .and_then(|state| state.commitment.as_ref())
                .is_some_and(|c| &c.commitment.snarker == previous.public_key.as_ref());
            if ours {
                self.snark_pool.remove_commitment(&job.job_id);
            }
        }
        if let Some(previous) = &previous {
            if next.as_ref().map(|next| &next.public_key) != Some(&previous.public_key) {
                // The old key's commitments are no longer ours: release them,
                // so that the jobs are open again.
                let released = self
                    .snark_pool
                    .release_commitments_of(previous.public_key.as_ref());
                mina_core::info!(now;
                    kind = "SnarkWorkerCommitmentsReleased",
                    summary = "released the commitments of the key the worker no longer uses",
                    commitments = released);
            }
        }
        let worker = &mut self.external_snark_worker.0;
        if let Some(next) = &next {
            worker
                .included
                .watch(&next.public_key, u64::from(now) / 1_000_000);
        }
        mina_core::info!(now;
            kind = "SnarkWorkerConfigured",
            summary = match (&next, changed) {
                (None, _) => "SNARK worker stopped by the operator",
                (Some(_), true) => "SNARK worker setting changed by the operator",
                (Some(_), false) => "SNARK worker setting unchanged",
            },
            public_key = next.as_ref().map(|c| c.public_key.to_string()).unwrap_or_default(),
            fee = next.as_ref().map(|c| c.fee.0.as_u64()).unwrap_or_default(),
            strategy = next.as_ref().map(|c| format!("{:?}", c.strategy)).unwrap_or_default());
        self.config.snarker = next;

        // The file is rewritten, or deleted: a warning about it is moot.
        worker.setting_warning = None;
        if matches!(worker.state, ExternalSnarkWorkerState::Error(..)) {
            worker.state = ExternalSnarkWorkerState::None;
            worker.timestamp = now;
            worker.job = None;
        } else if changed
            && !matches!(
                worker.state,
                ExternalSnarkWorkerState::None | ExternalSnarkWorkerState::Killing
            )
        {
            dispatcher.push(ExternalSnarkWorkerAction::Kill);
        }
        dispatcher.push(ExternalSnarkWorkerEffectfulAction::SaveSetting {
            setting: setting.cloned(),
        });
    }

    /// Whether our worker's `work` may be offered now: it is made for the
    /// key and the fee the worker has. A proof made before a stop, or for a
    /// key or fee since changed, is not ours to offer any more.
    pub fn snark_worker_authorizes(&self, work: &mina_core::snark::Snark) -> bool {
        self.config.snarker.as_ref().is_some_and(|config| {
            config.public_key.as_ref() == &work.snarker && config.fee == work.fee
        })
    }

    /// The worker as the operator sees it.
    pub fn snark_worker_control(&self) -> RpcSnarkWorkerControl {
        let worker = &self.external_snark_worker.0;
        RpcSnarkWorkerControl {
            allowed: self.config.worker_control,
            enabled: self.config.snarker.is_some(),
            public_key: self
                .config
                .snarker
                .as_ref()
                .map(|config| config.public_key.clone()),
            fee: self
                .config
                .snarker
                .as_ref()
                .map(|config| config.fee.0.as_u64()),
            account_creation_fee: mina_core::constants::constraint_constants().account_creation_fee,
            worker: worker.state.clone().into(),
            job: worker.job.clone(),
            recent: worker.recent.iter().rev().cloned().collect(),
            paused_for_block: self.snark_pool.worker_paused_for_block,
            best_tip_height: self.transition_frontier.best_tip().map(|tip| tip.height()),
            zkapp_jobs: self.snark_pool.zkapp_jobs(),
            setting_warning: worker.setting_warning.clone(),
            proofs_made: worker.proofs_made,
            strategy: self
                .config
                .snarker
                .as_ref()
                .map_or(self.config.snarker_strategy, |config| config.strategy),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mina_is_formatted_without_trailing_zeros() {
        for (nanomina, mina) in [
            (0, "0"),
            (1_000_000_000, "1"),
            (4_000_000, "0.004"),
            (4_300_000, "0.0043"),
            (1, "0.000000001"),
            (2_250_000_000, "2.25"),
        ] {
            assert_eq!(format_mina(nanomina), mina);
        }
    }

    /// OCaml's rule: a prover without an account must ask at least the
    /// account creation fee (1 MINA); one whose account can't receive is
    /// refused at any fee.
    #[test]
    fn the_fee_rule_names_the_problem() {
        let missing = fee_rule_problem(4_000_000, ProverAccount::Missing).unwrap();
        assert_eq!(
            missing,
            "this account doesn't exist yet: fee must be at least 1 MINA, or fund it first"
        );
        assert_eq!(
            fee_rule_problem(1_000_000_000, ProverAccount::Missing),
            None
        );
        assert_eq!(
            fee_rule_problem(4_000_000, ProverAccount::Present { can_receive: true }),
            None
        );
        assert!(fee_rule_problem(
            10_000_000_000,
            ProverAccount::Present { can_receive: false }
        )
        .unwrap()
        .contains("can't receive"));
    }
}
