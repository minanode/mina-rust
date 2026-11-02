//! Opt-in uptime, node status and node error reporting, compatible with
//! OCaml Mina 4.0.4's services (R43e.2).
//!
//! Each reporter runs only with its URL. None shares code with the metrics
//! or with `/make_heartbeat`, which keeps its own envelope and consumers.
//! Uptime submissions follow OCaml's choice of SNARK work for the best tip
//! ([`uptime_work`]) and prove it on a dedicated worker.
//!
//! The reporters run on one thread with its own Tokio runtime and read the
//! node only through [`RpcRequest::ReportingSnapshotGet`], one consistent
//! sample per report.

pub mod delivery;
pub mod json;
pub mod node_error;
pub mod node_status;
pub mod schedule;
pub mod uptime;
pub mod uptime_prover;
pub mod uptime_work;
pub mod visibility;

#[cfg(test)]
mod tests;

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

use futures::future::BoxFuture;
use mina_node::{
    account::AccountSecretKey,
    rpc::{
        RpcReportingProducedBlockWatch, RpcReportingSnapshot, RpcReportingSnapshotGetResponse,
        RpcRequest,
    },
};
use mina_node_common::rpc::RpcSender;
use reqwest::Url;

use delivery::{Delivery, Interruptor, RetryPolicy, Timeouts};
use node_error::{NodeErrorEvent, NodeErrorReport};
use schedule::{Clock, IdSource};
use uptime::{BlockData, UptimeRequest};
use uptime_work::{BestTipWork, ProofOutcome, UptimeProofRequest, UptimeProver};
use visibility::{ReportIdentity, Reporter, StatusSink};

/// What uptime submissions contain, as shown in the status and logs.
pub const UPTIME_REPORT_KIND: &str = "uptime block and selected SNARK work";
pub const NODE_STATUS_REPORT_KIND: &str = "simplified node status";
pub const NODE_ERROR_REPORT_KIND: &str = "node error event";

#[derive(Default)]
pub struct ReportingConfig {
    pub uptime: Option<UptimeConfig>,
    pub node_status: Option<NodeStatusConfig>,
    pub node_error: Option<NodeErrorConfig>,
}

impl ReportingConfig {
    pub fn is_enabled(&self) -> bool {
        self.uptime.is_some() || self.node_status.is_some() || self.node_error.is_some()
    }
}

pub struct UptimeConfig {
    pub url: Url,
    /// Signs submissions; need not be the block producer's key.
    pub submitter: AccountSecretKey,
    pub snark_work_fee: mina_p2p_messages::v2::CurrencyFeeStableV1,
    /// Send the first 8 characters of the build commit.
    pub send_commit_sha: bool,
    /// Write each request body to a file here instead of sending it.
    pub dry_run_dir: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for UptimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UptimeConfig")
            .field("url", &delivery::redacted_url(&self.url))
            .field("submitter", &self.submitter.public_key().to_string())
            .field("send_commit_sha", &self.send_commit_sha)
            .field("dry_run_dir", &self.dry_run_dir)
            .finish()
    }
}

#[derive(Debug)]
pub struct NodeStatusConfig {
    pub url: Url,
}

#[derive(Debug)]
pub struct NodeErrorConfig {
    pub url: Url,
    pub contact_info: Option<String>,
}

/// Fixed for the reporters' lifetime.
#[derive(Clone)]
pub struct ReportingContext {
    pub slot_ms: u64,
    pub network: mina_signer::NetworkId,
    /// The full build commit.
    pub commit_hash: String,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdSource>,
    pub timeouts: Timeouts,
    pub uptime_retry: RetryPolicy,
    pub status: StatusSink,
    /// When the reporters started, by `clock`: `uptime_of_node` counts
    /// from here.
    pub started_ms: u64,
    /// How long a snapshot may take before the report is skipped.
    pub snapshot_timeout: Duration,
    pub produced_block_wait: Duration,
    pub produced_block_poll: Duration,
}

impl ReportingContext {
    pub fn new(slot_ms: u64, network: mina_signer::NetworkId, commit_hash: String) -> Self {
        Self {
            slot_ms,
            network,
            commit_hash,
            clock: Arc::new(schedule::SystemClock),
            ids: Arc::new(schedule::RandomUuid),
            timeouts: Timeouts::default(),
            uptime_retry: RetryPolicy::UPTIME,
            status: StatusSink::global(),
            started_ms: schedule::SystemClock.now_ms(),
            snapshot_timeout: Duration::from_secs(10),
            produced_block_wait: schedule::PRODUCED_BLOCK_WAIT,
            produced_block_poll: Duration::from_secs(1),
        }
    }

