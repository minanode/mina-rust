use crate::p2p::P2pNetworkPubsubAction;
use mina_core::{
    block::ArcBlockWithHash,
    bug_condition,
    consensus::{in_seed_update_range, ConsensusConstants},
};
use mina_p2p_messages::v2;
use mina_snark::block_verify::{SnarkBlockVerifyAction, SnarkBlockVerifyError, SnarkBlockVerifyId};
use redux::{callback, Dispatcher, Timestamp};

use crate::{
    transition_frontier::sync::TransitionFrontierSyncAction, Action, BlockProducerEffectfulAction,
    State, Substate, TransactionPoolAction,
};

use super::{
    build_unproven_block, next_epoch_first_slot, to_epoch_and_slot,
    vrf_evaluator::{
        BlockProducerVrfEvaluatorAction, BlockProducerVrfEvaluatorState, InterruptReason,
    },
    BlockProducerAction, BlockProducerActionWithMetaRef, BlockProducerCurrentState,
    BlockProducerEnabled, BlockProducerState,
};

impl BlockProducerState {
    pub fn reducer(
        state: &mut State,
        action: BlockProducerActionWithMetaRef<'_>,
        dispatcher: &mut Dispatcher<Action, State>,
    ) {
        let refresh = match action.action() {
            BlockProducerAction::BestTipUpdate { .. } => state
                .block_producer
                .with(false, |bp| bp.current.won_slot_should_search()),
            BlockProducerAction::WonSlotSearch
            | BlockProducerAction::WonSlot { .. }
            | BlockProducerAction::WonSlotDiscard { .. }
            | BlockProducerAction::WonSlotProduceInit
            | BlockProducerAction::BlockInjected
            | BlockProducerAction::BlockProveError { .. }
            | BlockProducerAction::BlockProofRejected { .. } => true,
            _ => false,
        };
        let proof_failed = matches!(
            action.action(),
            BlockProducerAction::BlockProveError { .. }
                | BlockProducerAction::BlockProofRejected { .. }
        );
        BlockProducerEnabled::reducer(Substate::new(state, dispatcher), action);
        // Retrying the same proof keeps its scheduled timing. Only a discard
        // makes a new scheduling decision.
        let refresh = refresh
            && (!proof_failed
                || state
                    .block_producer
                    .with(false, |bp| bp.current.won_slot_should_search()));
        if refresh {
            let cur_global_slot = state.cur_global_slot();
            if let (Some(best_tip), Some(producer)) = (
                state.transition_frontier.best_tip(),
                state.block_producer.as_mut(),
            ) {
                producer.update_next_producer_timing(best_tip, cur_global_slot);
            }
        }
    }
}

