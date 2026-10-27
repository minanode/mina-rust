use std::sync::Arc;

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
        rayon::spawn_fifo(move || {
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
        rayon::spawn_fifo(move || {
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