    fn now(&self) -> String {
        schedule::rfc3339(self.clock.now_ms())
    }
}

/// Where the reporters sample the node.
pub trait SnapshotSource: Send + Sync + 'static {
    fn snapshot(
        &self,
        watch: Option<RpcReportingProducedBlockWatch>,
    ) -> BoxFuture<'_, Option<RpcReportingSnapshot>>;
}

impl SnapshotSource for RpcSender {
    fn snapshot(
        &self,
        watch: Option<RpcReportingProducedBlockWatch>,
    ) -> BoxFuture<'_, Option<RpcReportingSnapshot>> {
        Box::pin(self.oneshot_request::<RpcReportingSnapshotGetResponse>(
            RpcRequest::ReportingSnapshotGet {
                produced_block_watch: watch,
            },
        ))
    }
}

/// OCaml's `max_blocklength_observed` gauges: the greatest heights seen in
/// any sample since start.
#[derive(Default)]
struct ObservedHeights {
    validated: AtomicU32,
    unvalidated: AtomicU32,
}

impl ObservedHeights {
    fn observe(&self, snapshot: &RpcReportingSnapshot) {
        if let Some(h) = snapshot.max_observed_block_height {
            self.validated.fetch_max(h, Ordering::Relaxed);
            self.unvalidated.fetch_max(h, Ordering::Relaxed);
        }
        if let Some(h) = snapshot.max_observed_unvalidated_block_height {
            self.unvalidated.fetch_max(h, Ordering::Relaxed);
        }
    }

    fn get(&self) -> (u32, u32) {
        (
            self.validated.load(Ordering::Relaxed),
            self.unvalidated.load(Ordering::Relaxed),
        )
    }
}

/// The parts shared by the reporters.
struct Shared<S> {
    source: S,
    ctx: ReportingContext,
    client: reqwest::Client,
    heights: ObservedHeights,
    /// The last sample, for an error report when the node can't answer.
    last_snapshot: Mutex<Option<Arc<RpcReportingSnapshot>>>,
}

impl<S: SnapshotSource> Shared<S> {
    async fn sample(&self, timeout: Duration) -> Option<Arc<RpcReportingSnapshot>> {
        let snapshot = tokio::time::timeout(timeout, self.source.snapshot(None))
            .await
            .ok()
            .flatten()?;
        self.heights.observe(&snapshot);
        let snapshot = Arc::new(snapshot);
        if let Ok(mut last) = self.last_snapshot.lock() {
            *last = Some(snapshot.clone());
        }
        Some(snapshot)
    }

    fn cached(&self) -> Option<Arc<RpcReportingSnapshot>> {
        self.last_snapshot.lock().ok().and_then(|last| last.clone())
    }

    async fn deliver(
        &self,
        reporter: Reporter,
        url: &Url,
        body: String,
        policy: RetryPolicy,
        classify: delivery::StatusPolicy,
        interrupt: &Interruptor,
    ) -> Delivery {
        let target = delivery::redacted_url(url);
        let status = &self.ctx.status;
        let ctx = &self.ctx;
        let max = policy.max_attempts;
        let mut observer = |attempt_no: u32, outcome: &delivery::Attempt| {
            status.attempt(reporter, ctx.now(), attempt_no, outcome);
            mina_core::info!(
                mina_core::log::system_time();
                kind = "Reporting",
                summary = format!("{} report attempt {attempt_no} of {max}: {outcome}", reporter.name()),
                url = target.clone(),
            );
        };
        let delivery = delivery::deliver(
            &self.client,
            url,
            body.into(),
            policy,
            classify,
            interrupt,
            &mut observer,
        )
        .await;
        status.outcome(reporter, ctx.now(), &delivery);
        let summary = match &delivery {
            Delivery::Delivered { attempts } => {
                format!(
                    "{} report delivered after {attempts} attempt(s)",
                    reporter.name()
                )
            }
            Delivery::Rejected { status, .. } => format!(
                "{} report refused with HTTP {status}, not retrying",
                reporter.name()
            ),
            Delivery::Exhausted { attempts, last } => format!(
                "{} report dropped after {attempts} attempt(s), the last: {last}",
                reporter.name()
            ),
            Delivery::Interrupted { attempts } => format!(
                "{} report interrupted by the next iteration after {attempts} attempt(s)",
                reporter.name()
            ),
        };
        match delivery {
            Delivery::Delivered { .. } => mina_core::info!(
                mina_core::log::system_time();
                kind = "Reporting",
                summary = summary,
                url = target,
            ),
            _ => mina_core::warn!(
                mina_core::log::system_time();
                kind = "Reporting",
                summary = summary,
                url = target,
            ),
        }
        delivery
    }

