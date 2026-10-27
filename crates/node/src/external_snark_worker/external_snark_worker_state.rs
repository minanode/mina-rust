use std::collections::VecDeque;

use redux::Timestamp;
use serde::{Deserialize, Serialize};

use crate::snark_pool::JobSummary;

use super::{
    ExternalSnarkWorkerError, ExternalSnarkWorkerWorkError, IncludedWorkLog, SnarkWorkId,
    SnarkWorkResult, WorkerJob, WorkerOutcome,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSnarkWorkers(pub(crate) ExternalSnarkWorker);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSnarkWorker {
    pub(crate) state: ExternalSnarkWorkerState,
    pub(crate) timestamp: Timestamp,
    /// The job being proved (P5.4).
    #[serde(default)]
    pub(crate) job: Option<WorkerJob>,
    /// The latest outcomes, oldest first (P5.4).
    #[serde(default)]
    pub(crate) recent: VecDeque<WorkerOutcome>,
    /// Our work that blocks included (P5.4).
    #[serde(default)]
    pub included: IncludedWorkLog,
    /// Why the saved worker setting was not used at startup (P5.4).
    #[serde(default)]
    pub(crate) setting_warning: Option<String>,
    #[serde(default)]
    pub proofs_made: u64,
    #[serde(default)]
    pub proofs_failed: u64,
    #[serde(default)]
    pub proofs_sold: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExternalSnarkWorkerState {
    None,
    Starting,

    Idle,
    Working(SnarkWorkId, JobSummary),
    WorkReady(SnarkWorkId, SnarkWorkResult),
    WorkError(SnarkWorkId, ExternalSnarkWorkerWorkError),

    Cancelling(SnarkWorkId),
    Cancelled(SnarkWorkId),

    Killing,
    Error(ExternalSnarkWorkerError, bool),
}

impl ExternalSnarkWorkers {
    pub fn new(now: Timestamp) -> Self {
        ExternalSnarkWorkers(ExternalSnarkWorker {
            state: ExternalSnarkWorkerState::None,
            timestamp: now,
            job: None,
            recent: VecDeque::new(),
            included: IncludedWorkLog::default(),
            setting_warning: None,
            proofs_made: 0,
            proofs_failed: 0,
            proofs_sold: 0,
        })
    }

    /// Our work that blocks included (P5.4).
    pub fn included_work(&self) -> &IncludedWorkLog {
        &self.0.included
    }

    /// The saved worker setting could not be used at startup: the operator
    /// sees why.
    pub fn set_setting_warning(&mut self, warning: String) {
        self.0.setting_warning = Some(warning);
    }

    /// The log kept in the work directory, at startup.
    pub fn restore_included_work(&mut self, log: IncludedWorkLog) {
        self.0.included = log;
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.0.state, ExternalSnarkWorkerState::Idle)
    }

    pub fn has_idle(&self) -> bool {
        self.available() > 0
    }

    pub fn available(&self) -> usize {
        if matches!(self.0.state, ExternalSnarkWorkerState::Idle) {
            1
        } else {
            0
        }
    }

    pub fn working_job_id(&self) -> Option<&SnarkWorkId> {
        match &self.0.state {
            ExternalSnarkWorkerState::Working(job_id, _) => Some(job_id),
            _ => None,
        }
    }
}
