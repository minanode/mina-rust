use super::{
    check_prover, AdmissionError, AdmissionStage, ProverAccount, ProverAccountRequest, WorkSource,
};
use crate::ledger::read::{LedgerReadAction, LedgerReadInitCallback, LedgerReadRequest};
use std::collections::BTreeMap;

use crate::{
    p2p::channels::{
        snark::P2pChannelsSnarkAction, snark_job_commitment::P2pChannelsSnarkJobCommitmentAction,
    },
    snark_pool::JobCommitment,
    ExternalSnarkWorkerAction, SnarkerStrategy,
};
use mina_core::snark::{SnarkJobCommitment, SnarkJobId};

use super::{
    JobState, SnarkPoolAction, SnarkPoolActionWithMetaRef, SnarkPoolEffectfulAction,
    SnarkPoolState, SnarkWork,
};

/// A rejected peer's SNARK work is per-proof detail: it can be rejected many
/// times a minute (O1/P5.4). Our worker's is said once more, at WARN, by
/// [`SnarkPoolState::own_work_rejected`] (SNARK-POOL-OWN-WORK).
macro_rules! admission_rejected {
    ($($tts:tt)*) => {{
        // A rejection describes one proof and can repeat each time work is
        // offered. Keep it with other per-proof detail at DEBUG (O1/P5.4).
        mina_core::debug!($($tts)*)
    }};
}

impl SnarkPoolState {
    /// Our worker's proof was not admitted: says why, once per proof
    /// (SNARK-POOL-OWN-WORK). A proof as cheap stored first is a race lost,
    /// not a fault. The job keeps our commitment, so the worker does not
    /// prove it again: OCaml's selector keeps a job it handed out in
    /// `jobs_scheduled` until the job leaves the best tip's available jobs,
    /// also when the pool refuses its proof, and its reassignment wait sends
    /// again only work still out at a worker, which a submitted result ends.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/work_selector/work_lib.ml#L142-L173>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/work_partitioner/work_partitioner.ml#L58-L86>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/work_partitioner/work_partitioner.ml#L306-L326>
    fn own_work_rejected(
        work: &mina_core::snark::Snark,
        stage: AdmissionStage,
        error: AdmissionError,
        time: redux::Timestamp,
    ) {
        let job_id = work.job_id();
        if error == AdmissionError::NotCheaper {
            mina_core::info!(time;
                kind = "SnarkWorkerProofRejected",
                summary = "the pool kept a proof as cheap as our worker's",
                job_id = display(&job_id),
                stage = debug(stage),
                reason = debug(error));
        } else {
            mina_core::warn!(time;
                kind = "SnarkWorkerProofRejected",
                summary = "the pool refused our worker's proof",
                job_id = display(&job_id),
                stage = debug(stage),
                reason = debug(error));
        }
    }

    /// The account of a prover of stored works, read at a new best tip
    /// (`JobsUpdate` reads each prover once). As OCaml does at a new best tip,
    /// its works that the prover rule refuses for the account are dropped
    /// and the others kept. A failed read (busy, or no ledger) says nothing
    /// about the prover: its works are kept, unused until a read succeeds
    /// (`CheckTimeouts` reads again). Dropping them there lost our worker's
    /// proofs while its commitments kept their jobs (SNARK-POOL-OWN-WORK).
    /// A job whose work is dropped keeps our commitment, as OCaml's selector
    /// keeps the job scheduled ([`Self::own_work_rejected`]).
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/network_pool/snark_pool.ml#L167-L198>
    fn revalidated(
        &mut self,
        request: &super::ProverAccountRequest,
        result: &Result<ProverAccount, String>,
        own_key: Option<&mina_p2p_messages::v2::NonZeroCurvePoint>,
        time: redux::Timestamp,
    ) {
        let prover = &request.work.snarker;
        let own = own_key == Some(prover);
        let account = match result {
            Ok(account) => *account,
            Err(error) => {
                self.admission_stats.revalidation_deferred += 1;
                // Once per best tip: the periodic check reads again.
                if self.revalidation_deferred_tip.as_ref() != Some(&request.best_tip) {
                    self.revalidation_deferred_tip = Some(request.best_tip.clone());
                    mina_core::warn!(time;
                        kind = "SnarkPoolRevalidationDeferred",
                        summary = "kept a prover's stored works unchecked: its account could not be read at the new best tip",
                        prover = display(prover),
                        own = own,
                        error = display(error));
                }
                return;
            }
        };
        self.cache_prover_account(prover.clone().into(), request.best_tip.clone(), account);
        let refused = self.refused_works_of(prover, account);
        for (job_id, error) in &refused {
            self.admission_stats
                .rejected(AdmissionStage::Revalidate, *error);
            self.remove_snark_work(job_id);
            if own {
                mina_core::warn!(time;
                    kind = "SnarkWorkerProofRemoved",
                    summary = "the pool dropped our worker's proof at a new best tip",
                    job_id = display(job_id),
                    reason = debug(error));
            }
        }
        if !own && !refused.is_empty() {
            mina_core::debug!(time;
                kind = "SnarkPoolRevalidationRemoved",
                summary = "dropped a prover's stored works at a new best tip",
                prover = display(prover),
                works = refused.len());
        }
    }
}

