//! What the reporters did, for `GET /reporting/status` and the logs.

use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::Serialize;

use super::delivery::{Attempt, Delivery};

/// Per reporter: whether it runs, and its last attempts and outcomes.
#[derive(Serialize, Clone, Debug, Default, utoipa::ToSchema)]
pub struct ReportingStatus {
    pub uptime: Option<ReporterStatus>,
    pub node_status: Option<ReporterStatus>,
    pub node_error: Option<ReporterStatus>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq, utoipa::ToSchema)]
pub struct ReporterStatus {
    /// The URL without credentials or query.
    pub target: String,
    /// What is sent, e.g. `uptime block-only (no SNARK work)`.
    pub report_kind: String,
    /// Reports built and sent (each retried as policy allows).
    pub reports: u64,
    /// HTTP attempts, retries included.
    pub attempts: u64,
    /// Attempts after the first of their report.
    pub retries: u64,
    pub delivered: u64,
    /// Refused with a status that isn't retried.
    pub rejected: u64,
    /// Given up after the last attempt, or not sent (too large).
    pub dropped: u64,
    /// Stopped by the next iteration.
    pub interrupted: u64,
    /// Iterations that sent nothing, a prerequisite missing.
    pub skipped: u64,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_http_status: Option<u16>,
    pub last_error: Option<String>,
    pub last_skip_reason: Option<String>,
    pub last_report: Option<ReportIdentity>,
}

/// What the last report described.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq, utoipa::ToSchema)]
pub struct ReportIdentity {
    pub kind: String,
    pub created_at: String,
    pub chain_id: Option<String>,
    pub state_hash: Option<String>,
    pub height: Option<u32>,
    pub global_slot: Option<u32>,
    pub produced: bool,
    pub body_bytes: usize,
    pub id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reporter {
    Uptime,
    NodeStatus,
    NodeError,
}

impl Reporter {
    pub fn name(self) -> &'static str {
        match self {
            Self::Uptime => "uptime",
            Self::NodeStatus => "node status",
            Self::NodeError => "node error",
        }
    }
}

static STATUS: OnceLock<Mutex<ReportingStatus>> = OnceLock::new();

fn global() -> MutexGuard<'static, ReportingStatus> {
    let lock = STATUS.get_or_init(Default::default);
    match lock.lock() {
        Ok(guard) => guard,
        // A panic while recording leaves only counters behind: keep going.
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The process's reporters, for the HTTP route.
pub fn snapshot() -> ReportingStatus {
    global().clone()
}

/// Records into the process-wide status, or, for tests, into a private one.
#[derive(Clone, Default)]
pub struct StatusSink(Option<std::sync::Arc<Mutex<ReportingStatus>>>);

impl StatusSink {
    pub fn global() -> Self {
        Self(None)
    }

    pub fn private() -> Self {
        Self(Some(Default::default()))
    }

    pub fn get(&self) -> ReportingStatus {
        match &self.0 {
            None => snapshot(),
            Some(lock) => lock.lock().map(|g| g.clone()).unwrap_or_default(),
        }
    }

    pub fn update(&self, reporter: Reporter, f: impl FnOnce(&mut ReporterStatus)) {
        let apply = |status: &mut ReportingStatus| {
            let slot = match reporter {
                Reporter::Uptime => &mut status.uptime,
                Reporter::NodeStatus => &mut status.node_status,
                Reporter::NodeError => &mut status.node_error,
            };
            f(slot.get_or_insert_with(Default::default));
        };
        match &self.0 {
            None => apply(&mut global()),
            Some(lock) => match lock.lock() {
                Ok(mut guard) => apply(&mut guard),
                Err(poisoned) => apply(&mut poisoned.into_inner()),
            },
        }
    }

    pub fn enable(&self, reporter: Reporter, target: String, report_kind: &str) {
        self.update(reporter, |s| {
            s.target = target;
            s.report_kind = report_kind.to_owned();
        });
    }

    pub fn attempt(&self, reporter: Reporter, at: String, attempt_no: u32, outcome: &Attempt) {
        self.update(reporter, |s| {
            s.attempts = s.attempts.saturating_add(1);
            if attempt_no > 1 {
                s.retries = s.retries.saturating_add(1);
            }
            s.last_attempt_at = Some(at);
            match outcome {
                Attempt::Status(status) => {
                    s.last_http_status = Some(*status);
                    s.last_error = None;
                }
                Attempt::Transport(error) => {
                    s.last_http_status = None;
                    s.last_error = Some(error.clone());
                }
            }
        });
    }

    pub fn report(&self, reporter: Reporter, identity: ReportIdentity) {
        self.update(reporter, |s| {
            s.reports = s.reports.saturating_add(1);
            s.last_report = Some(identity);
        });
    }

    pub fn outcome(&self, reporter: Reporter, at: String, delivery: &Delivery) {
        self.update(reporter, |s| match delivery {
            Delivery::Delivered { .. } => {
                s.delivered = s.delivered.saturating_add(1);
                s.last_success_at = Some(at);
            }
            Delivery::Rejected { .. } => s.rejected = s.rejected.saturating_add(1),
            Delivery::Exhausted { .. } => s.dropped = s.dropped.saturating_add(1),
            Delivery::Interrupted { .. } => s.interrupted = s.interrupted.saturating_add(1),
        });
    }

    pub fn dropped(&self, reporter: Reporter, reason: String) {
        self.update(reporter, |s| {
            s.dropped = s.dropped.saturating_add(1);
            s.last_error = Some(reason);
        });
    }

    pub fn skipped(&self, reporter: Reporter, reason: &str) {
        self.update(reporter, |s| {
            s.skipped = s.skipped.saturating_add(1);
            s.last_skip_reason = Some(reason.to_owned());
        });
    }
}
