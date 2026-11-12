use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, PoisonError},
};

use ledger::{
    scan_state::transaction_logic::WithStatus,
    transaction_pool::{TransactionError, TransactionPoolErrors},
    verifier::snark_work::{verify_snark_works, SnarkWorkError},
};
use mina_node::{
    core::{
        channels::mpsc,
        snark::{Snark, SnarkJobId},
        thread,
    },
    snark::{
        block_verify::{SnarkBlockVerifyError, SnarkBlockVerifyId, VerifiableBlockWithHash},
        work_verify::{SnarkWorkVerifyError, SnarkWorkVerifyId},
        BlockVerifier, SnarkEvent, TransactionVerifier, VerifierSRS,
    },
};
use rand::prelude::*;

use crate::NodeService;

use super::EventSender;

/// Runs `f`, and turns a panic into `None` (logged), so that a malformed input
/// from a peer can't abort the node from a rayon task.
fn catch_verifier_panic<T>(what: &str, f: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => Some(value),
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            mina_node::core::error!(
                summary = format!("{what} verifier panicked; treating the input as invalid"),
                error = message
            );
            None
        }
    }
}

type VerifyJob = Box<dyn FnOnce() + Send>;

/// Runs one service's verification jobs on the global rayon pool, one at a
/// time and in order.
///
/// A rayon thread that waits inside a job, in `join` or `par_iter`, runs other
/// queued pool tasks on its own stack meanwhile. With a pool task per request,
/// a burst of requests nests jobs inside each other as deep as the queue, until
/// the stack overflows; a 3-thread pool replaying the SNARK work of a mainnet
/// transaction burst this way reached 15 MiB of stack per thread. With one task
/// per service in the pool, the length of a service's queue no longer adds to
/// that nesting.
#[derive(Clone, Default)]
pub struct VerifyQueue(Arc<Mutex<VerifyQueueState>>);

#[derive(Default)]
struct VerifyQueueState {
    running: bool,
    pending: VecDeque<VerifyJob>,
}

impl VerifyQueue {
    pub fn push(&self, job: impl FnOnce() + Send + 'static) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.running {
            state.pending.push_back(Box::new(job));
            return;
        }
        state.running = true;
        drop(state);
        self.spawn(Box::new(job));
    }

    /// Runs `job` as a pool task, then hands the next pending job to a new
    /// task, so a long queue never nests on one stack.
    fn spawn(&self, job: VerifyJob) {
        let queue = self.clone();
        rayon::spawn_fifo(move || {
            job();
            let next = {
                let mut state = queue.0.lock().unwrap_or_else(PoisonError::into_inner);
                let next = state.pending.pop_front();
                state.running = next.is_some();
                next
            };
            if let Some(next) = next {
                queue.spawn(next);
            }
        });
    }
}

pub struct SnarkBlockVerifyArgs {
    pub req_id: SnarkBlockVerifyId,
    pub verifier_index: BlockVerifier,
    pub verifier_srs: Arc<VerifierSRS>,
    pub block: VerifiableBlockWithHash,
}

impl NodeService {
    pub fn snark_block_proof_verifier_spawn(
        event_sender: EventSender,
        skip_proof_verification: bool,
    ) -> mpsc::TrackedUnboundedSender<SnarkBlockVerifyArgs> {
        let (tx, mut rx) = mpsc::tracked_unbounded_channel();
        thread::Builder::new()
            .name("block_proof_verifier".to_owned())
            .spawn(move || {
                let verifier_key = BlockVerifier::source_sha256();
                while let Some(msg) = rx.blocking_recv() {
                    let SnarkBlockVerifyArgs {
                        req_id,
                        verifier_index,
                        verifier_srs,
                        block,
                    } = msg.0;
                    mina_core::debug!("verify({}) - start", block.hash_ref());
                    let header = block.header_ref();

                    if skip_proof_verification {
                        mina_core::debug!(
                            "verify({}) - skipping proof verification",
                            block.hash_ref()
                        );
                    }

                    let started = redux::Instant::now();
                    let verified = skip_proof_verification
                        || ledger::proofs::verification::verify_block(
                            header,
                            &verifier_index,
                            &verifier_srs,
                        );

                    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                    let result = if verified {
                        Ok(())
                    } else {
                        Err(SnarkBlockVerifyError::VerificationFailed)
                    };
                    mina_core::debug!("verify({}) - end", block.hash_ref());

                    let _ = event_sender.send(
                        SnarkEvent::BlockVerifyMeasured {
                            req_id,
                            result,
                            elapsed_ms,
                            skipped: skip_proof_verification,
                            verifier_key: verifier_key.clone(),
                        }
                        .into(),
                    );
                }
            })
            .expect("failed to spawn block_proof_verifier thread");

        tx
    }
}

impl mina_node::service::SnarkBlockVerifyService for NodeService {
    fn verify_init(
        &mut self,
        req_id: SnarkBlockVerifyId,
        verifier_index: BlockVerifier,
        verifier_srs: Arc<VerifierSRS>,
        block: VerifiableBlockWithHash,
    ) {
        if self.replayer.is_some() {
            return;
        }
        let args = SnarkBlockVerifyArgs {
            req_id,
            verifier_index,
            verifier_srs,
            block,
        };
        let _ = self.snark_block_proof_verify.tracked_send(args);
    }
}

