//! A dedicated uptime worker: it never enters the block prover's queue or
//! Rayon pool. Only an uptime-configured node creates it. The reporter owns
//! cancellation: a superseded request loses its receiver and cannot be sent.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/uptime_service/uptime_snark_worker.ml#L138-L208>

use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    time::{Duration, Instant},
};

use futures::future::BoxFuture;
use ledger::{
    proofs::{
        catch_prover_panic,
        snark_worker::{is_zkapp_command, prove_uptime, uptime_target_matches, WorkProvers},
        verification::verify_transaction,
        verifiers::TransactionVerifier,
    },
    scan_state::scan_state::transaction_snark::{SokDigest, SokMessage, Statement},
};
use tokio::sync::{mpsc, oneshot};

use super::uptime_work::{ProofData, ProofOutcome, UptimeProofRequest, UptimeProver};

struct Job {
    request: UptimeProofRequest,
    reply: oneshot::Sender<ProofOutcome>,
}

pub struct NativeUptimeProver {
    sender: mpsc::Sender<Job>,
}

impl NativeUptimeProver {
    pub fn start() -> anyhow::Result<Self> {
        // At most one running job and one waiting for it. A cancelled
        // queued job is discarded before any circuits or proofs are used.
        let (sender, receiver) = mpsc::channel::<Job>(1);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .stack_size(64 * 1024 * 1024)
            .thread_name(|_| "uptime_proof".to_owned())
            .build()?;
        std::thread::Builder::new()
            .name("uptime_worker".to_owned())
            .spawn(move || {
                let mut circuits = CircuitLoader::new();
                let mut load = || {
                    let before = rss_kib();
                    let started = Instant::now();
                    let result = pool.install(|| {
                        let verifier = TransactionVerifier::make();
                        WorkProvers::make(Some(verifier.clone()))
                            .map(|p| (p, verifier))
                            .map_err(|error| format!("{error:#}"))
                    });
                    if result.is_ok() {
                        mina_core::info!(mina_core::log::system_time();
                            kind = "UptimeProversLoaded",
                            load_ms = started.elapsed().as_millis() as u64,
                            rss_before_kib = before.unwrap_or_default(),
                            rss_after_kib = rss_kib().unwrap_or_default(), loaded = true);
                    }
                    result
                };
                // Load eagerly off the reporter thread, but retain only a
                // bounded retry deadline on failure, never a permanent error.
                let _ = circuits.get(Instant::now(), &mut load, log_load_state);
                run_jobs(receiver, |request| {
                    match circuits.get(Instant::now(), &mut load, log_load_state) {
                        Err(error) => ProofOutcome::CircuitsUnavailable(error),
                        Ok((provers, verifier)) => {
                            pool.install(|| match prove_request(request, provers, verifier) {
                                Ok(proof) => ProofOutcome::Proved(Box::new(proof)),
                                Err(error) => ProofOutcome::Failed(format!("{error:#}")),
                            })
                        }
                    }
                });
            })?;
        Ok(Self { sender })
    }
    #[cfg(test)]
    pub(super) fn for_test(
        prove: impl FnMut(&UptimeProofRequest) -> ProofOutcome + Send + 'static,
    ) -> (Self, std::thread::JoinHandle<()>) {
        let (sender, receiver) = mpsc::channel(1);
        let handle = std::thread::spawn(move || run_jobs(receiver, prove));
        (Self { sender }, handle)
    }
}

/// Demand-driven retry: 30, 60, 120, 240, then at most 300 seconds.
/// The cap is below Mesa's 450-second reporting interval. Failed startup
/// therefore gets another chance at the next interval, without a busy loop.
struct CircuitLoader<T> {
    loaded: Option<T>,
    failure: Option<String>,
    retry_at: Option<Instant>,
    delay: Duration,
}

impl<T> CircuitLoader<T> {
    fn new() -> Self {
        Self {
            loaded: None,
            failure: None,
            retry_at: None,
            delay: Duration::from_secs(30),
        }
    }

