use std::sync::Arc;

use ledger::scan_state::transaction_logic::valid;
use mina_core::{block::ArcBlockWithHash, ActionEvent};
use mina_p2p_messages::v2::{MinaBaseProofStableV2, StateHash};
use mina_snark::block_verify::{SnarkBlockVerifyId, SnarkBlockVerifyStatus};
use serde::{Deserialize, Serialize};

use crate::block_producer_effectful::StagedLedgerDiffCreateOutput;

use super::{
    vrf_evaluator::BlockProducerVrfEvaluatorAction, BlockProducerCurrentState,
    BlockProducerWonSlot, BlockProducerWonSlotDiscardReason,
};

pub type BlockProducerActionWithMeta = redux::ActionWithMeta<BlockProducerAction>;
pub type BlockProducerActionWithMetaRef<'a> = redux::ActionWithMeta<&'a BlockProducerAction>;

#[derive(Serialize, Deserialize, Debug, Clone, ActionEvent)]
#[action_event(level = debug)]
pub enum BlockProducerAction {
    VrfEvaluator(BlockProducerVrfEvaluatorAction),
    BestTipUpdate {
        best_tip: ArcBlockWithHash,
    },
    WonSlotSearch,
    #[action_event(
        level = info,
        fields(
            slot = won_slot.global_slot.slot_number.as_u32(),
            slot_time = mina_core::log::to_rfc_3339(won_slot.slot_time)
                .unwrap_or_else(|_| "<error>".to_owned()),
            current_time = mina_core::log::to_rfc_3339(context.timestamp())
                .unwrap_or_else(|_| "<error>".to_owned()),
        )
    )]
    WonSlot {
        won_slot: BlockProducerWonSlot,
    },
    #[action_event(
        level = info,
        fields(
            reason = format!("{reason:?}"),
        )
    )]
    WonSlotDiscard {
        reason: BlockProducerWonSlotDiscardReason,
    },
    WonSlotWait,
    WonSlotTransactionsGet,
    WonSlotTransactionsSuccess {
        transactions_by_fee: Vec<valid::UserCommand>,
    },
    WonSlotProduceInit,
    StagedLedgerDiffCreateInit,
    StagedLedgerDiffCreatePending,
    StagedLedgerDiffCreateSuccess {
        output: Arc<StagedLedgerDiffCreateOutput>,
    },
    BlockUnprovenBuild,
    BlockProveInit,
    BlockProvePending,
    BlockProofVerify {
        block_hash: StateHash,
        proof: Arc<MinaBaseProofStableV2>,
    },
    BlockProofVerifyPending {
        block_hash: StateHash,
        req_id: SnarkBlockVerifyId,
    },
    BlockProveSuccess {
        block_hash: StateHash,
    },
    BlockProveError {
        block_hash: StateHash,
    },
    #[action_event(level = warn, fields(display(block_hash)))]
    BlockProofRejected {
        block_hash: mina_p2p_messages::v2::StateHash,
    },
    BlockProduced,
    #[action_event(level = trace)]
    BlockInject,
    BlockInjected,
}