    fn skip(&self, reporter: Reporter, reason: &str) {
        self.ctx.status.skipped(reporter, reason);
        mina_core::info!(
            mina_core::log::system_time();
            kind = "Reporting",
            summary = format!("{} report skipped: {reason}", reporter.name()),
        );
    }
}

/// What becomes of the best tip's submission.
enum TipSubmission {
    /// Sent, with this `snark_work`.
    Send(Option<String>),
    /// Dropped, as OCaml drops it.
    Skipped,
    /// The next iteration started.
    Interrupted,
}

/// Uptime: from the next slot boundary, an iteration every five slots; each
/// sends the best tip, and our produced block when we produce within four
/// slots. The next iteration interrupts what the last still does.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/uptime_service/uptime_service.ml#L402-L548>
pub struct UptimeReporter<S> {
    shared: Arc<Shared<S>>,
    config: UptimeConfig,
    /// The SNARK work for best-tip submissions.
    prover: Arc<dyn UptimeProver>,
}

impl<S: SnapshotSource> UptimeReporter<S> {
    fn built_with_commit_sha(&self) -> Option<String> {
        self.config
            .send_commit_sha
            .then(|| uptime::short_commit(&self.shared.ctx.commit_hash))
    }

    /// Runs forever: waits for each desired start, interrupts the previous
    /// iteration and starts the next in the background.
    pub async fn run(self: Arc<Self>) {
        let ctx = &self.shared.ctx;
        let mut desired = schedule::first_iteration_start(ctx.clock.now_ms(), ctx.slot_ms);
        let mut previous: Option<Interruptor> = None;
        loop {
            let now = ctx.clock.now_ms();
            if now < desired {
                tokio::time::sleep(Duration::from_millis(desired.saturating_sub(now))).await;
            } else if now > desired {
                mina_core::warn!(
                    mina_core::log::system_time();
                    kind = "Reporting",
                    summary = "uptime: the current time is past the iteration's desired start",
                    late_ms = now.saturating_sub(desired),
                );
            }
            if let Some(previous) = previous.take() {
                previous.fire();
            }
            let interrupt = Interruptor::new();
            previous = Some(interrupt.clone());
            let this = self.clone();
            tokio::spawn(async move { this.iteration(desired, interrupt).await });
            desired = schedule::next_iteration_start(desired, ctx.slot_ms);
        }
    }

    /// One iteration, until done or interrupted.
    pub async fn iteration(&self, desired_start_ms: u64, interrupt: Interruptor) {
        let shared = &self.shared;
        let Some(snapshot) = interrupt
            .run(shared.sample(shared.ctx.snapshot_timeout))
            .await
        else {
            shared.ctx.status.update(Reporter::Uptime, |s| {
                s.interrupted = s.interrupted.saturating_add(1)
            });
            return;
        };
        let Some(snapshot) = snapshot else {
            shared.skip(
                Reporter::Uptime,
                "the node did not answer the snapshot request",
            );
            return;
        };
        let Some(peer_id) = snapshot.peer_id.clone() else {
            shared.skip(
                Reporter::Uptime,
                "the node is not yet a peer in the gossip network",
            );
            return;
        };
        let produced_at = snapshot
            .next_production_time
            .map(|t| u64::from(t) / 1_000_000)
            .filter(|&t| schedule::sends_produced_block(desired_start_ms, t, shared.ctx.slot_ms));

        match &snapshot.best_tip {
            None => shared.skip(Reporter::Uptime, "transition frontier not available"),
            Some(best_tip) => match self.best_tip_work(&snapshot, best_tip, &interrupt).await {
                TipSubmission::Interrupted => return,
                TipSubmission::Skipped => {}
                TipSubmission::Send(snark_work) => {
                    let delivery = self
                        .send_block(&snapshot, best_tip, &peer_id, false, snark_work, &interrupt)
                        .await;
                    if matches!(delivery, Some(Delivery::Interrupted { .. })) {
                        return;
                    }
                }
            },
        }
        if let Some(production_ms) = produced_at {
            self.send_produced_block(production_ms, &peer_id, &interrupt)
                .await;
        }
    }

