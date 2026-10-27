use std::sync::Arc;

use ledger::scan_state::scan_state::{transaction_snark::OneOrTwo, AvailableJobMessage};
use mina_core::{
    snark::{Snark, SnarkJobCommitment, SnarkJobId},
    ActionEvent,
};
use serde::{Deserialize, Serialize};

use crate::p2p::PeerId;

use super::{candidate::SnarkPoolCandidateAction, SnarkWork};

pub type SnarkPoolActionWithMeta = redux::ActionWithMeta<SnarkPoolAction>;
pub type SnarkPoolActionWithMetaRef<'a> = redux::ActionWithMeta<&'a SnarkPoolAction>;

#[derive(Serialize, Deserialize, Debug, Clone, ActionEvent)]
pub enum SnarkPoolAction {
    Candidate(SnarkPoolCandidateAction),
    WorkCheck {
        work: Snark,
        sender: PeerId,
        is_sender_local: bool,
        stage: super::AdmissionStage,
        retry: bool,
    },
    ProverAccountResult {
        request: Box<super::ProverAccountRequest>,
        result: Result<super::ProverAccount, String>,
    },
    WorkBroadcast {
        snark: Snark,
        is_local: bool,
    },

    JobsUpdate {
        jobs: Arc<Vec<OneOrTwo<AvailableJobMessage>>>,
        orphaned_snarks: Vec<SnarkWork>,
    },
    /// Pull OCaml's oldest completed jobs after sync and while work is missing.
    #[action_event(level = trace)]
    CompletedSnarksFetch,
    #[action_event(level = trace)]
    WorkCoverageRefresh,
    WorkCoveragePending,
    WorkCoverageResult {
        result: Result<super::WorkCoverage, String>,
    },
    CompletedSnarksReceived {
        peer_id: PeerId,
        works: mina_p2p_messages::rpc::CompletedSnarks,
    },
    AutoCreateCommitment,
    /// The worker found no job it may take, or may again (P5.3): it shares
    /// its prover with the block producer, whose block is near or done.
    WorkerPausedForBlock {
        paused: bool,
    },
    CommitmentCreateMany {
        job_ids: Vec<SnarkJobId>,
    },
    CommitmentCreate {
        job_id: SnarkJobId,
    },
    CommitmentAdd {
        commitment: SnarkJobCommitment,
        sender: PeerId,
    },
    #[action_event(level = debug, fields(is_sender_local))]
    WorkAdd {
        snark: Snark,
        sender: PeerId,
        is_sender_local: bool,
    },
    #[action_event(level = trace)]
    P2pSendAll,
    #[action_event(level = trace)]
    P2pSend {
        peer_id: PeerId,
    },
    CheckTimeouts,
    JobCommitmentTimeout {
        job_id: SnarkJobId,
    },
}