impl mina_node::service::SnarkWorkVerifyService for NodeService {
    fn verify_init(
        &mut self,
        req_id: SnarkWorkVerifyId,
        verifier_index: TransactionVerifier,
        verifier_srs: Arc<VerifierSRS>,
        work: Vec<Snark>,
    ) {
        if self.replayer.is_some() {
            return;
        }
        let tx = self.event_sender().clone();
        self.snark_work_verify_queue.push(move || {
            // Keep SOK binding checks inside P6's panic containment.
            let result = catch_verifier_panic("snark work", || {
                // The sok digests are checked before any proof is verified: a
                // work whose fee or prover isn't the one its proofs were made for
                // is someone else's work re-advertised.
                verify_snark_works(&work, &verifier_index, &verifier_srs).map_err(|err| {
                    mina_core::warn!(
                        kind = "SnarkWorkVerify",
                        summary = format!("rejected a batch of {} snark works", work.len()),
                        error = err.to_string()
                    );
                    match err {
                        SnarkWorkError::SokDigestMismatch { .. } => {
                            SnarkWorkVerifyError::SokDigestMismatch
                        }
                        SnarkWorkError::InvalidInput { .. }
                        | SnarkWorkError::VerificationFailed => {
                            SnarkWorkVerifyError::VerificationFailed
                        }
                    }
                })
            })
            .unwrap_or(Err(SnarkWorkVerifyError::VerificationFailed));

            let _ = tx.send(SnarkEvent::WorkVerify(req_id, result).into());
        });
    }
}

impl mina_node::service::SnarkUserCommandVerifyService for NodeService {
    fn verify_init(
        &mut self,
        req_id: mina_node::snark::user_command_verify::SnarkUserCommandVerifyId,
        commands: Vec<WithStatus<ledger::scan_state::transaction_logic::verifiable::UserCommand>>,
    ) {
        if self.replayer.is_some() {
            return;
        }

        let tx = self.event_sender().clone();
        self.user_command_verify_queue.push(move || {
            // Commands come from peers and RPC callers; a panic while checking
            // a malformed one (a key or proof point off the curve, say) must
            // not abort the node, which rayon does on a panic in a task.
            let result = catch_verifier_panic("user command", || {
                let (verified, invalid): (Vec<_>, Vec<_>) = ledger::verifier::Verifier
                    .verify_commands(commands, None)
                    .into_iter()
                    .partition(Result::is_ok);

                let verified: Vec<_> = verified.into_iter().map(Result::unwrap).collect();
                let invalid: Vec<_> = invalid.into_iter().map(Result::unwrap_err).collect();

                if !invalid.is_empty() {
                    let transaction_pool_errors = invalid
                        .into_iter()
                        .map(TransactionError::Verifier)
                        .collect();
                    Err(TransactionPoolErrors::BatchedErrors(
                        transaction_pool_errors,
                    ))
                } else {
                    Ok(verified)
                }
            });

            let result = match result {
                Some(result) => result.map_err(|err| err.to_string()),
                None => Err("the verifier panicked on these commands".to_owned()),
            };

            let _ = tx.send(SnarkEvent::UserCommandVerify(req_id, result).into());
        });
    }
}

impl mina_node::service::SnarkPoolService for NodeService {
    fn random_choose<'a>(
        &mut self,
        iter: impl Iterator<Item = &'a SnarkJobId>,
        n: usize,
    ) -> Vec<SnarkJobId> {
        iter.choose_multiple(&mut self.rng, n)
            .into_iter()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod verify_queue_tests {
    use super::VerifyQueue;
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc, Arc, Mutex,
        },
        time::Duration,
    };

    #[test]
    fn runs_jobs_one_at_a_time_in_order() {
        const JOBS: usize = 64;
        let queue = VerifyQueue::default();
        let running = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();
        for i in 0..JOBS {
            let (running, most, order, tx) =
                (running.clone(), most.clone(), order.clone(), tx.clone());
            queue.push(move || {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(now, Ordering::SeqCst);
                // Wait inside the pool, as verification does in `join`.
                rayon::join(|| std::thread::sleep(Duration::from_millis(2)), || ());
                order.lock().unwrap().push(i);
                running.fetch_sub(1, Ordering::SeqCst);
                tx.send(()).unwrap();
            });
        }
        for _ in 0..JOBS {
            rx.recv_timeout(Duration::from_secs(60)).unwrap();
        }
        assert_eq!(most.load(Ordering::SeqCst), 1);
        assert_eq!(*order.lock().unwrap(), (0..JOBS).collect::<Vec<_>>());
    }

    #[test]
    fn concurrent_pushes_and_idle_restart() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 50;
        let queue = VerifyQueue::default();
        let running = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let job = |running: Arc<AtomicUsize>, most: Arc<AtomicUsize>, tx: mpsc::Sender<()>| {
            move || {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(now, Ordering::SeqCst);
                rayon::join(|| (), || ());
                running.fetch_sub(1, Ordering::SeqCst);
                tx.send(()).unwrap();
            }
        };
        // Pushes from several threads at once, racing with completions.
        std::thread::scope(|scope| {
            for _ in 0..THREADS {
                scope.spawn(|| {
                    for _ in 0..PER_THREAD {
                        queue.push(job(running.clone(), most.clone(), tx.clone()));
                    }
                });
            }
        });
        for _ in 0..THREADS * PER_THREAD {
            rx.recv_timeout(Duration::from_secs(60)).unwrap();
        }
        // An idle queue starts again on the next push.
        std::thread::sleep(Duration::from_millis(20));
        queue.push(job(running.clone(), most.clone(), tx.clone()));
        rx.recv_timeout(Duration::from_secs(60)).unwrap();
        assert_eq!(most.load(Ordering::SeqCst), 1);
        assert!(rx.try_recv().is_err());
    }
}