impl SnarkPoolState {
    pub fn reducer(
        mut state_context: crate::Substate<Self>,
        action: SnarkPoolActionWithMetaRef<'_>,
    ) {
        let tip = state_context
            .unsafe_get_state()
            .transition_frontier
            .best_tip()
            .cloned();
        // Whether our worker's work is still ours to offer (P5.4): the
        // operator may stop or re-price it while its admission waits.
        let own_work_authorized = |work: &mina_core::snark::Snark| {
            state_context
                .unsafe_get_state()
                .snark_worker_authorizes(work)
        };
        let own_work_dropped = match action.action() {
            SnarkPoolAction::ProverAccountResult { request, .. } if request.is_sender_local => {
                !own_work_authorized(&request.work)
            }
            _ => false,
        };
        // Our worker's key: says which works and commitments are ours.
        let own_key = state_context
            .unsafe_get_state()
            .config
            .snarker
            .as_ref()
            .map(|config| {
                let key: &mina_p2p_messages::v2::NonZeroCurvePoint = config.public_key.as_ref();
                key.clone()
            });
        let own_key = own_key.as_ref();
        let Ok(state) = state_context.get_substate_mut() else {
            // TODO: log or propagate
            return;
        };
        let (action, meta) = action.split();

        match action {
            SnarkPoolAction::WorkCheck {
                work,
                sender,
                is_sender_local,
                stage,
                retry,
            } => {
                if let AdmissionStage::Received(source) = stage {
                    if !retry {
                        state.admission_stats.received(*source);
                        mina_core::debug!(meta.time(); kind = "SnarkPoolAdmissionReceived", source = debug(source), job_id = display(work.job_id()));
                    }
                }
                // Reject competing fees before even looking up the account.
                // Revalidation checks the already-stored work against a new tip.
                if *stage != AdmissionStage::Revalidate && !state.work_fee_is_competitive(work) {
                    state
                        .admission_stats
                        .rejected(*stage, AdmissionError::NotCheaper);
                    admission_rejected!(meta.time(); kind = "SnarkPoolAdmissionRejected", stage = debug(stage), reason = debug(AdmissionError::NotCheaper), job_id = display(work.job_id()));
                    if *is_sender_local {
                        Self::own_work_rejected(
                            work,
                            *stage,
                            AdmissionError::NotCheaper,
                            meta.time(),
                        );
                    }
                    return;
                }
                let Some(tip) = tip
                    .as_ref()
                    .filter(|tip| state.jobs_tip.as_ref() == Some(tip.hash()))
                else {
                    let error = check_prover(
                        work.fee.0.as_u64(),
                        mina_core::constants::constraint_constants().account_creation_fee,
                        ProverAccount::Missing,
                    )
                    .err()
                    .unwrap_or(AdmissionError::NoFrontier);
                    if *stage == AdmissionStage::Revalidate {
                        // Nothing is removed: a later tip revalidates.
                        state.admission_stats.revalidation_deferred += 1;
                    } else {
                        state.admission_stats.rejected(*stage, error);
                    }
                    admission_rejected!(meta.time(); kind = "SnarkPoolAdmissionRejected", stage = debug(stage), reason = debug(error), job_id = display(work.job_id()));
                    if *is_sender_local {
                        Self::own_work_rejected(work, *stage, error, meta.time());
                    }
                    return;
                };
                state.account_request_id = state.account_request_id.wrapping_add(1);
                let request = Box::new(ProverAccountRequest {
                    id: state.account_request_id,
                    best_tip: tip.hash().clone(),
                    ledger_hash: tip.merkle_root_hash().clone(),
                    work: work.clone(),
                    sender: *sender,
                    is_sender_local: *is_sender_local,
                    stage: *stage,
                });
                let cached = state
                    .prover_accounts
                    .get(&work.snarker.clone().into())
                    .filter(|(hash, _)| hash == tip.hash())
                    .map(|(_, account)| *account);
                let (dispatcher, global) = state_context.into_dispatcher_and_state();
                if let Some(account) = cached {
                    dispatcher.push(SnarkPoolAction::ProverAccountResult {
                        request,
                        result: Ok(account),
                    });
                } else {
                    let read = LedgerReadRequest::SnarkPoolProver(request.clone());
                    // Admitted over the limit for our worker's proof.
                    if global.ledger.read.can_admit(&read) {
                        dispatcher.push(LedgerReadAction::Init {
                            request: read,
                            callback: LedgerReadInitCallback::None,
                        });
                    } else {
                        dispatcher.push(SnarkPoolAction::ProverAccountResult {
                            request,
                            result: Err("ledger read capacity exceeded".into()),
                        });
                    }
                }
            }
            SnarkPoolAction::ProverAccountResult { request, result } => {
                if own_work_dropped {
                    // Its job is open again for the worker as it is now: our
                    // commitment, which never times out, would keep it.
                    let job_id = request.work.job_id();
                    let ours = state
                        .get(&job_id)
                        .and_then(|job| job.commitment.as_ref())
                        .is_some_and(|c| c.commitment.snarker == request.work.snarker);
                    if ours {
                        state.remove_commitment(&job_id);
                    }
                    mina_core::warn!(meta.time();
                        kind = "SnarkWorkerProofDropped",
                        summary = "our proof was not admitted: the worker was stopped or re-priced while it waited",
                        job_id = display(job_id));
                    return;
                }
                if tip.as_ref().map(|tip| tip.hash()) != Some(&request.best_tip)
                    || state.jobs_tip.as_ref() != Some(&request.best_tip)
                {
                    state.admission_stats.stale_account_results += 1;
                    state_context
                        .into_dispatcher()
                        .push(SnarkPoolAction::WorkCheck {
                            work: request.work.clone(),
                            sender: request.sender,
                            is_sender_local: request.is_sender_local,
                            stage: request.stage,
                            retry: true,
                        });
                    return;
                }
                if request.stage == AdmissionStage::Revalidate {
                    state.revalidated(request, result, own_key, meta.time());
                    return;
                }
                // Another proof may have been stored while the ledger read
                // waited. Recheck before inspecting or caching its account.
                if !state.work_fee_is_competitive(&request.work) {
                    state
                        .admission_stats
                        .rejected(request.stage, AdmissionError::NotCheaper);
                    admission_rejected!(meta.time(); kind = "SnarkPoolAdmissionRejected", stage = debug(request.stage), reason = debug(AdmissionError::NotCheaper), job_id = display(request.work.job_id()));
                    if request.is_sender_local {
                        Self::own_work_rejected(
                            &request.work,
                            request.stage,
                            AdmissionError::NotCheaper,
                            meta.time(),
                        );
                    }
                    return;
                }
                let checked = result
                    .as_ref()
                    .map_err(|error| {
                        if error == "ledger read capacity exceeded" {
                            AdmissionError::Busy
                        } else {
                            AdmissionError::AccountUnavailable
                        }
                    })
                    .and_then(|account| {
                        check_prover(
                            request.work.fee.0.as_u64(),
                            mina_core::constants::constraint_constants().account_creation_fee,
                            *account,
                        )
                    })
                    .and_then(|()| {
                        state
                            .work_is_referenced(&request.work, &request.best_tip)
                            .then_some(())
                            .ok_or(AdmissionError::WorkNotReferenced)
                    });
                if let Err(error) = checked {
                    state.admission_stats.rejected(request.stage, error);
                    // These context/fee failures are not invalid proofs. Do not
                    // disconnect a peer for them.
                    state
                        .candidates
                        .peer_work_remove(request.sender, &request.work.job_id());
                    admission_rejected!(meta.time(); kind = "SnarkPoolAdmissionRejected", stage = debug(request.stage), reason = debug(error), job_id = display(request.work.job_id()));
                    if request.is_sender_local {
                        if let Ok(account) = result {
                            state.warn_own_fee_refused(&request.work, *account, meta.time());
                        }
                        Self::own_work_rejected(&request.work, request.stage, error, meta.time());
                    }
                    return;
                }
                if let Ok(account) = result {
                    state.cache_prover_account(
                        request.work.snarker.clone().into(),
                        request.best_tip.clone(),
                        *account,
                    );
                }
                if matches!(request.stage, AdmissionStage::Received(_)) {
                    state.admission_stats.candidates += 1;
                }
                let dispatcher = state_context.into_dispatcher();
                match request.stage {
                    AdmissionStage::Received(_) => {
                        dispatcher.push(
                            super::candidate::SnarkPoolCandidateAction::WorkFetchSuccess {
                                peer_id: request.sender,
                                work: request.work.clone(),
                            },
                        );
                        dispatcher.push(super::candidate::SnarkPoolCandidateAction::WorkVerifyNext);
                    }
                    AdmissionStage::Verified => {
                        dispatcher.push(SnarkPoolAction::WorkAdd {
                            snark: request.work.clone(),
                            sender: request.sender,
                            is_sender_local: request.is_sender_local,
                        });
                    }
                    AdmissionStage::Revalidate => {}
                }
            }
            SnarkPoolAction::WorkBroadcast { snark, is_local } => {
                state.admission_stats.forwarded += 1;
                mina_core::debug!(meta.time(); kind = "SnarkPoolAdmissionForwarded", job_id = display(snark.job_id()));
                state_context
                    .into_dispatcher()
                    .push(P2pChannelsSnarkAction::Libp2pBroadcast {
                        snark: snark.clone(),
                        nonce: 0,
                        is_local: *is_local,
                    });
            }
            SnarkPoolAction::Candidate(action) => {
                if let super::candidate::SnarkPoolCandidateAction::WorkVerifyError {
                    batch, ..
                } = action
                {
                    state.admission_stats.failed_verification_batches += 1;
                    state.admission_stats.works_in_failed_verification_batches +=
                        batch.len() as u64;
                }
                super::candidate::SnarkPoolCandidatesState::reducer(
                    crate::Substate::from_compatible_substate(state_context),
                    meta.with_action(action),
                );
            }
            SnarkPoolAction::JobsUpdate {
                jobs,
                orphaned_snarks,
            } => {
                state.jobs_tip = tip.as_ref().map(|tip| tip.hash().clone());
                state.prover_accounts.clear();
                let mut jobs_map = jobs
                    .iter()
                    .enumerate()
                    .map(|(index, job)| (SnarkJobId::from(job), (index, job.clone())))
                    .collect::<BTreeMap<_, _>>();

                state.retain(|old| {
                    let (_, new) = jobs_map.get(&old.id)?;
                    let statement = super::snark_pool_state::job_statement(new)?;
                    if super::snark_pool_state::job_statement(&old.job).as_ref() != Some(&statement)
                    {
                        return None;
                    }
                    jobs_map.remove(&old.id).map(|(order, _)| order)
                });

                // Drop candidates for replaced statements before reusing their IDs.
                state.candidates_prune();
                for (id, (order, job)) in jobs_map {
                    state.insert(JobState {
                        time: meta.time(),
                        id,
                        job,
                        commitment: None,
                        snark: None,
                        order,
                    });
                }

                // One account read per prover, not per work: with a read per
                // work, a pool of more than the ledger reads' capacity (256)
                // had the reads of its newest works refused as busy, and
                // dropped those works (SNARK-POOL-OWN-WORK).
                let revalidate = state.works_by_prover();

                state.candidates_prune();
                state.zkapp_jobs = state
                    .jobs_iter()
                    .filter(|job| job.has_zkapp_command())
                    .count();
                let zkapp_jobs = state.zkapp_jobs;

                // Dispatch
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                match &global_state.config.snarker {
                    Some(config) if zkapp_jobs > 0 && config.skip_zkapp_jobs => {
                        mina_core::info!(meta.time(); kind = "SnarkWorkerZkappJobsSkipped",
                            summary = "zkApp command jobs are left to other workers: this worker's prover has no zkApp circuits",
                            jobs = zkapp_jobs);
                    }
                    Some(_) if zkapp_jobs > 0 => {
                        mina_core::debug!(meta.time(); kind = "SnarkWorkerZkappJobs",
                            summary = "zkApp command jobs at the best tip",
                            jobs = zkapp_jobs);
                    }
                    _ => {}
                }
                for snark in revalidate {
                    dispatcher.push(SnarkPoolAction::WorkCheck {
                        work: snark.work,
                        sender: snark.sender,
                        is_sender_local: false,
                        stage: AdmissionStage::Revalidate,
                        retry: false,
                    });
                }
                for snark in orphaned_snarks {
                    dispatcher.push(SnarkPoolAction::WorkCheck {
                        work: snark.work.clone(),
                        sender: snark.sender,
                        is_sender_local: false,
                        stage: AdmissionStage::Verified,
                        retry: false,
                    });
                }
                if let Some(job_id) = global_state.external_snark_worker.working_job_id() {
                    if !global_state.snark_pool.contains(job_id) {
                        // job is no longer needed.
                        dispatcher.push(ExternalSnarkWorkerAction::CancelWork);
                    }
                } else {
                    dispatcher.push(SnarkPoolAction::AutoCreateCommitment);
                }
            }
            SnarkPoolAction::WorkCoverageRefresh => {
                state.coverage_last_attempt = Some(meta.time());
                let (dispatcher, global) = state_context.into_dispatcher_and_state();
                use super::work_coverage::*;
                let Some(parent) = global.transition_frontier.best_tip() else {
                    return;
                };
                let capacity = 1usize
                    .checked_shl(
                        mina_core::constants::constraint_constants().transaction_capacity_log_2
                            as u32,
                    )
                    .unwrap_or(usize::MAX);
                if capacity > MAX_COVERAGE_TRANSACTIONS
                    || global.snark_pool.completed_snarks_iter().count() > MAX_COVERAGE_WORKS
                {
                    return;
                }
                let (coinbase_receiver, receiver_source) = global
                    .block_producer
                    .config()
                    .map(|config| (config.coinbase_receiver().clone(), "configured_producer"))
                    .unwrap_or_else(|| {
                        (
                            parent
                                .header()
                                .protocol_state
                                .body
                                .consensus_state
                                .coinbase_receiver
                                .clone(),
                            "parent_receiver_hypothesis",
                        )
                    });
                let request = WorkCoverageRequest {
                    time: meta.time(),
                    parent: parent.clone(),
                    slot: global
                        .cur_global_slot_since_genesis()
                        .unwrap_or(0)
                        .max(parent.global_slot_since_genesis().saturating_add(1)),
                    coinbase_receiver,
                    receiver_source: receiver_source.into(),
                    completed_snarks: global
                        .snark_pool
                        .completed_snarks_iter()
                        .map(|work| (work.job_id(), work.clone()))
                        .collect(),
                    transactions: global
                        .transaction_pool
                        .list_includable_transactions(capacity)
                        .into_iter()
                        .map(|cmd| cmd.data)
                        .collect(),
                };
                dispatcher.push(crate::ledger::read::LedgerReadAction::Init {
                    request: crate::ledger::read::LedgerReadRequest::WorkCoverage(Box::new(
                        request,
                    )),
                    callback: crate::ledger::read::LedgerReadInitCallback::None,
                });
            }
            SnarkPoolAction::WorkCoveragePending => {
                state.work_coverage.pending = true;
            }
            SnarkPoolAction::WorkCoverageResult { result } => {
                state.work_coverage.pending = false;
                match result {
                    Ok(coverage) => {
                        state.work_coverage.snapshot = Some(coverage.clone());
                        state.work_coverage.error = None;
                    }
                    Err(error) => {
                        state.work_coverage.error = Some(error.clone());
                    }
                }
            }
            SnarkPoolAction::CompletedSnarksFetch => {
                let (dispatcher, global) = state_context.into_dispatcher_and_state();
                let Some(p2p) = global.p2p.ready() else {
                    return;
                };
                let Some((peer_id, peer)) = p2p
                    .ready_peers_iter()
                    .filter(|(id, peer)| {
                        peer.channels.rpc.can_send_request()
                            && p2p.is_completed_snarks_request_within_budget(id, meta.time())
                    })
                    .min_by_key(|(id, _)| p2p.completed_snarks_requests.last_request(id))
                else {
                    return;
                };
                dispatcher.push(
                    crate::p2p::channels::rpc::P2pChannelsRpcAction::RequestSend {
                        peer_id: *peer_id,
                        id: peer.channels.next_local_rpc_id(),
                        request: Box::new(
                            crate::p2p::channels::rpc::P2pRpcRequest::CompletedSnarks,
                        ),
                        on_init: None,
                    },
                );
            }
            SnarkPoolAction::CompletedSnarksReceived { peer_id, works } => {
                state.completed_snarks_received = state
                    .completed_snarks_received
                    .saturating_add(works.0.len() as u64);
                mina_core::debug!(meta.time(); kind = "SnarkPoolCompletedSnarksReceived",
                    peer_id = display(peer_id), works = works.0.len(),
                    total = state.completed_snarks_received);
                let dispatcher = state_context.into_dispatcher();
                for work in &works.0 {
                    dispatcher.push(SnarkPoolAction::WorkCheck {
                        work: work.clone().into(),
                        sender: *peer_id,
                        is_sender_local: false,
                        stage: AdmissionStage::Received(WorkSource::CompletedSnarks),
                        retry: false,
                    });
                }
            }
            SnarkPoolAction::AutoCreateCommitment => {
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let Some(snarker_config) = &global_state.config.snarker else {
                    return;
                };
                let available_workers = global_state.external_snark_worker.available();
                // Block proofs first (P5.3): no job while a block of ours is
                // near and the worker shares its prover.
                let paused = available_workers > 0 && !global_state.snark_worker_may_take_job();
                if paused != global_state.snark_pool.worker_paused_for_block {
                    dispatcher.push(SnarkPoolAction::WorkerPausedForBlock { paused });
                }
                if paused {
                    return;
                }

                if available_workers > 0 {
                    let fee = snarker_config.fee.0.as_u64();
                    let skip_zkapp = snarker_config.skip_zkapp_jobs;
                    match snarker_config.strategy {
                        SnarkerStrategy::Sequential => {
                            let job_ids = global_state
                                .snark_pool
                                .jobs_to_prove_with_highest_priority(
                                    fee,
                                    skip_zkapp,
                                    available_workers,
                                )
                                .into_iter()
                                .map(|job| job.id.clone())
                                .take(available_workers) // just in case
                                .collect();
                            dispatcher.push(SnarkPoolAction::CommitmentCreateMany { job_ids });
                        }
                        // OCaml's default: uniformly among the same jobs.
                        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector/random.ml#L4-L11>
                        SnarkerStrategy::Random => {
                            let jobs = global_state.snark_pool.jobs_to_prove_iter(fee, skip_zkapp);
                            let choices = jobs.map(|job| job.id.clone()).collect();

                            dispatcher.push(SnarkPoolEffectfulAction::SnarkPoolJobsRandomChoose {
                                choices,
                                count: available_workers,
                                on_result: redux::callback!(
                                    on_snark_pool_jobs_random_choose_result(job_ids: Vec<SnarkJobId>) -> crate::Action {
                                        SnarkPoolAction::CommitmentCreateMany { job_ids }
                                    }
                                ),
                            });
                        }
                    }
                };
            }
            SnarkPoolAction::CommitmentCreateMany { job_ids } => {
                let dispatcher = state_context.into_dispatcher();
                for job_id in job_ids.iter().cloned() {
                    dispatcher.push(SnarkPoolAction::CommitmentCreate { job_id });
                }
            }
            SnarkPoolAction::CommitmentCreate { job_id } => {
                let job_id = job_id.clone();
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let Some(summary) = global_state.snark_pool.job_summary(&job_id) else {
                    return;
                };

                if global_state.external_snark_worker.is_idle() {
                    let timestamp_ms = meta.time_as_nanos() / 1_000_000;
                    let Some(config) = global_state.config.snarker.as_ref() else {
                        return;
                    };
                    let Some(job) = global_state.snark_pool.get(&job_id) else {
                        return;
                    };
                    let zkapp = job.has_zkapp_command();
                    // Where the job is in the scan state, and the jobs the
                    // worker could take: what its job selection did.
                    let orders = || {
                        global_state
                            .snark_pool
                            .jobs_to_prove_iter(config.fee.0.as_u64(), config.skip_zkapp_jobs)
                            .map(|job| job.order)
                    };
                    mina_core::info!(meta.time();
                        kind = "SnarkWorkerJobTaken",
                        summary = "the SNARK worker took a job",
                        job_id = display(&job_id),
                        order = job.order,
                        lowest_order = orders().min().unwrap_or(job.order),
                        eligible = orders().count(),
                        strategy = debug(config.strategy),
                        zkapp = zkapp);
                    // Committed before it is submitted, as OCaml marks a job
                    // scheduled before it builds its spec: a job that fails at
                    // once (its spec can't be made) is then ours already, not
                    // chosen again by the worker's next selection, which runs
                    // before an action queued after this one.
                    // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector/work_lib.ml#L177-L189>
                    dispatcher.push(SnarkPoolAction::CommitmentAdd {
                        commitment: SnarkJobCommitment::new(
                            timestamp_ms,
                            job_id.clone(),
                            config.fee.clone(),
                            config.public_key.clone().into(),
                        ),
                        sender: global_state.p2p.my_id(),
                    });
                    dispatcher.push(ExternalSnarkWorkerAction::SubmitWork {
                        job_id,
                        summary,
                        zkapp,
                    });
                }
            }
            SnarkPoolAction::CommitmentAdd { commitment, sender } => {
                state.set_commitment(JobCommitment {
                    commitment: commitment.clone(),
                    received_t: meta.time(),
                    sender: *sender,
                });

                // Dispatch
                let commitment = commitment.clone();
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                if let Some(job_id) = global_state.external_snark_worker.working_job_id() {
                    let Some(config) = global_state.config.snarker.as_ref() else {
                        return;
                    };
                    if &commitment.job_id == job_id
                        && &commitment.snarker != config.public_key.as_ref()
                    {
                        dispatcher.push(ExternalSnarkWorkerAction::CancelWork);
                    }
                }
            }
            SnarkPoolAction::WorkAdd {
                snark,
                sender,
                is_sender_local,
            } => {
                state.admission_stats.admitted += 1;
                mina_core::debug!(meta.time(); kind = "SnarkPoolAdmissionAdmitted", job_id = display(snark.job_id()));
                let replaced = state.set_snark_work(SnarkWork {
                    work: snark.clone(),
                    received_t: meta.time(),
                    sender: *sender,
                });
                state.candidates.remove_inferior_snarks(snark);
                // What became of each proof of our worker's (SNARK-POOL-OWN-WORK).
                if *is_sender_local {
                    mina_core::info!(meta.time();
                        kind = "SnarkWorkerProofAdmitted",
                        summary = "the pool took our worker's proof",
                        job_id = display(snark.job_id()));
                } else if let Some(replaced) = replaced
                    .filter(|replaced| own_key == Some(&replaced.work.snarker))
                    .filter(|_| own_key != Some(&snark.snarker))
                {
                    mina_core::info!(meta.time();
                        kind = "SnarkWorkerProofReplaced",
                        summary = "a cheaper proof replaced our worker's",
                        job_id = display(snark.job_id()),
                        our_fee = replaced.work.fee.0.as_u64(),
                        fee = snark.fee.0.as_u64(),
                        prover = display(&snark.snarker));
                }

                // Dispatch
                let snark = snark.clone();
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                if let Some(job_id) = global_state
                    .external_snark_worker
                    .working_job_id()
                    .filter(|job_id| *job_id == &snark.job_id())
                {
                    if let Some(commitment) = global_state
                        .snark_pool
                        .get(job_id)
                        .and_then(|job| job.commitment.as_ref())
                    {
                        if snark > commitment.commitment {
                            dispatcher.push(ExternalSnarkWorkerAction::CancelWork);
                        }
                    }
                }

                dispatcher.push(SnarkPoolAction::WorkBroadcast {
                    snark: snark.clone(),
                    is_local: *is_sender_local,
                });
            }
            SnarkPoolAction::P2pSendAll => {
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                for peer_id in global_state.p2p.ready_peers() {
                    dispatcher.push(SnarkPoolAction::P2pSend { peer_id });
                }
            }
            SnarkPoolAction::P2pSend { peer_id } => {
                let peer_id = *peer_id;
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let Some(peer) = global_state.p2p.get_ready_peer(&peer_id) else {
                    return;
                };

                // Send commitments.
                let index_and_limit = peer
                    .channels
                    .snark_job_commitment
                    .next_send_index_and_limit();
                let (commitments, first_index, last_index) = global_state
                    .snark_pool
                    .next_commitments_to_send(index_and_limit);

                let send_commitments = P2pChannelsSnarkJobCommitmentAction::ResponseSend {
                    peer_id,
                    commitments,
                    first_index,
                    last_index,
                };

                // Send snarks.
                let index_and_limit = peer.channels.snark.next_send_index_and_limit();
                let (snarks, first_index, last_index) =
                    global_state.snark_pool.next_snarks_to_send(index_and_limit);

                dispatcher.push(send_commitments);
                dispatcher.push(P2pChannelsSnarkAction::ResponseSend {
                    peer_id,
                    snarks,
                    first_index,
                    last_index,
                });
            }
            SnarkPoolAction::CheckTimeouts => {
                state.last_check_timeouts = meta.time();
                // PC1: a stored work whose cached proof file went missing or
                // changed is never used again; drop it, so the job is open.
                let unreadable = state
                    .unreadable_works_iter()
                    .map(|job| job.id.clone())
                    .collect::<Vec<_>>();
                for job_id in unreadable {
                    mina_core::warn!(meta.time();
                        kind = "SnarkPoolCachedProofUnreadable",
                        summary = "dropped a stored work: its cached proof failed a read",
                        job_id = display(&job_id));
                    state.remove_snark_work(&job_id);
                }

                // Dispatch
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let timed_out_ids = global_state
                    .snark_pool
                    .timed_out_commitments_iter(meta.time(), own_key)
                    .cloned()
                    .collect::<Vec<_>>();
                for job_id in timed_out_ids {
                    dispatcher.push(SnarkPoolAction::JobCommitmentTimeout { job_id });
                }

                // Read again the accounts that a new best tip's revalidation
                // could not read, unless a read for the prover is under way
                // (SNARK-POOL-OWN-WORK).
                let tip = global_state.transition_frontier.best_tip();
                if tip.is_some_and(|tip| global_state.snark_pool.jobs_tip() == Some(tip.hash())) {
                    let reading = global_state
                        .ledger
                        .read
                        .pending_requests()
                        .filter_map(|(_, request, _)| match request {
                            LedgerReadRequest::SnarkPoolProver(request) => {
                                Some(&request.work.snarker)
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    for snark in global_state.snark_pool.unchecked_works_by_prover() {
                        if !reading.contains(&&snark.work.snarker) {
                            dispatcher.push(SnarkPoolAction::WorkCheck {
                                work: snark.work,
                                sender: snark.sender,
                                is_sender_local: false,
                                stage: AdmissionStage::Revalidate,
                                retry: true,
                            });
                        }
                    }
                }
            }
            SnarkPoolAction::WorkerPausedForBlock { paused } => {
                state.worker_paused_for_block = *paused;
                mina_core::info!(
                    meta.time();
                    kind = "SnarkWorkerBlockFirst",
                    summary = if *paused {
                        "no new SNARK job: a block of ours is near"
                    } else {
                        "SNARK jobs resume"
                    },
                );
            }
            SnarkPoolAction::JobCommitmentTimeout { job_id } => {
                state.remove_commitment(job_id);

                // Dispatch
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(SnarkPoolAction::AutoCreateCommitment);
            }
        }
    }
}