impl redux::EnablingCondition<crate::State> for BlockProducerAction {
    fn is_enabled(&self, state: &crate::State, time: redux::Timestamp) -> bool {
        if !state.transition_frontier.genesis_ledgers_ready() {
            return false;
        }
        match self {
            BlockProducerAction::VrfEvaluator(a) => a.is_enabled(state, time),
            BlockProducerAction::BestTipUpdate { .. } => true,
            BlockProducerAction::WonSlotSearch => state
                .block_producer
                .with(None, |this| {
                    if !this.current.won_slot_should_search() {
                        return None;
                    }
                    if is_syncing_to_produced_block(state) {
                        return None;
                    }
                    let best_tip = state.transition_frontier.best_tip()?;
                    let cur_global_slot = state.cur_global_slot()?;
                    let next = this.next_won_slot(cur_global_slot, best_tip);
                    Some(next.is_some_and(|slot| {
                        mina_core::constants::stop_slots().blocks_allowed(slot.global_slot())
                    }))
                })
                .is_some_and(|v| v),
            BlockProducerAction::WonSlot { won_slot } => state.block_producer.with(false, |this| {
                let Some(best_tip) = state.transition_frontier.best_tip() else {
                    return false;
                };
                if is_syncing_to_produced_block(state) {
                    return false;
                }

                this.current.won_slot_should_search()
                    && mina_core::constants::stop_slots().blocks_allowed(won_slot.global_slot())
                    && state.cur_global_slot().is_none_or(|cur_global_slot| {
                        won_slot.global_slot()
                            >= this.current.won_slot_search_start(cur_global_slot)
                    })
                    && won_slot >= best_tip
            }),
            BlockProducerAction::WonSlotWait => state
                .block_producer
                .with(false, |this| this.current.won_slot_should_wait(time)),
            BlockProducerAction::WonSlotProduceInit => {
                state.block_producer.with(false, |this| {
                    let has_genesis_proven_if_needed = || {
                        state.transition_frontier.best_tip().is_some_and(|tip| {
                            let proven_block = state.transition_frontier.genesis.proven_block();
                            !tip.is_genesis()
                                || proven_block.is_some_and(|b| Arc::ptr_eq(&b.block, &tip.block))
                        })
                    };
                    this.current.won_slot_should_produce(time)
                        && this.current.won_slot().is_some_and(|slot| mina_core::constants::stop_slots().blocks_allowed(slot.global_slot()))
                        && has_genesis_proven_if_needed()
                        // don't start block production (particularly staged ledger diff creation),
                        // if transition frontier sync commit is pending,
                        // as in case when fork is being committed, there
                        // is no guarantee that staged ledger for the current
                        // best tip (chosen as a parent for the new block being produced),
                        // will be still there, once the staged ledger
                        // diff creation request reaches the ledger service.
                        // So we would be trying to build on top of
                        // non-existent staged ledger causing a failure.
                        && !state.transition_frontier.sync.is_commit_pending()
                })
            }
            BlockProducerAction::WonSlotTransactionsGet => {
                state.block_producer.with(false, |this| {
                    matches!(
                        this.current,
                        BlockProducerCurrentState::WonSlotProduceInit { .. }
                    )
                })
            }
            BlockProducerAction::WonSlotTransactionsSuccess { .. } => {
                state.block_producer.with(false, |this| {
                    matches!(
                        this.current,
                        BlockProducerCurrentState::WonSlotTransactionsGet { .. }
                    )
                })
            }
            BlockProducerAction::StagedLedgerDiffCreateInit => {
                state.block_producer.with(false, |this| {
                    matches!(
                        this.current,
                        BlockProducerCurrentState::WonSlotTransactionsSuccess { .. }
                    )
                })
            }
            BlockProducerAction::StagedLedgerDiffCreatePending => {
                state.block_producer.with(false, |this| {
                    matches!(
                        this.current,
                        BlockProducerCurrentState::WonSlotTransactionsSuccess { .. }
                    )
                })
            }
            BlockProducerAction::StagedLedgerDiffCreateSuccess { .. } => {
                state.block_producer.with(false, |this| {
                    matches!(
                        this.current,
                        BlockProducerCurrentState::StagedLedgerDiffCreatePending { .. }
                    )
                })
            }
            BlockProducerAction::BlockUnprovenBuild => state.block_producer.with(false, |this| {
                matches!(
                    this.current,
                    BlockProducerCurrentState::StagedLedgerDiffCreateSuccess { .. }
                )
            }),
            BlockProducerAction::BlockProveInit => state.block_producer.with(false, |this| {
                matches!(
                    this.current,
                    BlockProducerCurrentState::BlockUnprovenBuilt { .. }
                )
            }),
            BlockProducerAction::BlockProvePending => state.block_producer.with(false, |this| {
                matches!(
                    this.current,
                    BlockProducerCurrentState::BlockUnprovenBuilt { .. }
                )
            }),
            BlockProducerAction::BlockProofVerify { block_hash, .. }
            | BlockProducerAction::BlockProveError { block_hash } => {
                state.block_producer.with(false, |this| {
                    matches!(&this.current, BlockProducerCurrentState::BlockProvePending {
                    block_hash: pending, verification_req_id: None, ..
                } if pending == block_hash)
                })
            }
            BlockProducerAction::BlockProofVerifyPending { block_hash, req_id } => {
                state.block_producer.with(false, |this| {
                    matches!(&this.current, BlockProducerCurrentState::BlockProvePending {
                    block_hash: pending, verification_req_id: None, ..
                } if pending == block_hash)
                        && state
                            .snark
                            .block_verify
                            .jobs
                            .get(*req_id)
                            .is_some_and(|job| {
                                job.is_init() && job.block().hash_ref() == block_hash
                            })
                })
            }
            BlockProducerAction::BlockProveSuccess { block_hash }
            | BlockProducerAction::BlockProofRejected { block_hash } => {
                state.block_producer.with(false, |this| {
                    let BlockProducerCurrentState::BlockProvePending {
                        block_hash: pending,
                        verification_req_id: Some(req_id),
                        ..
                    } = &this.current
                    else {
                        return false;
                    };
                    pending == block_hash
                        && state
                            .snark
                            .block_verify
                            .jobs
                            .get(*req_id)
                            .is_some_and(|job| {
                                job.block().hash_ref() == block_hash
                                    && matches!(
                                        (self, job),
                                        (
                                            Self::BlockProveSuccess { .. },
                                            SnarkBlockVerifyStatus::Success { .. }
                                        ) | (
                                            Self::BlockProofRejected { .. },
                                            SnarkBlockVerifyStatus::Error { .. }
                                        )
                                    )
                            })
                })
            }
            BlockProducerAction::BlockProduced => state.block_producer.with(false, |this| {
                matches!(
                    this.current,
                    BlockProducerCurrentState::BlockProveSuccess { .. }
                )
            }),
            BlockProducerAction::BlockInject => {
                state
                    .block_producer
                    .with(false, |this| match &this.current {
                        BlockProducerCurrentState::Produced { block, .. } => {
                            block_inject_delay_elapsed(
                                block.timestamp(),
                                time,
                                this.config.block_inject_delay_ms,
                            ) && !state.transition_frontier.sync.is_commit_pending()
                        }
                        _ => false,
                    })
            }
            BlockProducerAction::BlockInjected => state.block_producer.with(false, |this| {
                matches!(this.current, BlockProducerCurrentState::Produced { .. })
            }),
            BlockProducerAction::WonSlotDiscard { reason } => {
                let current_reason = state.block_producer.with(None, |bp| {
                    let best_tip = state.transition_frontier.best_tip()?;
                    bp.current.won_slot_should_discard(best_tip, time)
                });
                Some(reason) == current_reason.as_ref()
            }
        }
    }
}

