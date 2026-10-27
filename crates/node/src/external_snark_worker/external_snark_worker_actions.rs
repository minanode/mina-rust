use std::time::Duration;

use mina_core::{
    snark::{Snark, SnarkJobId},
    ActionEvent,
};
use redux::{EnablingCondition, Timestamp};
use serde::{Deserialize, Serialize};

use crate::{snark_pool::JobSummary, SnarkerSetting, State};

use super::{ExternalSnarkWorkerError, ExternalSnarkWorkerState, ExternalSnarkWorkerWorkError};

#[derive(Debug, Clone, Serialize, Deserialize, ActionEvent)]
#[action_event(fields(display(job_id), display(error)))]
pub enum ExternalSnarkWorkerAction {
    Start,
    Started,
    StartTimeout {
        now: Timestamp,
    },
    Kill,
    Killed,

    #[action_event(level = debug)]
    SubmitWork {
        job_id: SnarkJobId,
        summary: JobSummary,
        /// The bundle proves a zkApp command.
        #[serde(default)]
        zkapp: bool,
    },
    /// The worker's proof, with the key and fee its sok digest commits to.
    #[action_event(level = debug)]
    WorkResult {
        result: Snark,
    },
    WorkError {
        error: ExternalSnarkWorkerWorkError,
    },
    /// How a proof went, for the operator (P5.4).
    #[action_event(level = debug)]
    Outcome {
        outcome: Box<super::WorkerOutcome>,
    },
    WorkTimeout {
        now: Timestamp,
    },

    CancelWork,
    WorkCancelled,

    PruneWork,

    Error {
        error: ExternalSnarkWorkerError,
        permanent: bool,
    },

    /// The operator starts the worker with a key and fee, re-prices it, or
    /// stops it (`None`) while the node runs (P5.4).
    #[action_event(level = info)]
    Configure {
        setting: Option<SnarkerSetting>,
    },
}

/// OCaml's default `--work-reassignment-wait`: a proof handed to a worker
/// (a single job, a zkApp segment or a merge of segments) stays its proof for
/// this long. Our estimate (10 s per account update plus 10 s) is far shorter
/// than a proof on a loaded host.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/cli_lib/default.ml#L2>
pub const WORK_REASSIGNMENT_WAIT: Duration = Duration::from_millis(420_000);

/// How long our worker may take on a job before it is cancelled, when the
/// job is no longer in the pool to count its proofs (see
/// [`crate::snark_pool::JobState::work_timeout`]).
pub fn work_timeout(summary: &JobSummary) -> Duration {
    summary.estimated_duration().max(WORK_REASSIGNMENT_WAIT)
}

pub type ExternalSnarkWorkerActionWithMetaRef<'a> =
    redux::ActionWithMeta<&'a ExternalSnarkWorkerAction>;

impl EnablingCondition<State> for ExternalSnarkWorkerAction {
    fn is_enabled(&self, state: &State, _time: redux::Timestamp) -> bool {
        match self {
            ExternalSnarkWorkerAction::Start => {
                state.config.snarker.is_some()
                    && matches!(
                        state.external_snark_worker.0.state,
                        ExternalSnarkWorkerState::None
                    )
            }
            ExternalSnarkWorkerAction::Started => {
                matches!(
                    state.external_snark_worker.0.state,
                    ExternalSnarkWorkerState::Starting
                )
            }
            ExternalSnarkWorkerAction::StartTimeout { now } => {
                const TIMEOUT: Duration = Duration::from_secs(120);
                matches!(
                    state.external_snark_worker.0.state,
                    ExternalSnarkWorkerState::Starting
                ) && now
                    .checked_sub(state.external_snark_worker.0.timestamp)
                    .is_some_and(|d| d > TIMEOUT)
            }
            ExternalSnarkWorkerAction::Kill => !matches!(
                state.external_snark_worker.0.state,
                ExternalSnarkWorkerState::Error(_, false)
                    | ExternalSnarkWorkerState::None
                    | ExternalSnarkWorkerState::Killing
            ),
            ExternalSnarkWorkerAction::Killed => {
                matches!(
                    state.external_snark_worker.0.state,
                    ExternalSnarkWorkerState::Killing
                )
            }
            ExternalSnarkWorkerAction::SubmitWork { .. } => state.external_snark_worker.is_idle(),
            // The in-process worker cannot stop a proof it has started, so a
            // proof may finish after a cancel, even after the cancel ended. A
            // finished proof is offered to the pool, which keeps it only if
            // its job is still wanted, unless the worker was stopped or now
            // uses another key (P5.4): the reducer then drops it.
            ExternalSnarkWorkerAction::WorkResult { .. } => true,
            ExternalSnarkWorkerAction::WorkError { .. } => {
                matches!(
                    state.external_snark_worker.0.state,
                    ExternalSnarkWorkerState::Working(..) | ExternalSnarkWorkerState::Cancelling(_)
                )
            }
            ExternalSnarkWorkerAction::WorkTimeout { now } => {
                if let ExternalSnarkWorkerState::Working(job_id, summary) =
                    &state.external_snark_worker.0.state
                {
                    let timeout = state
                        .snark_pool
                        .get(job_id)
                        .map_or_else(|| work_timeout(summary), |job| job.work_timeout());
                    now.checked_sub(state.external_snark_worker.0.timestamp)
                        .is_some_and(|d| d > timeout)
                } else {
                    false
                }
            }
            ExternalSnarkWorkerAction::CancelWork => {
                matches!(
                    state.external_snark_worker.0.state,
                    ExternalSnarkWorkerState::Working(..)
                )
            }
            ExternalSnarkWorkerAction::WorkCancelled => {
                matches!(
                    state.external_snark_worker.0.state,
                    ExternalSnarkWorkerState::Cancelling(_)
                )
            }
            ExternalSnarkWorkerAction::PruneWork => {
                matches!(
                    state.external_snark_worker.0.state,
                    ExternalSnarkWorkerState::WorkReady(..)
                        | ExternalSnarkWorkerState::WorkError(..)
                        | ExternalSnarkWorkerState::Cancelled(..)
                )
            }
            ExternalSnarkWorkerAction::Error { .. } => true,
            ExternalSnarkWorkerAction::Outcome { .. } => true,
            ExternalSnarkWorkerAction::Configure { .. } => state.config.worker_control,
        }
    }
}