    /// Whether the best tip goes, and with which `snark_work`, as OCaml
    /// chooses ([`uptime_work::best_tip_work`]).
    async fn best_tip_work(
        &self,
        snapshot: &RpcReportingSnapshot,
        best_tip: &mina_core::block::ArcBlockWithHash,
        interrupt: &Interruptor,
    ) -> TipSubmission {
        let shared = &self.shared;
        let job = match uptime_work::best_tip_work(best_tip, snapshot.uptime_job.as_ref()) {
            BestTipWork::BlockOnly(reason) => {
                mina_core::info!(
                    mina_core::log::system_time();
                    kind = "Reporting",
                    summary = format!("uptime: {reason}, sending the block without SNARK work"),
                );
                return TipSubmission::Send(None);
            }
            BestTipWork::Skip(reason) => {
                self.skip_with_error(&reason);
                return TipSubmission::Skipped;
            }
            BestTipWork::Prove(job) => job,
        };
        let request = UptimeProofRequest {
            best_tip: best_tip.clone(),
            job,
            submitter: self.config.submitter.public_key(),
            snark_work_fee: snapshot
                .snark_work_fee
                .clone()
                .unwrap_or_else(|| self.config.snark_work_fee.clone()),
            spec: snapshot.uptime_spec.clone(),
        };
        let Some(outcome) = interrupt.run(self.prover.prove(request)).await else {
            shared.ctx.status.update(Reporter::Uptime, |s| {
                s.interrupted = s.interrupted.saturating_add(1)
            });
            return TipSubmission::Interrupted;
        };
        match outcome {
            ProofOutcome::Proved(proof) => match proof.to_base64() {
                Ok(work) => TipSubmission::Send(Some(work)),
                Err(error) => {
                    self.skip_with_error(&format!("could not encode the SNARK work: {error}"));
                    TipSubmission::Skipped
                }
            },
            ProofOutcome::CircuitsUnavailable(error) => {
                // The loader logs only when its failure state changes. Keep
                // every skipped interval visible without repeating that log.
                self.shared.ctx.status.skipped(Reporter::Uptime, &error);
                TipSubmission::Skipped
            }
            ProofOutcome::Failed(error) => {
                self.skip_with_error(&format!("computing the SNARK work failed: {error}"));
                TipSubmission::Skipped
            }
        }
    }

    /// A submission OCaml drops with an error log.
    fn skip_with_error(&self, reason: &str) {
        self.shared.ctx.status.skipped(Reporter::Uptime, reason);
        mina_core::error!(
            mina_core::log::system_time();
            kind = "Reporting",
            summary = format!("uptime report skipped: {reason}"),
        );
    }

    /// Waits for the produced block's slot, then up to three minutes for the
    /// local production notification, and sends it without SNARK work.
    async fn send_produced_block(
        &self,
        production_ms: u64,
        peer_id: &str,
        interrupt: &Interruptor,
    ) {
        let shared = &self.shared;
        mina_core::info!(
            mina_core::log::system_time();
            kind = "Reporting",
            summary = "uptime: will send the next produced block",
            production_time = schedule::rfc3339(production_ms),
        );
        let now = shared.ctx.clock.now_ms();
        if production_ms > now
            && interrupt
                .run(tokio::time::sleep(Duration::from_millis(
                    production_ms.saturating_sub(now),
                )))
                .await
                .is_none()
        {
            return;
        }
        // OCaml subscribes only after best-tip work/delivery and `at tm`.
        // Bvar drops earlier broadcasts. Retain eligible events across forks,
        // but do not replay a completed event from before this wait.
        // OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/uptime_service/uptime_service.ml#L166-L198>
        static NEXT_SUBSCRIPTION: AtomicU64 = AtomicU64::new(1);
        let subscription = NEXT_SUBSCRIPTION.fetch_add(1, Ordering::Relaxed);
        let start = shared
            .source
            .snapshot(Some(RpcReportingProducedBlockWatch::Start { subscription }));
        let subscribed = interrupt
            .run(tokio::time::timeout(shared.ctx.snapshot_timeout, start))
            .await;
        if matches!(subscribed, Some(Ok(Some(_)))) {
            self.wait_for_produced_block(subscription, peer_id, interrupt)
                .await;
        } else {
            shared.skip(Reporter::Uptime, "could not subscribe to local production");
        }
        // Clear after consumption, timeout or interruption, even if the start
        // reply was lost. An old iteration must not cancel a newer waiter.
        let cancel = shared
            .source
            .snapshot(Some(RpcReportingProducedBlockWatch::Cancel {
                subscription,
            }));
        let _ = tokio::time::timeout(shared.ctx.snapshot_timeout, cancel).await;
    }