fn block_inject_delay_elapsed(
    block_time: redux::Timestamp,
    now: redux::Timestamp,
    delay_ms: u64,
) -> bool {
    // OCaml rejects future gossip against the receiver's clock; keep the
    // previous 1 s margin even for directly constructed configurations.
    // OCaml: src/lib/transition_router/initial_validator.ml#L108-L112
    super::effective_block_inject_delay_ms(delay_ms)
        .checked_mul(1_000_000)
        .and_then(|delay_ns| block_time.checked_add(delay_ns))
        .is_some_and(|earliest| now >= earliest)
}

#[cfg(test)]
mod tests {
    use super::block_inject_delay_elapsed;
    use redux::Timestamp;

    #[test]
    fn block_inject_delay_has_an_inclusive_boundary() {
        let start = Timestamp::new(10_000_000_000_u64);
        for delay_ms in [0, 999, 1_000] {
            assert!(!block_inject_delay_elapsed(
                start,
                Timestamp::new(10_999_999_999_u64),
                delay_ms
            ));
            assert!(block_inject_delay_elapsed(
                start,
                Timestamp::new(11_000_000_000_u64),
                delay_ms
            ));
        }
        assert!(!block_inject_delay_elapsed(
            start,
            Timestamp::new(12_999_999_999_u64),
            3_000
        ));
        assert!(block_inject_delay_elapsed(
            start,
            Timestamp::new(13_000_000_000_u64),
            3_000
        ));
        assert!(!block_inject_delay_elapsed(
            start,
            Timestamp::new(u64::MAX),
            u64::MAX
        ));
    }
}

fn is_syncing_to_produced_block(state: &crate::State) -> bool {
    state
        .transition_frontier
        .sync
        .best_tip()
        .is_some_and(|tip| state.block_producer.is_me(tip.producer()))
}
