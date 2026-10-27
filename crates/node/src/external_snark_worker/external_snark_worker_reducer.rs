use redux::Timestamp;

use super::{
    available_job_to_snark_worker_spec,
    external_snark_worker_state::{ExternalSnarkWorker, ExternalSnarkWorkerState},
    worker_control::push_outcome,
    ExternalSnarkWorkerAction, ExternalSnarkWorkerActionWithMetaRef, ExternalSnarkWorkerWorkError,
    ExternalSnarkWorkers, WorkerJob, WorkerOutcome,
};
use crate::{
    external_snark_worker_effectful::ExternalSnarkWorkerEffectfulAction, SnarkPoolAction, Substate,
};

impl ExternalSnarkWorkers {
    pub fn reducer(
        state_context: Substate<ExternalSnarkWorkers>,
        action: ExternalSnarkWorkerActionWithMetaRef<'_>,
    ) {
        ExternalSnarkWorker::reducer(Substate::from_compatible_substate(state_context), action);
    }
}

impl ExternalSnarkWorker {
    pub fn reducer(
        mut state_context: Substate<ExternalSnarkWorker>,
        action: ExternalSnarkWorkerActionWithMetaRef<'_>,
    ) {
        let Ok(worker_state) = state_context.get_substate_mut() else {
            return;
        };
        let (action, meta) = action.split();
        match action {
            ExternalSnarkWorkerAction::Start => {
                worker_state.state = ExternalSnarkWorkerState::Starting;
                worker_state.update_timestamp(meta.time());
                worker_state.job = None;

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let Some(config) = &state.config.snarker else {
                    return;
                };

                let public_key = config.public_key.clone().into();
                let fee = config.fee.clone();

                dispatcher.push(ExternalSnarkWorkerEffectfulAction::Start { public_key, fee });
            }
            ExternalSnarkWorkerAction::Started => {
                worker_state.state = ExternalSnarkWorkerState::Idle;
                worker_state.update_timestamp(meta.time());

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(SnarkPoolAction::AutoCreateCommitment);
            }
            ExternalSnarkWorkerAction::StartTimeout { .. } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(ExternalSnarkWorkerAction::Error {
                    error: super::ExternalSnarkWorkerError::StartTimeout,
                    permanent: true,
                });
            }
            ExternalSnarkWorkerAction::Kill => {
                worker_state.state = ExternalSnarkWorkerState::Killing;
                worker_state.update_timestamp(meta.time());

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(ExternalSnarkWorkerEffectfulAction::Kill);
            }
            ExternalSnarkWorkerAction::Killed => {
                worker_state.state = ExternalSnarkWorkerState::None;
                worker_state.update_timestamp(meta.time());
                worker_state.job = None;
            }
            ExternalSnarkWorkerAction::Error { error, permanent } => {
                worker_state.state = ExternalSnarkWorkerState::Error(error.clone(), *permanent);
                worker_state.update_timestamp(meta.time());
                // No job is being proved any more (P5.4).
                worker_state.job = None;

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(ExternalSnarkWorkerAction::Kill);
            }
            ExternalSnarkWorkerAction::SubmitWork {
                job_id,
                summary,
                zkapp,
            } => {
                worker_state.state =
                    ExternalSnarkWorkerState::Working(job_id.clone(), summary.clone());
                worker_state.update_timestamp(meta.time());
                worker_state.job = Some(WorkerJob {
                    job_id: job_id.clone(),
                    summary: summary.clone(),
                    zkapp: *zkapp,
                    submitted: meta.time(),
                });

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let Some(job) = state.snark_pool.get(job_id) else {
                    return;
                };

                match available_job_to_snark_worker_spec(
                    job.job.clone(),
                    &state.transition_frontier,
                ) {
                    Ok(spec) => {
                        dispatcher.push(ExternalSnarkWorkerEffectfulAction::SubmitWork {
                            spec: Box::new(spec),
                        });
                    }
                    Err(err) => {
                        dispatcher.push(ExternalSnarkWorkerAction::WorkError {
                            error: ExternalSnarkWorkerWorkError::WorkSpecError(err),
                        });
                    }
                }
            }
            ExternalSnarkWorkerAction::WorkResult { result } => {
                // The proof's own job: a proof that finished after its job's
                // cancel ended (the worker moved on) is still offered, and
                // leaves the worker's current job alone.
                let result_job_id = result.job_id();
                // O2's proofs made: every proof our worker returns, also one
                // that finishes while the worker is stopped or re-priced.
                worker_state.proofs_made = worker_state.proofs_made.saturating_add(1);
                let current = matches!(
                    &worker_state.state,
                    ExternalSnarkWorkerState::Working(job_id, _)
                        | ExternalSnarkWorkerState::Cancelling(job_id)
                        if *job_id == result_job_id
                );
                let job = worker_state.job.take_if(|job| job.job_id == result_job_id);
                if current {
                    worker_state.state = ExternalSnarkWorkerState::WorkReady(
                        result_job_id.clone(),
                        result.proofs.clone(),
                    );
                    worker_state.update_timestamp(meta.time());
                }
                let zkapp = job.as_ref().is_some_and(|job| job.zkapp);
                let duration_ms = job.as_ref().map(|job| {
                    meta.time()
                        .checked_sub(job.submitted)
                        .unwrap_or_default()
                        .as_millis() as u64
                });

                // Offered only if it was made for the key and fee the worker
                // has now (P5.4): a stopped worker's proof, or one for a
                // setting since changed, goes nowhere; its job was released
                // at the change. Admission checks again
                // (`SnarkPoolAction::ProverAccountResult`).
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let offered = state.snark_worker_authorizes(result);
                let outcome = WorkerOutcome {
                    job_id: result.job_id(),
                    summary: job.map(|job| job.summary),
                    zkapp,
                    time: meta.time(),
                    duration_ms,
                    fee: Some(result.fee.0.as_u64()),
                    offered,
                    error: None,
                };
                dispatcher.push(ExternalSnarkWorkerAction::Outcome {
                    outcome: Box::new(outcome),
                });
                if current {
                    dispatcher.push(ExternalSnarkWorkerAction::PruneWork);
                }
                if !offered {
                    mina_core::warn!(meta.time();
                        kind = "SnarkWorkerProofDropped",
                        summary = "a proof made for a stopped worker or a former key or fee is not offered",
                        job_id = display(&result_job_id));
                    return;
                }
                let Some(p2p) = state.p2p.ready() else {
                    return;
                };
                let sender = p2p.my_id();
                // Locally proved work still needs current pool admission checks.
                dispatcher.push(SnarkPoolAction::WorkCheck {
                    work: result.clone(),
                    stage: crate::snark_pool::AdmissionStage::Verified,
                    retry: false,
                    sender,
                    is_sender_local: true,
                });
            }
            ExternalSnarkWorkerAction::Outcome { outcome } => {
                if outcome.offered && outcome.zkapp {
                    worker_state.included.note_zkapp_job(&outcome.job_id);
                }
                push_outcome(&mut worker_state.recent, (**outcome).clone());
            }
            ExternalSnarkWorkerAction::WorkError { error } => {
                let (ExternalSnarkWorkerState::Working(job_id, _)
                | ExternalSnarkWorkerState::Cancelling(job_id)) = &worker_state.state
                else {
                    return;
                };
                let job_id = job_id.clone();
                worker_state.proofs_failed = worker_state.proofs_failed.saturating_add(1);
                let job = worker_state.job.take_if(|job| job.job_id == job_id);
                push_outcome(
                    &mut worker_state.recent,
                    WorkerOutcome {
                        job_id: job_id.clone(),
                        summary: job.as_ref().map(|job| job.summary.clone()),
                        zkapp: job.as_ref().is_some_and(|job| job.zkapp),
                        time: meta.time(),
                        duration_ms: job.as_ref().map(|job| {
                            meta.time()
                                .checked_sub(job.submitted)
                                .unwrap_or_default()
                                .as_millis() as u64
                        }),
                        fee: None,
                        offered: false,
                        error: Some(error.to_string()),
                    },
                );
                worker_state.state = ExternalSnarkWorkerState::WorkError(job_id, error.clone());
                worker_state.update_timestamp(meta.time());

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(ExternalSnarkWorkerAction::PruneWork);
            }
            ExternalSnarkWorkerAction::WorkTimeout { .. } => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(ExternalSnarkWorkerAction::CancelWork);
            }
            ExternalSnarkWorkerAction::CancelWork => {
                let ExternalSnarkWorkerState::Working(job_id, _) = &worker_state.state else {
                    return;
                };
                worker_state.state = ExternalSnarkWorkerState::Cancelling(job_id.clone());
                worker_state.update_timestamp(meta.time());

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(ExternalSnarkWorkerEffectfulAction::CancelWork);
            }
            ExternalSnarkWorkerAction::WorkCancelled => {
                let ExternalSnarkWorkerState::Cancelling(job_id) = &worker_state.state else {
                    return;
                };
                worker_state.state = ExternalSnarkWorkerState::Cancelled(job_id.clone());
                worker_state.update_timestamp(meta.time());

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(ExternalSnarkWorkerAction::PruneWork);
            }
            ExternalSnarkWorkerAction::PruneWork => {
                worker_state.state = ExternalSnarkWorkerState::Idle;
                worker_state.update_timestamp(meta.time());

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(SnarkPoolAction::AutoCreateCommitment);
            }
            // On the global state: `State::snark_worker_configure`.
            ExternalSnarkWorkerAction::Configure { .. } => {}
        }
    }

    fn update_timestamp(&mut self, time: Timestamp) {
        self.timestamp = time;
    }
}