    async fn wait_for_produced_block(
        &self,
        subscription: u64,
        peer_id: &str,
        interrupt: &Interruptor,
    ) {
        let shared = &self.shared;
        let deadline = Instant::now() + shared.ctx.produced_block_wait;
        loop {
            let Some(snapshot) = interrupt
                .run(shared.sample(shared.ctx.snapshot_timeout))
                .await
            else {
                return;
            };
            let produced = snapshot.as_ref().and_then(|s| {
                s.produced_block
                    .as_ref()
                    .filter(|p| p.subscription == subscription)
                    .map(|p| (s.clone(), p.block.clone()))
            });
            if let Some((snapshot, block)) = produced {
                self.send_block(&snapshot, &block, peer_id, true, None, interrupt)
                    .await;
                return;
            }
            if Instant::now() >= deadline {
                let reason = format!(
                    "no produced block within {:.1} minutes after the scheduled time",
                    shared.ctx.produced_block_wait.as_secs_f64() / 60.0
                );
                shared.ctx.status.skipped(Reporter::Uptime, &reason);
                mina_core::error!(
                    mina_core::log::system_time();
                    kind = "Reporting",
                    summary = format!("uptime: {reason}"),
                );
                return;
            }
            if interrupt
                .run(tokio::time::sleep(shared.ctx.produced_block_poll))
                .await
                .is_none()
            {
                return;
            }
        }
    }

    /// Serializes and signs once, then delivers with retries.
    async fn send_block(
        &self,
        snapshot: &RpcReportingSnapshot,
        block: &mina_core::block::ArcBlockWithHash,
        peer_id: &str,
        produced: bool,
        snark_work: Option<String>,
        interrupt: &Interruptor,
    ) -> Option<Delivery> {
        let shared = &self.shared;
        let encoded = match uptime::encode_block_base64(&block.block) {
            Ok(encoded) => encoded,
            Err(error) => {
                shared.ctx.status.dropped(
                    Reporter::Uptime,
                    format!("could not encode the block: {error}"),
                );
                return None;
            }
        };
        let created_at = shared.ctx.now();
        let data = BlockData {
            block: encoded,
            created_at: created_at.clone(),
            peer_id: peer_id.to_owned(),
            snark_work,
            graphql_control_port: None,
            built_with_commit_sha: self.built_with_commit_sha(),
        };
        let request =
            UptimeRequest::create(data, &self.config.submitter, shared.ctx.network.clone());
        shared.ctx.status.report(
            Reporter::Uptime,
            ReportIdentity {
                kind: if produced {
                    "produced block".to_owned()
                } else {
                    "best tip".to_owned()
                },
                created_at,
                chain_id: snapshot.chain_id.clone(),
                state_hash: Some(block.hash().to_string()),
                height: Some(block.height()),
                global_slot: Some(block.global_slot()),
                produced,
                body_bytes: request.body.len(),
                id: None,
            },
        );
        mina_core::info!(
            mina_core::log::system_time();
            kind = "Reporting",
            summary = "uptime: sending a block",
            state_hash = block.hash().to_string(),
            height = block.height(),
            produced = produced,
            includes_snark_work = request.data.snark_work.is_some(),
            bytes = request.body.len(),
        );
        if let Some(dir) = &self.config.dry_run_dir {
            let kind = if produced { "produced" } else { "best-tip" };
            match uptime::write_dry_run(
                dir,
                &request.data.created_at,
                kind,
                &block.hash().to_string(),
                &request.body,
            ) {
                Ok(path) => mina_core::info!(
                    mina_core::log::system_time();
                    kind = "Reporting",
                    summary = "uptime dry run: request written, not sent",
                    path = path.display().to_string(),
                ),
                Err(error) => shared.ctx.status.dropped(
                    Reporter::Uptime,
                    format!("dry run: could not write the request: {error}"),
                ),
            }
            return None;
        }
        Some(
            shared
                .deliver(
                    Reporter::Uptime,
                    &self.config.url,
                    request.body,
                    shared.ctx.uptime_retry,
                    delivery::uptime_status_class,
                    interrupt,
                )
                .await,
        )
    }
}

/// Simplified node status, five slots after start and every five slots
/// after, a fresh sample each time, sent once.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_status_service/node_status_service.ml#L423-L445>
pub struct NodeStatusReporter<S> {
    shared: Arc<Shared<S>>,
    config: NodeStatusConfig,
    in_flight: AtomicBool,
}