    fn get(
        &mut self,
        now: Instant,
        mut load: impl FnMut() -> Result<T, String>,
        mut changed: impl FnMut(Option<&str>),
    ) -> Result<&T, String> {
        if self.loaded.is_none() && self.retry_at.is_none_or(|at| now >= at) {
            let result = catch_unwind(AssertUnwindSafe(&mut load))
                .unwrap_or_else(|_| Err("uptime circuit loading panicked".to_owned()));
            match result {
                Ok(value) => {
                    self.loaded = Some(value);
                    self.failure = None;
                    self.retry_at = None;
                    self.delay = Duration::from_secs(30);
                    changed(None);
                }
                Err(error) => {
                    if self.failure.as_ref() != Some(&error) {
                        changed(Some(&error));
                    }
                    self.failure = Some(error);
                    // Backoff starts after a potentially slow load, while the
                    // supplied clock still allows deterministic deadline tests.
                    self.retry_at = Some(now.max(Instant::now()) + self.delay);
                    self.delay = (self.delay * 2).min(Duration::from_secs(300));
                }
            }
        }
        self.loaded
            .as_ref()
            .ok_or_else(|| self.failure.clone().unwrap_or_default())
    }
}

fn log_load_state(error: Option<&str>) {
    match error {
        Some(error) => mina_core::error!(mina_core::log::system_time();
            kind = "UptimeProversLoadFailed", error = error,
            summary = "uptime circuits unavailable; later reports will retry"),
        None => {}
    }
}

fn run_jobs(
    mut receiver: mpsc::Receiver<Job>,
    mut prove: impl FnMut(&UptimeProofRequest) -> ProofOutcome,
) {
    while let Some(job) = receiver.blocking_recv() {
        if job.reply.is_closed() {
            continue;
        }
        let before = rss_kib();
        let started = std::time::Instant::now();
        // A panic costs this job; if it poisoned the proving thread's SRS,
        // the next proof makes it again.
        let outcome =
            catch_prover_panic("uptime", || prove(&job.request)).unwrap_or_else(|message| {
                ProofOutcome::Failed(format!("uptime proving panicked: {message}"))
            });
        mina_core::info!(mina_core::log::system_time();
                        kind = "UptimeProofCompleted",
                        state_hash = job.request.best_tip.hash().to_string(),
                        zkapp = job.request.job.zkapp,
                        wall_ms = started.elapsed().as_millis() as u64,
                        proof_seconds = match &outcome {
                            ProofOutcome::Proved(proof) => proof.proof_time,
                            _ => 0.0,
                        },
                        rss_before_kib = before.unwrap_or_default(),
                        rss_after_kib = rss_kib().unwrap_or_default(),
                        cancelled = job.reply.is_closed(),
                        proved = matches!(outcome, ProofOutcome::Proved(_)));
        let _ = job.reply.send(outcome);
    }
}

impl UptimeProver for NativeUptimeProver {
    fn prove(&self, request: UptimeProofRequest) -> BoxFuture<'_, ProofOutcome> {
        Box::pin(async move {
            let (reply, response) = oneshot::channel();
            if self.sender.try_send(Job { request, reply }).is_err() {
                return ProofOutcome::Failed("uptime worker unavailable or busy".to_owned());
            }
            response.await.unwrap_or_else(|_| {
                ProofOutcome::Failed("uptime worker stopped without a proof".to_owned())
            })
        })
    }
}