impl redux::EnablingCondition<crate::State> for SnarkPoolAction {
    fn is_enabled(&self, state: &crate::State, time: redux::Timestamp) -> bool {
        match self {
            SnarkPoolAction::Candidate(action) => action.is_enabled(state, time),
            SnarkPoolAction::WorkCheck { .. } | SnarkPoolAction::ProverAccountResult { .. } => true,
            SnarkPoolAction::WorkBroadcast { snark, .. } => {
                state.transition_frontier.best_tip().is_some_and(|tip| {
                    state.snark_pool.work_is_checked(snark, tip.hash())
                        && state
                            .snark_pool
                            .get(&snark.job_id())
                            .and_then(|job| job.snark.as_ref())
                            .is_some_and(|stored| {
                                stored.work.fee == snark.fee
                                    && stored.work.snarker == snark.snarker
                                    && stored.work.proofs == snark.proofs
                            })
                })
            }
            SnarkPoolAction::WorkCoverageRefresh => {
                state.transition_frontier.sync.is_synced()
                    && state.transition_frontier.best_tip().is_some()
                    && state.ledger.read.is_total_cost_under_limit()
                    && !state.snark_pool.work_coverage.pending
                    && state.snark_pool.coverage_last_attempt.is_none_or(|last| {
                        time.checked_sub(last).unwrap_or_default()
                            >= super::work_coverage::COVERAGE_INTERVAL
                    })
            }
            SnarkPoolAction::WorkCoveragePending => !state.snark_pool.work_coverage.pending,
            SnarkPoolAction::WorkCoverageResult { .. } => state.snark_pool.work_coverage.pending,
            SnarkPoolAction::CompletedSnarksFetch => {
                state.transition_frontier.sync.is_synced()
                    && state.snark.work_verify.jobs.is_empty()
                    && state.snark_pool.jobs_iter().any(|job| job.snark.is_none())
                    && state.p2p.ready().is_some_and(|p2p| {
                        p2p.ready_peers_iter().any(|(id, peer)| {
                            peer.channels.rpc.can_send_request()
                                && p2p.is_completed_snarks_request_within_budget(id, time)
                        })
                    })
            }
            SnarkPoolAction::CompletedSnarksReceived { works, .. } => {
                works.0.len() <= mina_p2p_messages::rpc::MAX_COMPLETED_SNARKS
            }
            SnarkPoolAction::AutoCreateCommitment => {
                state.config.snarker.as_ref().is_some_and(|v| v.auto_commit)
            }
            SnarkPoolAction::WorkerPausedForBlock { paused } => {
                *paused != state.snark_pool.worker_paused_for_block
            }
            SnarkPoolAction::CommitmentCreateMany { .. } => state.config.snarker.is_some(),
            SnarkPoolAction::CommitmentCreate { job_id } => {
                state.config.snarker.as_ref().is_some_and(|config| {
                    state
                        .snark_pool
                        .should_create_commitment(job_id, config.fee.0.as_u64())
                })
            }
            SnarkPoolAction::CommitmentAdd { commitment, .. } => state
                .snark_pool
                .get(&commitment.job_id)
                .is_some_and(|s| match s.commitment.as_ref() {
                    Some(cur) => commitment > &cur.commitment,
                    None => true,
                }),
            // Strictly cheaper than the stored work, local work included.
            // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool_diff.ml#L185-L208>
            // Our worker's work only for the key and fee it has now (P5.4).
            SnarkPoolAction::WorkAdd {
                snark,
                is_sender_local,
                ..
            } => {
                (!is_sender_local || state.snark_worker_authorizes(snark))
                    && state.snark_pool.get(&snark.job_id()).is_some_and(|s| {
                        state
                            .transition_frontier
                            .best_tip()
                            .is_some_and(|tip| state.snark_pool.work_is_checked(snark, tip.hash()))
                            && s.snark
                                .as_ref()
                                .is_none_or(|cur| snark.fee.0.as_u64() < cur.work.fee.0.as_u64())
                    })
            }
            SnarkPoolAction::P2pSend { peer_id } => state
                .p2p
                .get_ready_peer(peer_id)
                // can't propagate empty snarkpool
                .filter(|_| !state.snark_pool.is_empty())
                .filter(|_| {
                    state
                        .transition_frontier
                        .best_tip()
                        .is_some_and(|tip| state.snark_pool.jobs_tip.as_ref() == Some(tip.hash()))
                })
                // Only send commitments/snarks if peer has the same best tip,
                // or its best tip is extension of our best tip. In such case
                // no commitment/snark will be dropped by peer, because it
                // doesn't yet have those jobs.
                //
                // By sending commitments/snarks to the peer, which has next
                // best tip, we might send outdated commitments/snarks, but
                // we might send useful ones as well.
                .and_then(|p| {
                    let peer_best_tip = p.best_tip.as_ref()?;
                    let our_best_tip = state.transition_frontier.best_tip()?.hash();
                    Some(p).filter(|_| {
                        peer_best_tip.hash() == our_best_tip
                            || peer_best_tip.pred_hash() == our_best_tip
                    })
                })
                .is_some_and(|p| {
                    let check =
                        |(next_index, limit), last_index| limit > 0 && next_index <= last_index;
                    let last_index = state.snark_pool.last_index();

                    check(
                        p.channels.snark_job_commitment.next_send_index_and_limit(),
                        last_index,
                    ) || check(p.channels.snark.next_send_index_and_limit(), last_index)
                }),
            SnarkPoolAction::CheckTimeouts => time
                .checked_sub(state.snark_pool.last_check_timeouts)
                .is_some_and(|dur| dur.as_secs() >= 5),
            SnarkPoolAction::JobCommitmentTimeout { job_id } => {
                state.snark_pool.is_commitment_timed_out(
                    job_id,
                    time,
                    state
                        .config
                        .snarker
                        .as_ref()
                        .map(|config| config.public_key.as_ref()),
                )
            }
            SnarkPoolAction::JobsUpdate { .. } => true,
            SnarkPoolAction::P2pSendAll => true,
        }
    }
}

// Effectful actions

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SnarkPoolEffectfulAction {
    SnarkPoolJobsRandomChoose {
        choices: Vec<SnarkJobId>,
        count: usize,
        on_result: redux::Callback<Vec<SnarkJobId>>,
    },
}

pub type SnarkPoolEffectfulActionWithMeta = redux::ActionWithMeta<SnarkPoolEffectfulAction>;

impl redux::EnablingCondition<crate::State> for SnarkPoolEffectfulAction {
    fn is_enabled(&self, _state: &crate::State, _time: redux::Timestamp) -> bool {
        true
    }
}