impl<S: SnapshotSource> NodeStatusReporter<S> {
    pub async fn run(self: Arc<Self>) {
        let interval = schedule::status_interval(self.shared.ctx.slot_ms);
        let mut next = tokio::time::Instant::now() + interval;
        loop {
            tokio::time::sleep_until(next).await;
            next += interval;
            // In the background, as OCaml's `don't_wait_for`, but one at a
            // time: a send still in flight skips this report.
            if self.in_flight.swap(true, Ordering::SeqCst) {
                self.shared.skip(
                    Reporter::NodeStatus,
                    "the previous report is still in flight",
                );
                continue;
            }
            let this = self.clone();
            tokio::spawn(async move {
                this.send_once().await;
                this.in_flight.store(false, Ordering::SeqCst);
            });
        }
    }

    pub async fn send_once(&self) -> Option<Delivery> {
        let shared = &self.shared;
        let Some(snapshot) = shared.sample(shared.ctx.snapshot_timeout).await else {
            shared.skip(
                Reporter::NodeStatus,
                "the node did not answer the snapshot request",
            );
            return None;
        };
        let (Some(peer_id), Some(chain_id)) = (snapshot.peer_id.clone(), snapshot.chain_id.clone())
        else {
            shared.skip(
                Reporter::NodeStatus,
                "the node is not yet a peer in the gossip network",
            );
            return None;
        };
        let timestamp = shared.ctx.now();
        let report = node_status::SimplifiedStatus {
            max_observed_block_height: shared.heights.get().0,
            commit_hash: shared.ctx.commit_hash.clone(),
            chain_id: chain_id.clone(),
            peer_id,
            peer_count: snapshot.peer_count,
            timestamp: timestamp.clone(),
            block_producer_public_key: snapshot.block_producer.as_ref().map(|k| k.to_string()),
        };
        let body = report.body();
        shared.ctx.status.report(
            Reporter::NodeStatus,
            ReportIdentity {
                kind: NODE_STATUS_REPORT_KIND.to_owned(),
                created_at: timestamp,
                chain_id: Some(chain_id),
                state_hash: snapshot.best_tip.as_ref().map(|b| b.hash().to_string()),
                height: snapshot.best_tip.as_ref().map(|b| b.height()),
                global_slot: snapshot.best_tip.as_ref().map(|b| b.global_slot()),
                produced: false,
                body_bytes: body.len(),
                id: None,
            },
        );
        Some(
            shared
                .deliver(
                    Reporter::NodeStatus,
                    &self.config.url,
                    body,
                    RetryPolicy::ONCE,
                    delivery::status_status_class,
                    &Interruptor::new(),
                )
                .await,
        )
    }
}

/// Events waiting to be reported; a full queue drops new events.
const ERROR_QUEUE: usize = 16;
/// How long a crash waits for its report before the process goes on dying.
const CRASH_REPORT_WAIT: Duration = Duration::from_secs(10);
/// A crashing state machine doesn't answer: don't wait long for it.
const CRASH_SNAPSHOT_WAIT: Duration = Duration::from_secs(1);

struct QueuedError {
    event: NodeErrorEvent,
    done: Option<std::sync::mpsc::SyncSender<()>>,
}

/// Node error reports, one per explicit event: a fresh ID and timestamp
/// each, sent once, never retried, never merged with an equal event, and
/// dropped when larger than 10,000 bytes.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_error_service/node_error_service.ml#L54-L183>
pub struct NodeErrorReporter<S> {
    shared: Arc<Shared<S>>,
    config: NodeErrorConfig,
}