impl BlockProducerEnabled {
    /// Substate is accesses from global state, because applied blocks from transition frontier are required
    pub fn reducer(mut state_context: Substate<State>, action: BlockProducerActionWithMetaRef<'_>) {
        let (action, meta) = action.split();
        let Ok(global_state) = state_context.get_substate_mut() else {
            return;
        };
        let consensus_constants = &global_state.config.consensus_constants;

        let best_chain = &global_state.transition_frontier.best_chain;
        let Some(state) = global_state.block_producer.as_mut() else {
            return;
        };

        match action {
            BlockProducerAction::LedgerRecoveryAbort => {
                state.current = BlockProducerCurrentState::Idle { time: meta.time() };
                state.next_producer_timing = None;
                state.injected_blocks.clear();
                state.vrf_evaluator = state.vrf_evaluator.reset(meta.time());
                state.proof_retried = false;
            }
            BlockProducerAction::VrfEvaluator(action) => {
                BlockProducerVrfEvaluatorState::reducer(
                    Substate::from_compatible_substate(state_context),
                    meta.with_action(action),
                );
            }
            BlockProducerAction::BestTipUpdate { best_tip } => {
                state.injected_blocks.remove(best_tip.hash());
                // set the genesis timestamp on the first best tip update
                // TODO: move/remove once we can generate the genesis block
                if state.vrf_evaluator.genesis_timestamp == redux::Timestamp::ZERO {
                    state.vrf_evaluator.genesis_timestamp = best_tip.genesis_timestamp();
                }

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                Self::dispatch_best_tip_update(dispatcher, state, best_tip, meta.time());
            }
            BlockProducerAction::EvaluationRecheck => {
                // Recorded before the check, which may not run while an
                // evaluation that is still valid continues.
                let current_epoch = global_state.current_epoch();
                if let Some(producer) = global_state.block_producer.as_mut() {
                    producer
                        .vrf_evaluator
                        .set_checked_clock_epoch(current_epoch);
                }
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                if let Some(best_tip) = state.transition_frontier.best_tip() {
                    Self::dispatch_evaluability_check(dispatcher, state, best_tip);
                }
            }
            BlockProducerAction::WonSlotSearch => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                if let Some(won_slot) = state.block_producer.with(None, |bp| {
                    let best_tip = state.transition_frontier.best_tip()?;
                    let cur_global_slot = state.cur_global_slot()?;
                    bp.next_won_slot(cur_global_slot, best_tip)
                }) {
                    dispatcher.push(BlockProducerAction::WonSlot { won_slot });
                }
            }
            BlockProducerAction::WonSlot { won_slot } => {
                state.proof_retried = false;
                state.current = BlockProducerCurrentState::WonSlot {
                    time: meta.time(),
                    won_slot: won_slot.clone(),
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(BlockProducerEffectfulAction::WonSlot {
                    won_slot: won_slot.clone(),
                });
            }
            BlockProducerAction::WonSlotDiscard { reason } => {
                if let Some(won_slot) = state.current.won_slot() {
                    mina_core::info!(meta.time();
                        kind = "ProducerBlockDiscarded",
                        slot = won_slot.global_slot.slot_number.as_u32(),
                        state_hash = state.current.built_block_hash().map(ToString::to_string),
                        reason = format!("{reason:?}"),
                        best_tip_hash = best_chain.last().map(|tip| tip.hash().to_string()),
                        after_injection = matches!(state.current, BlockProducerCurrentState::Injected { .. }),
                    );
                    state.current = BlockProducerCurrentState::WonSlotDiscarded {
                        time: meta.time(),
                        won_slot: won_slot.clone(),
                        reason: *reason,
                    };
                }

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(BlockProducerEffectfulAction::WonSlotDiscard { reason: *reason });
            }
            BlockProducerAction::WonSlotWait => {
                if let Some(won_slot) = state.current.won_slot() {
                    state.current = BlockProducerCurrentState::WonSlotWait {
                        time: meta.time(),
                        won_slot: won_slot.clone(),
                    };
                }
            }
            BlockProducerAction::WonSlotProduceInit => {
                // Pool reconciliation never delays a won slot. Capture the parent
                // and select the immediately available transactions in this dispatch.
                // OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/block_producer/block_producer.ml#L714-L798>
                // OCaml: <https://github.com/MinaProtocol/mina/blob/73134bfd3a43c3f4d8ee564b3d04a0c6ed2e0b3c/src/lib/network_pool/transaction_pool.ml#L438>
                if let Some(won_slot) = state.current.won_slot() {
                    if let Some(chain) = best_chain.last().map(|best_tip| {
                        if best_tip.global_slot() == won_slot.global_slot() {
                            // We are producing block which replaces current best tip
                            // instead of extending it.
                            best_chain
                                .get(..best_chain.len().saturating_sub(1))
                                .unwrap_or(&[])
                                .to_vec()
                        } else {
                            best_chain.to_vec()
                        }
                    }) {
                        state.current = BlockProducerCurrentState::WonSlotProduceInit {
                            time: meta.time(),
                            won_slot: won_slot.clone(),
                            chain,
                        };
                    };
                }

                // The win's VRF used the epoch data it was evaluated with. A
                // parent selecting other data (a reorg or a delayed block since
                // the evaluation) would give the block other staking data.
                if matches!(
                    state.current,
                    BlockProducerCurrentState::WonSlotProduceInit { .. }
                ) && !state.current.parent_epoch_data_matches()
                {
                    let won_slot = state.current.won_slot().cloned();
                    let parent = state
                        .current
                        .parent_chain()
                        .and_then(|chain| chain.last())
                        .map(|block| block.hash().clone());
                    if let Some(won_slot) = won_slot {
                        mina_core::warn!(meta.time();
                            kind = "ProducerWinEpochDataChanged",
                            slot = won_slot.global_slot(),
                            parent = parent.as_ref().map(ToString::to_string),
                            message = "Won slot dropped: its parent takes other epoch data");
                        // Not chosen again for this parent; another parent,
                        // or the slot evaluated again, may still produce.
                        state.epoch_data_refused = parent.map(|parent| super::RefusedWin {
                            global_slot: won_slot.global_slot(),
                            epoch_data: won_slot.epoch_data.clone(),
                            parent,
                        });
                        state.current = BlockProducerCurrentState::WonSlotDiscarded {
                            time: meta.time(),
                            won_slot,
                            reason: super::BlockProducerWonSlotDiscardReason::EpochDataChanged,
                        };
                    }
                    let (dispatcher, state) = state_context.into_dispatcher_and_state();
                    dispatcher.push(BlockProducerEffectfulAction::WonSlotDiscard {
                        reason: super::BlockProducerWonSlotDiscardReason::EpochDataChanged,
                    });
                    if let Some(best_tip) = state.transition_frontier.best_tip() {
                        Self::dispatch_evaluability_check(dispatcher, state, best_tip);
                    }
                    dispatcher.push(BlockProducerAction::WonSlotSearch);
                    return;
                }

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(BlockProducerAction::WonSlotTransactionsGet);
            }
            BlockProducerAction::WonSlotTransactionsGet => {
                let BlockProducerCurrentState::WonSlotProduceInit {
                    won_slot, chain, ..
                } = &mut state.current
                else {
                    bug_condition!("Invalid state for `BlockProducerAction::WonSlotTransactionsGet` expected: `BlockProducerCurrentState::WonSlotProduceInit`, found: {:?}", state.current);
                    return;
                };

                state.current = BlockProducerCurrentState::WonSlotTransactionsGet {
                    time: meta.time(),
                    won_slot: won_slot.clone(),
                    chain: chain.clone(),
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(TransactionPoolAction::CollectTransactionsByFee);
            }
            BlockProducerAction::WonSlotTransactionsSuccess {
                transactions_by_fee,
            } => {
                let BlockProducerCurrentState::WonSlotTransactionsGet {
                    won_slot, chain, ..
                } = &mut state.current
                else {
                    bug_condition!("Invalid state for `BlockProducerAction::WonSlotTransactionsSuccess` expected: `BlockProducerCurrentState::WonSlotTransactionsGet`, found: {:?}", state.current);
                    return;
                };

                state.current = BlockProducerCurrentState::WonSlotTransactionsSuccess {
                    time: meta.time(),
                    won_slot: won_slot.clone(),
                    chain: chain.clone(),
                    transactions_by_fee: transactions_by_fee.clone(),
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(BlockProducerAction::StagedLedgerDiffCreateInit);
            }
            BlockProducerAction::StagedLedgerDiffCreateInit => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(BlockProducerEffectfulAction::StagedLedgerDiffCreateInit);
            }
            BlockProducerAction::StagedLedgerDiffCreatePending => {
                let BlockProducerCurrentState::WonSlotTransactionsSuccess {
                    won_slot,
                    chain,
                    transactions_by_fee,
                    ..
                } = &mut state.current
                else {
                    bug_condition!("Invalid state for `BlockProducerAction::StagedLedgerDiffCreatePending` expected: `BlockProducerCurrentState::WonSlotTransactionsSuccess`, found: {:?}", state.current);
                    return;
                };
                state.current = BlockProducerCurrentState::StagedLedgerDiffCreatePending {
                    time: meta.time(),
                    won_slot: won_slot.clone(),
                    chain: std::mem::take(chain),
                    transactions_by_fee: transactions_by_fee.to_vec(),
                };
            }
            BlockProducerAction::StagedLedgerDiffCreateSuccess { output } => {
                let BlockProducerCurrentState::StagedLedgerDiffCreatePending {
                    won_slot,
                    chain,
                    ..
                } = &mut state.current
                else {
                    bug_condition!("Invalid state for `BlockProducerAction::StagedLedgerDiffCreateSuccess` expected: `BlockProducerCurrentState::StagedLedgerDiffCreatePending`, found: {:?}", state.current);
                    return;
                };
                state.current = BlockProducerCurrentState::StagedLedgerDiffCreateSuccess {
                    time: meta.time(),
                    won_slot: won_slot.clone(),
                    chain: std::mem::take(chain),
                    diff: output.diff.clone(),
                    diff_hash: output.diff_hash.clone(),
                    staged_ledger_hash: output.staged_ledger_hash.clone(),
                    emitted_ledger_proof: output.emitted_ledger_proof.clone(),
                    pending_coinbase_update: output.pending_coinbase_update.clone(),
                    pending_coinbase_witness: output.pending_coinbase_witness.clone(),
                    stake_proof_sparse_ledger: output.stake_proof_sparse_ledger.clone(),
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(BlockProducerEffectfulAction::StagedLedgerDiffCreateSuccess);
            }
            BlockProducerAction::BlockUnprovenBuild => {
                state.reduce_block_unproved_build(consensus_constants, meta.time());

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(BlockProducerEffectfulAction::BlockUnprovenBuild);
            }
            BlockProducerAction::BlockProveInit => {
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(BlockProducerEffectfulAction::BlockProveInit);
            }
            BlockProducerAction::BlockProvePending => {
                let current_state = std::mem::take(&mut state.current);

                if let BlockProducerCurrentState::BlockUnprovenBuilt {
                    won_slot,
                    chain,
                    emitted_ledger_proof,
                    pending_coinbase_update,
                    pending_coinbase_witness,
                    stake_proof_sparse_ledger,
                    block,
                    block_hash,
                    ..
                } = current_state
                {
                    state.current = BlockProducerCurrentState::BlockProvePending {
                        time: meta.time(),
                        won_slot,
                        chain,
                        emitted_ledger_proof,
                        pending_coinbase_update,
                        pending_coinbase_witness,
                        stake_proof_sparse_ledger,
                        block,
                        block_hash,
                        verification_req_id: None,
                    };
                } else {
                    bug_condition!("Invalid state for `BlockProducerAction::BlockProvePending` expected: `BlockProducerCurrentState::BlockUnprovenBuilt`, found: {:?}", current_state);
                }
            }
            BlockProducerAction::BlockProofVerify { block_hash, proof } => {
                let BlockProducerCurrentState::BlockProvePending { block, .. } = &state.current
                else {
                    return;
                };
                let block = block
                    .clone()
                    .with_hash_and_proof(block_hash.clone(), proof.clone());
                // Use the same asynchronous verifier worker as incoming blocks. Keep
                // the original unproven inputs until verification succeeds or we retry.
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(SnarkBlockVerifyAction::Init {
                    block: block.into(),
                    on_init: callback!(
                        on_produced_block_verify_init((block_hash: v2::StateHash, req_id: SnarkBlockVerifyId)) -> crate::Action {
                            BlockProducerAction::BlockProofVerifyPending { block_hash, req_id }
                        }),
                    on_success: callback!(
                        on_produced_block_verify_success(block_hash: v2::StateHash) -> crate::Action {
                            BlockProducerAction::BlockProveSuccess { block_hash }
                        }),
                    on_error: callback!(
                        on_produced_block_verify_error((block_hash: v2::StateHash, _error: SnarkBlockVerifyError)) -> crate::Action {
                            BlockProducerAction::BlockProofRejected { block_hash }
                        }),
                });
            }
            BlockProducerAction::BlockProofVerifyPending { req_id, .. } => {
                if let BlockProducerCurrentState::BlockProvePending {
                    verification_req_id,
                    ..
                } = &mut state.current
                {
                    *verification_req_id = Some(*req_id);
                }
            }
            BlockProducerAction::BlockProveSuccess { .. } => {
                let BlockProducerCurrentState::BlockProvePending {
                    verification_req_id: Some(req_id),
                    ..
                } = &state.current
                else {
                    return;
                };
                let Some(job) = global_state.snark.block_verify.jobs.get(*req_id) else {
                    return;
                };
                let proof = job.block().header_ref().protocol_state_proof.clone();
                let current_state = std::mem::take(&mut state.current);

                if let BlockProducerCurrentState::BlockProvePending {
                    won_slot,
                    chain,
                    block,
                    block_hash,
                    ..
                } = current_state
                {
                    state.current = BlockProducerCurrentState::BlockProveSuccess {
                        time: meta.time(),
                        won_slot,
                        chain,
                        block,
                        block_hash,
                        proof: proof.clone(),
                    };
                } else {
                    bug_condition!("Invalid state for `BlockProducerAction::BlockProveSuccess` expected: `BlockProducerCurrentState::BlockProvePending`, found: {:?}", current_state);
                }

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(BlockProducerEffectfulAction::BlockProveSuccess);
            }
            BlockProducerAction::BlockProofRejected { block_hash }
            | BlockProducerAction::BlockProveError { block_hash } => {
                let reason = if matches!(action, BlockProducerAction::BlockProveError { .. }) {
                    super::BlockProducerWonSlotDiscardReason::ProofGenerationFailed
                } else {
                    super::BlockProducerWonSlotDiscardReason::ProofVerificationFailed
                };
                let won_slot = state.current.won_slot().cloned();
                let retry = state.retry_failed_proof(block_hash, meta.time());
                if !retry {
                    if let Some(won_slot) = won_slot {
                        state.current = BlockProducerCurrentState::WonSlotDiscarded {
                            time: meta.time(),
                            won_slot,
                            reason,
                        };
                    }
                }
                mina_core::error!(meta.time();
                    message = if retry { "Block proof failed; retrying once" } else { "Block proof failed; discarding slot" },
                    block_hash = block_hash.to_string(), reason = format!("{reason:?}"));
                let dispatcher = state_context.into_dispatcher();
                if retry {
                    dispatcher.push(BlockProducerAction::BlockProveInit);
                } else {
                    dispatcher.push(BlockProducerEffectfulAction::WonSlotDiscard { reason });
                }
            }
            BlockProducerAction::BlockProduced => {
                let current_state = std::mem::take(&mut state.current);

                if let BlockProducerCurrentState::BlockProveSuccess {
                    won_slot,
                    chain,
                    block,
                    block_hash,
                    proof,
                    ..
                } = current_state
                {
                    let block = block.with_hash_and_proof(block_hash, proof);
                    // OCaml notifies uptime before inserting the breadcrumb in
                    // the frontier. Keep the event even if a later fork wins.
                    // OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/block_producer/block_producer.ml#L968-L980>
                    if let Some(waiter) = &mut state.uptime_produced_block {
                        waiter.complete(&block, won_slot.slot_time);
                    }
                    state.current = BlockProducerCurrentState::Produced {
                        time: meta.time(),
                        won_slot,
                        chain,
                        block,
                    };
                } else {
                    bug_condition!("Invalid state for `BlockProducerAction::BlockProduced` expected: `BlockProducerCurrentState::BlockProveSuccess`, found: {:?}", current_state);
                }

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(BlockProducerAction::BlockInject);
            }
            BlockProducerAction::BlockInject => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                let Some((best_tip, root_block, blocks_inbetween)) = None.or_else(|| {
                    let (best_tip, chain) = state.block_producer.produced_block_with_chain()?;
                    let mut iter = chain.iter();
                    let root_block = iter.next()?.block_with_hash();
                    let blocks_inbetween = iter.map(|b| b.hash().clone()).collect();
                    Some((best_tip.clone(), root_block.clone(), blocks_inbetween))
                }) else {
                    bug_condition!("Invalid state for `BlockProducerAction::BlockInject`: did not find best_tip/root_block in block producer");
                    return;
                };

                let previous_root_snarked_ledger_hash = state
                    .transition_frontier
                    .previous_root_snarked_ledger_hash();

                dispatcher.push(TransitionFrontierSyncAction::BestTipUpdate {
                    previous_root_snarked_ledger_hash,
                    best_tip: best_tip.clone(),
                    root_block,
                    blocks_inbetween,
                    on_success: Some(callback!(
                        on_transition_frontier_sync_best_tip_update(_p: ()) -> crate::Action{
                            BlockProducerAction::BlockInjected
                        }
                    )),
                });
            }
            BlockProducerAction::BlockInjected => {
                if let BlockProducerCurrentState::Produced {
                    won_slot,
                    chain,
                    block,
                    ..
                } = &mut state.current
                {
                    state.injected_blocks.insert(block.hash().clone());
                    state.current = BlockProducerCurrentState::Injected {
                        time: meta.time(),
                        won_slot: won_slot.clone(),
                        chain: std::mem::take(chain),
                        block: block.clone(),
                    };
                } else {
                    bug_condition!("Invalid state for `BlockProducerAction::BlockInjected` expected: `BlockProducerCurrentState::Produced`, found: {:?}", state.current);
                }

                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();

                #[cfg(feature = "p2p-libp2p")]
                broadcast_injected_block(global_state, dispatcher);

                dispatcher.push(BlockProducerAction::WonSlotSearch);
            }
        }
    }

    /// Reuse exactly the unproven block and its witnesses once; an expired or
    /// twice-failed slot is removed so WonSlotSearch cannot select it again.
    pub(super) fn retry_failed_proof(&mut self, hash: &v2::StateHash, time: Timestamp) -> bool {
        if !matches!(&self.current, BlockProducerCurrentState::BlockProvePending { block_hash, .. } if block_hash == hash)
        {
            return false;
        }
        let BlockProducerCurrentState::BlockProvePending {
            won_slot,
            chain,
            emitted_ledger_proof,
            pending_coinbase_update,
            pending_coinbase_witness,
            stake_proof_sparse_ledger,
            block,
            block_hash,
            ..
        } = std::mem::take(&mut self.current)
        else {
            unreachable!()
        };
        if self.proof_retried || time >= won_slot.next_slot_time() {
            self.vrf_evaluator.won_slots.remove(&won_slot.global_slot());
            self.current = BlockProducerCurrentState::Idle { time };
            return false;
        }
        self.proof_retried = true;
        self.current = BlockProducerCurrentState::BlockUnprovenBuilt {
            time,
            won_slot,
            chain,
            emitted_ledger_proof,
            pending_coinbase_update,
            pending_coinbase_witness,
            stake_proof_sparse_ledger,
            block,
            block_hash,
        };
        true
    }

    fn reduce_block_unproved_build(
        &mut self,
        consensus_constants: &ConsensusConstants,
        time: Timestamp,
    ) {
        if !self.current.parent_epoch_data_matches() {
            // Checked when the parent was chosen; the chain is fixed since.
            bug_condition!("BlockUnprovenBuild with a parent selecting other epoch data");
            return;
        }
        let current_state = std::mem::take(&mut self.current);

        let BlockProducerCurrentState::StagedLedgerDiffCreateSuccess {
            won_slot,
            chain,
            diff,
            diff_hash,
            staged_ledger_hash,
            emitted_ledger_proof,
            pending_coinbase_update,
            pending_coinbase_witness,
            stake_proof_sparse_ledger,
            ..
        } = current_state
        else {
            bug_condition!("Invalid state for `BlockProducerAction::BlockUnprovenBuild` expected: `BlockProducerCurrentState::StagedLedgerDiffCreateSuccess`, found: {:?}", current_state);
            return;
        };

        let (block, block_hash) = match build_unproven_block(
            consensus_constants,
            &self.config,
            &won_slot,
            &chain,
            &diff,
            &diff_hash,
            &staged_ledger_hash,
            emitted_ledger_proof.as_deref(),
            &stake_proof_sparse_ledger,
        ) {
            Ok(built) => built,
            Err(error) => {
                mina_core::log::inner::error!("Unproven block build failed: {error}");
                return;
            }
        };

        self.current = BlockProducerCurrentState::BlockUnprovenBuilt {
            time,
            won_slot,
            chain,
            emitted_ledger_proof,
            pending_coinbase_update,
            pending_coinbase_witness,
            stake_proof_sparse_ledger,
            block,
            block_hash,
        };
    }

    fn dispatch_best_tip_update(
        dispatcher: &mut Dispatcher<Action, State>,
        state: &State,
        best_tip: &ArcBlockWithHash,
        time: Timestamp,
    ) {
        dispatcher.push(BlockProducerVrfEvaluatorAction::InitializeEvaluator {
            best_tip: best_tip.clone(),
        });
        Self::dispatch_evaluability_check(dispatcher, state, best_tip);

        if let Some(reason) = state.block_producer.with(None, |bp| {
            bp.current.won_slot_should_discard(best_tip, time)
        }) {
            dispatcher.push(BlockProducerAction::WonSlotDiscard { reason });
        } else {
            dispatcher.push(BlockProducerAction::WonSlotSearch);
        }
    }

    /// Interrupts an evaluation the best tip or the clock has made useless,
    /// then checks which epoch to evaluate. Any change of the selected epoch
    /// data, from a new tip, a reorg or a delayed block, restarts the
    /// evaluation, completed or not.
    fn dispatch_evaluability_check(
        dispatcher: &mut Dispatcher<Action, State>,
        state: &State,
        best_tip: &ArcBlockWithHash,
    ) {
        let global_slot = best_tip
            .consensus_state()
            .curr_global_slot_since_hard_fork
            .clone();

        let (best_tip_epoch, best_tip_slot) = to_epoch_and_slot(&global_slot);
        let root_block_epoch = if let Some(root_block) = state.transition_frontier.root() {
            let root_block_global_slot = root_block.curr_global_slot_since_hard_fork();
            to_epoch_and_slot(root_block_global_slot).0
        } else {
            bug_condition!("Expected to find a block at the root of the transition frontier but there was none");
            best_tip_epoch.saturating_sub(1)
        };
        let next_epoch_first_slot = next_epoch_first_slot(&global_slot);
        let current_epoch = state.current_epoch();
        let current_slot = state.current_slot();

        let is_next_epoch_seed_finalized = if let Some(current_slot) = current_slot {
            !in_seed_update_range(current_slot, best_tip.constants())
        } else {
            false
        };

        // An evaluation bound to data the best tip no longer selects for its
        // epoch, or to an epoch the clock has passed, can no longer produce a
        // block: restart instead of finishing it.
        if let Some(target) = state.block_producer.vrf_evaluator().and_then(|vrf| {
            vrf.target_is_stale(best_tip.consensus_state(), current_epoch)
                .then(|| vrf.target().cloned())
                .flatten()
        }) {
            let reason = if target.epoch < best_tip_epoch {
                InterruptReason::BestTipWithHigherEpoch
            } else {
                InterruptReason::EpochDataChanged
            };
            dispatcher.push(BlockProducerVrfEvaluatorAction::InterruptEpochEvaluation { reason });
        }

        dispatcher.push(BlockProducerVrfEvaluatorAction::CheckEpochEvaluability {
            current_epoch,
            is_next_epoch_seed_finalized,
            root_block_epoch,
            best_tip_epoch,
            best_tip_slot,
            best_tip_global_slot: best_tip.global_slot(),
            next_epoch_first_slot,
            staking_epoch_data: Box::new(best_tip.consensus_state().staking_epoch_data.clone()),
            next_epoch_data: Box::new(best_tip.consensus_state().next_epoch_data.clone()),
        });
    }
}

#[cfg(feature = "p2p-libp2p")]
fn broadcast_injected_block(global_state: &State, dispatcher: &mut Dispatcher<Action, State>) {
    use mina_p2p_messages::gossip::GossipNetMessageV2;

    let Some((block_hash, block)) = global_state
        .block_producer
        .as_ref()
        .and_then(|bp| bp.current.injected_block())
        .map(|pb| (pb.hash().to_string(), pb.block.clone()))
    else {
        // Should be impossible, we call this immediately after having injected the block.
        return;
    };

    let state_hash = block.header.protocol_state.try_hash().ok();
    let slot = block
        .header
        .protocol_state
        .body
        .consensus_state
        .curr_global_slot_since_hard_fork
        .slot_number
        .as_u32();
    mina_core::debug!(mina_core::log::system_time();
        kind = "ProducerBlockQueuedForGossip",
        state_hash = state_hash.map(|hash| hash.to_string()),
        slot = slot,
    );
    // OCaml adopts a successful local proof even after its slot and attempts
    // broadcast. Keep the stale-gossip guard; default catch-up RPCs expose the
    // retained block when the next timely descendant reaches the peer.
    // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L853-L1026>
    let message = GossipNetMessageV2::NewState(block);
    if let Some(p2p) = global_state.p2p.ready() {
        if !p2p
            .network
            .scheduler
            .broadcast_state
            .block_gossip
            .can_announce(&message, global_state.time())
        {
            mina_core::warn!(global_state.time(); kind = "BlockProducerAnnouncementSuppressed", block_hash = block_hash,
                message = "Produced block retained for catch-up; gossip announcement suppressed");
        }
    }
    dispatcher.push(P2pNetworkPubsubAction::Broadcast { message });
}