/// Prove the frozen witness and verify the result before encoding it. zkApp
/// uptime preserves OCaml's zero SOK; verification here checks the proof of
/// that statement, not the delegation service's separate fee/SOK policy.
pub fn prove_request(
    request: &UptimeProofRequest,
    provers: &WorkProvers,
    verifier: &TransactionVerifier,
) -> anyhow::Result<ProofData> {
    let spec = request
        .spec
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("uptime witness missing"))?;
    let target = &request
        .best_tip
        .staged_ledger_hashes()
        .non_snark
        .ledger_hash;
    anyhow::ensure!(
        uptime_target_matches(spec, target),
        "uptime job target differs from block ledger"
    );
    let zkapp = is_zkapp_command(spec);
    anyhow::ensure!(
        zkapp == request.job.zkapp,
        "uptime job kind differs from witness"
    );
    let submitter = mina_signer::CompressedPubKey::from_address(&request.submitter.to_string())
        .map_err(|_| anyhow::anyhow!("invalid uptime submitter public key"))?;
    let message = SokMessage::create((&request.snark_work_fee).into(), submitter);
    let (proof, proof_time) = prove_uptime(spec, target, &message, provers)?;
    anyhow::ensure!(
        &proof.0.statement.target.second_pass_ledger == target,
        "uptime proof target differs from block ledger"
    );
    let statement = Statement::<SokDigest>::try_from(&proof.0.statement)
        .map_err(|_| anyhow::anyhow!("invalid uptime proof statement"))?;
    if !zkapp {
        if let mina_p2p_messages::v2::SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single::Transition(expected, _) = spec {
            let expected = Statement::<()>::try_from(&expected.0)
                .map_err(|_| anyhow::anyhow!("invalid uptime job statement"))?;
            anyhow::ensure!(statement.clone().without_digest() == expected,
                "uptime proof statement differs from job");
        }
    }
    let digest = if zkapp {
        SokDigest::default()
    } else {
        message.digest()
    };
    anyhow::ensure!(
        statement.sok_digest == digest,
        "uptime proof SOK differs from request"
    );
    let raw_proof = proof
        .0
        .proof
        .get()
        .map_err(|_| anyhow::anyhow!("uptime proof unavailable"))?;
    anyhow::ensure!(
        verify_transaction(
            [(&statement, &*raw_proof)],
            verifier,
            &ledger::verifier::get_srs::<mina_curves::pasta::Fp>(),
        ),
        "uptime proof failed verification"
    );
    Ok(ProofData {
        proof,
        proof_time,
        snark_work_fee: request.snark_work_fee.clone(),
    })
}

fn rss_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .trim()
        .trim_end_matches(" kB")
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circuit_load_recovers_at_next_interval_and_logs_state_changes_once() {
        let mut loader = CircuitLoader::new();
        let start = Instant::now() + Duration::from_secs(3600);
        let mut events = Vec::new();
        let mut calls = 0;
        let mut load = || {
            calls += 1;
            Err::<(), _>("missing circuit".to_owned())
        };
        let mut log = |error: Option<&str>| events.push(error.map(str::to_owned));
        assert!(loader.get(start, &mut load, &mut log).is_err());
        assert!(loader
            .get(start + Duration::from_secs(29), &mut load, &mut log)
            .is_err());
        assert!(loader
            .get(start + Duration::from_secs(450), &mut load, &mut log)
            .is_err());
        assert_eq!(calls, 2);
        assert!(loader
            .get(start + Duration::from_secs(900), || Ok(()), &mut log)
            .is_ok());
        assert!(loader
            .get(
                start + Duration::from_secs(1350),
                || panic!("must reuse circuits"),
                &mut log
            )
            .is_ok());
        assert_eq!(events, [Some("missing circuit".to_owned()), None]);
    }

    #[test]
    fn circuit_backoff_is_bounded_and_panics_are_retryable() {
        let mut loader = CircuitLoader::<()>::new();
        let mut now = Instant::now() + Duration::from_secs(3600);
        let mut events = Vec::new();
        for delay in [30, 60, 120, 240, 300, 300] {
            assert!(loader
                .get(
                    now,
                    || Err("missing".to_owned()),
                    |e| events.push(e.map(str::to_owned))
                )
                .is_err());
            assert_eq!(
                loader.retry_at.unwrap().duration_since(now),
                Duration::from_secs(delay)
            );
            now = loader.retry_at.unwrap();
        }
        assert!(loader
            .get(
                now,
                || panic!("load failure"),
                |e| events.push(e.map(str::to_owned))
            )
            .is_err());
        now = loader.retry_at.unwrap();
        assert!(loader
            .get(now, || Ok(()), |e| events.push(e.map(str::to_owned)))
            .is_ok());
        assert_eq!(
            events,
            [
                Some("missing".to_owned()),
                Some("uptime circuit loading panicked".to_owned()),
                None
            ]
        );
    }
}