impl<S: SnapshotSource> NodeErrorReporter<S> {
    async fn run(self: Arc<Self>, mut events: tokio::sync::mpsc::Receiver<QueuedError>) {
        // Keep a recent sample for a crash, when the node may not answer.
        let refresh = Duration::from_millis(self.shared.ctx.slot_ms.max(1_000));
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                this.shared.sample(this.shared.ctx.snapshot_timeout).await;
                tokio::time::sleep(refresh).await;
            }
        });
        while let Some(QueuedError { event, done }) = events.recv().await {
            let wait = if done.is_some() {
                CRASH_SNAPSHOT_WAIT
            } else {
                self.shared.ctx.snapshot_timeout
            };
            self.report(&event, wait).await;
            if let Some(done) = done {
                let _ = done.try_send(());
            }
        }
        // No sender left (reporting started twice): keep the other reporters.
        futures::future::pending::<()>().await
    }

    /// Builds and sends one report.
    pub async fn report(
        &self,
        event: &NodeErrorEvent,
        snapshot_wait: Duration,
    ) -> Option<Delivery> {
        let shared = &self.shared;
        let snapshot = match shared.sample(snapshot_wait).await {
            Some(snapshot) => snapshot,
            None => match shared.cached() {
                Some(snapshot) => snapshot,
                None => {
                    shared.skip(Reporter::NodeError, "no node state available yet");
                    return None;
                }
            },
        };
        let Some(mut report) = self.base_report(&snapshot) else {
            shared.skip(
                Reporter::NodeError,
                "the node is not yet a peer in the gossip network",
            );
            return None;
        };
        report.error = event.error_json(report.error_budget());
        let body = report.body();
        shared.ctx.status.report(
            Reporter::NodeError,
            ReportIdentity {
                kind: event.kind().to_owned(),
                created_at: report.timestamp.clone(),
                chain_id: Some(report.chain_id.clone()),
                state_hash: snapshot.best_tip.as_ref().map(|b| b.hash().to_string()),
                height: report.block_height_at_best_tip,
                global_slot: snapshot.best_tip.as_ref().map(|b| b.global_slot()),
                produced: false,
                body_bytes: body.len(),
                id: Some(report.id.clone()),
            },
        );
        if body.len() > node_error::MAX_REPORT_BYTES {
            let reason = format!(
                "the {} report is {} bytes, over the {} byte limit: not sent",
                event.kind(),
                body.len(),
                node_error::MAX_REPORT_BYTES
            );
            shared
                .ctx
                .status
                .dropped(Reporter::NodeError, reason.clone());
            mina_core::error!(
                mina_core::log::system_time();
                kind = "Reporting",
                summary = reason,
            );
            return None;
        }
        Some(
            shared
                .deliver(
                    Reporter::NodeError,
                    &self.config.url,
                    body,
                    RetryPolicy::ONCE,
                    delivery::error_status_class,
                    &Interruptor::new(),
                )
                .await,
        )
    }

    /// OCaml's `generate_report` with an empty error: fresh timestamp and ID.
    pub fn base_report(&self, snapshot: &RpcReportingSnapshot) -> Option<NodeErrorReport> {
        let shared = &self.shared;
        let (validated, unvalidated) = shared.heights.get();
        Some(NodeErrorReport {
            peer_id: snapshot.peer_id.clone()?,
            ip_address: snapshot
                .external_ip
                .clone()
                .unwrap_or_else(|| "0.0.0.0".to_owned()),
            public_key: snapshot.block_producer.as_ref().map(|k| k.to_string()),
            commit_hash: shared.ctx.commit_hash.clone(),
            chain_id: snapshot.chain_id.clone()?,
            contact_info: self.config.contact_info.clone(),
            hardware_info: None,
            timestamp: shared.ctx.now(),
            id: shared.ctx.ids.new_id(),
            error: json::Json::str(""),
            catchup_job_states: None,
            sync_status: sync_status(snapshot),
            block_height_at_best_tip: snapshot.best_tip.as_ref().map(|b| b.height()),
            max_observed_block_height: validated,
            max_observed_unvalidated_block_height: unvalidated,
            uptime_of_node: schedule::span_to_string_hum(Duration::from_millis(
                shared
                    .ctx
                    .clock
                    .now_ms()
                    .saturating_sub(shared.ctx.started_ms),
            )),
        })
    }
}

/// OCaml's sync status names from this node's state. `Offline` (no gossip
/// for a while) isn't tracked here.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/sync_status/sync_status.ml#L17-L63>
pub fn sync_status(snapshot: &RpcReportingSnapshot) -> &'static str {
    match snapshot.sync_phase.as_deref() {
        Some("Synced") => "Synced",
        Some("Catchup") => "Catchup",
        Some("Bootstrap") => "Bootstrap",
        _ if snapshot.peer_count > 0 => "Listening",
        _ => "Connecting",
    }
}

/// The running node error reporter, for the panic hook and the ledger.
struct ErrorHandle {
    events: tokio::sync::mpsc::Sender<QueuedError>,
    status: StatusSink,
}

static ERROR_HANDLE: OnceLock<ErrorHandle> = OnceLock::new();
static CRASH_REPORTED: AtomicBool = AtomicBool::new(false);

fn enqueue_error(event: NodeErrorEvent, done: Option<std::sync::mpsc::SyncSender<()>>) -> bool {
    let Some(handle) = ERROR_HANDLE.get() else {
        return false;
    };
    let kind = event.kind();
    match handle.events.try_send(QueuedError { event, done }) {
        Ok(()) => true,
        Err(_) => {
            handle.status.dropped(
                Reporter::NodeError,
                format!("the {kind} event was dropped: the report queue is full"),
            );
            false
        }
    }
}

/// Reports the process's first panic, waiting up to ten seconds for the
/// report; later panics are only counted. For the CLI's panic hook.
pub fn report_panic(message: &str, location: &str, thread: &str) {
    let Some(handle) = ERROR_HANDLE.get() else {
        return;
    };
    if thread == REPORTING_THREAD {
        // The reporter can't report its own crash.
        return;
    }
    if CRASH_REPORTED.swap(true, Ordering::SeqCst) {
        handle.status.update(Reporter::NodeError, |s| {
            s.skipped = s.skipped.saturating_add(1);
            s.last_skip_reason = Some("a later panic: only the first is reported".to_owned());
        });
        return;
    }
    let (done, wait) = std::sync::mpsc::sync_channel(1);
    let event = NodeErrorEvent::Crash {
        message: format!("thread '{thread}' panicked: {message}"),
        location: location.to_owned(),
    };
    if enqueue_error(event, Some(done)) {
        let _ = wait.recv_timeout(CRASH_REPORT_WAIT);
    }
}

pub const REPORTING_THREAD: &str = "mina_reporting";

/// Starts the configured reporters on their own thread. Does nothing
/// without a URL.
pub fn start(
    rpc_sender: RpcSender,
    config: ReportingConfig,
    ctx: ReportingContext,
) -> anyhow::Result<()> {
    if !config.is_enabled() {
        return Ok(());
    }
    let prover = config
        .uptime
        .as_ref()
        .map(|_| uptime_prover::NativeUptimeProver::start())
        .transpose()?;
    let client = delivery::client(ctx.timeouts)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (events_tx, events_rx) = tokio::sync::mpsc::channel(ERROR_QUEUE);
    let status = ctx.status.clone();
    let shared = Arc::new(Shared {
        source: rpc_sender,
        ctx,
        client,
        heights: Default::default(),
        last_snapshot: Mutex::new(None),
    });
    let ReportingConfig {
        uptime,
        node_status,
        node_error,
    } = config;
    if let Some(config) = &uptime {
        let target = delivery::redacted_url(&config.url);
        status.enable(
            Reporter::Uptime,
            match config.dry_run_dir {
                Some(_) => format!("dry run, not sent to {target}"),
                None => target,
            },
            UPTIME_REPORT_KIND,
        );
        mina_core::info!(
            mina_core::log::system_time();
            kind = "Reporting",
            summary = match config.dry_run_dir {
                Some(_) => "starting the uptime reporter as a dry run: requests are written to files, not sent",
                None => "starting the uptime reporter with a dedicated SNARK prover",
            },
            url = delivery::redacted_url(&config.url),
            submitter = config.submitter.public_key().to_string(),
            network = format!("{:?}", shared.ctx.network),
            dry_run_dir = format!("{:?}", config.dry_run_dir),
        );
    }
    if let Some(config) = &node_status {
        status.enable(
            Reporter::NodeStatus,
            delivery::redacted_url(&config.url),
            NODE_STATUS_REPORT_KIND,
        );
        mina_core::info!(
            mina_core::log::system_time();
            kind = "Reporting",
            summary = "starting the simplified node status reporter",
            url = delivery::redacted_url(&config.url),
        );
    }
    if let Some(config) = &node_error {
        status.enable(
            Reporter::NodeError,
            delivery::redacted_url(&config.url),
            NODE_ERROR_REPORT_KIND,
        );
        mina_core::info!(
            mina_core::log::system_time();
            kind = "Reporting",
            summary = "starting the node error reporter",
            url = delivery::redacted_url(&config.url),
        );
        let _ = ERROR_HANDLE.set(ErrorHandle {
            events: events_tx,
            status: status.clone(),
        });
        mina_node::node_error_events::set_transaction_failure_sink(|failures| {
            enqueue_error(NodeErrorEvent::TransactionInclusionFailures(failures), None);
        });
    }
    mina_core::thread::Builder::new()
        .name(REPORTING_THREAD.to_owned())
        .spawn(move || {
            runtime.block_on(async move {
                if let Some(config) = uptime {
                    let reporter = Arc::new(UptimeReporter {
                        shared: shared.clone(),
                        config,
                        prover: Arc::new(prover.expect("uptime configured")),
                    });
                    tokio::spawn(reporter.run());
                }
                if let Some(config) = node_status {
                    let reporter = Arc::new(NodeStatusReporter {
                        shared: shared.clone(),
                        config,
                        in_flight: AtomicBool::new(false),
                    });
                    tokio::spawn(reporter.run());
                }
                match node_error {
                    Some(config) => {
                        let reporter = Arc::new(NodeErrorReporter {
                            shared: shared.clone(),
                            config,
                        });
                        reporter.run(events_rx).await;
                    }
                    None => futures::future::pending::<()>().await,
                }
            })
        })?;
    Ok(())
}
