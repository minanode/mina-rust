use mina_core::bug_condition;
use mina_vrf::VrfEvaluationOutput;

use crate::{
    block_producer::to_epoch_and_slot,
    block_producer_effectful::vrf_evaluator_effectful::BlockProducerVrfEvaluatorEffectfulAction,
    ledger::read::{LedgerReadAction, LedgerReadInitCallback, LedgerReadRequest},
    BlockProducerAction, Substate,
};

use super::{
    BlockProducerVrfEvaluatorAction, BlockProducerVrfEvaluatorActionWithMetaRef,
    BlockProducerVrfEvaluatorState, BlockProducerVrfEvaluatorStatus, EpochFingerprint,
    PendingEvaluation, SlotPositionInEpoch, VrfWonSlotWithHash,
};

impl BlockProducerVrfEvaluatorState {
    pub fn reducer(
        mut state_context: Substate<Self>,
        action: BlockProducerVrfEvaluatorActionWithMetaRef<'_>,
    ) {
        let Ok(state) = state_context.get_substate_mut() else {
            return;
        };

        let (action, meta) = action.split();
        match action {
            BlockProducerVrfEvaluatorAction::EvaluateSlot { vrf_input } => {
                state.status = BlockProducerVrfEvaluatorStatus::SlotEvaluationPending {
                    time: meta.time(),
                    global_slot: vrf_input.global_slot,
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(BlockProducerVrfEvaluatorEffectfulAction::EvaluateSlot {
                    vrf_input: vrf_input.clone(),
                });
            }
            BlockProducerVrfEvaluatorAction::ProcessSlotEvaluationSuccess {
                vrf_output,
                staking_ledger_hash: _,
                generation: _,
            } => {
                // The enabling condition matched the response to the pending
                // evaluation's generation, slot and ledger. Its win is bound
                // to the epoch data it was evaluated with.
                let Some(epoch_data) = state.current_evaluation().map(|p| p.fingerprint()) else {
                    bug_condition!("ProcessSlotEvaluationSuccess without a pending evaluation");
                    return;
                };
                let mut newly_won = false;
                let global_slot_evaluated = match &vrf_output {
                    mina_vrf::VrfEvaluationOutput::SlotWon(won_slot_data) => {
                        if state
                            .won_slots
                            .insert(
                                won_slot_data.global_slot,
                                VrfWonSlotWithHash::new(won_slot_data.clone(), epoch_data),
                            )
                            .is_none()
                        {
                            state.total_won_slots = state.total_won_slots.saturating_add(1);
                            newly_won = true;
                        }
                        won_slot_data.global_slot
                    }
                    mina_vrf::VrfEvaluationOutput::SlotLost(global_slot) => *global_slot,
                };
                state.set_latest_evaluated_global_slot(&global_slot_evaluated);

                state.status = BlockProducerVrfEvaluatorStatus::SlotEvaluationReceived {
                    time: meta.time(),
                    global_slot: global_slot_evaluated,
                };

                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                if let Some(vrf_evaluator_state) = state.block_producer.vrf_evaluator() {
                    if let Some(pending_evaluation) = vrf_evaluator_state.current_evaluation() {
                        dispatcher.push(BlockProducerVrfEvaluatorEffectfulAction::SlotEvaluated {
                            epoch: pending_evaluation.epoch_number,
                            won: newly_won,
                        });
                        dispatcher.push(BlockProducerVrfEvaluatorAction::CheckEpochBounds {
                            epoch_number: pending_evaluation.epoch_number,
                            latest_evaluated_global_slot: vrf_output.global_slot(),
                        });
                    }
                }

                if matches!(vrf_output, VrfEvaluationOutput::SlotWon(_)) {
                    dispatcher.push(BlockProducerAction::WonSlotSearch);
                }
            }
            BlockProducerVrfEvaluatorAction::CheckEpochBounds {
                epoch_number,
                latest_evaluated_global_slot,
            } => {
                let latest_evaluated_global_slot = *latest_evaluated_global_slot;
                let epoch_number = *epoch_number;

                let epoch_current_bound =
                    Self::evaluate_epoch_bounds(&latest_evaluated_global_slot);
                state.status = BlockProducerVrfEvaluatorStatus::EpochBoundsCheck {
                    time: meta.time(),
                    epoch_number,
                    latest_evaluated_global_slot,
                    epoch_current_bound,
                };

                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                if let Some(epoch_bound) = state
                    .block_producer
                    .vrf_evaluator()
                    .and_then(|s| s.get_epoch_bound_from_check())
                {
                    match epoch_bound {
                        SlotPositionInEpoch::Beginning | SlotPositionInEpoch::Within
                            if latest_evaluated_global_slot < u32::MAX =>
                        {
                            dispatcher.push(
                                BlockProducerVrfEvaluatorAction::ContinueEpochEvaluation {
                                    latest_evaluated_global_slot,
                                    epoch_number,
                                },
                            );
                        }
                        _ => {
                            // The protocol has no successor to its maximum
                            // slot, even when that slot is inside an epoch.
                            dispatcher.push(
                                BlockProducerVrfEvaluatorAction::FinishEpochEvaluation {
                                    latest_evaluated_global_slot,
                                    epoch_number,
                                },
                            );
                        }
                    }
                }
            }
            BlockProducerVrfEvaluatorAction::InitializeEvaluator { best_tip } => {
                state.status =
                    BlockProducerVrfEvaluatorStatus::InitialisationPending { time: meta.time() };

                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                // Note: pure function, but needs access to other parts of the state
                if state.block_producer.vrf_evaluator().is_some() {
                    if best_tip.consensus_state().epoch_count.as_u32() == 0 {
                        dispatcher.push(
                            BlockProducerVrfEvaluatorAction::FinalizeEvaluatorInitialization {
                                previous_epoch_and_height: None,
                            },
                        );
                    } else {
                        let k = best_tip.constants().k.as_u32();
                        let (epoch, slot) = to_epoch_and_slot(
                            &best_tip.consensus_state().curr_global_slot_since_hard_fork,
                        );
                        let previous_epoch = epoch.saturating_sub(1);
                        let last_height = if slot < k {
                            let found =
                                state.transition_frontier.best_chain.iter().rev().find(|b| {
                                    b.consensus_state().epoch_count.as_u32() == previous_epoch
                                });

                            if let Some(block) = found {
                                block.height()
                            } else {
                                Default::default()
                            }
                        } else if let Some(root_block) = state.transition_frontier.root() {
                            root_block.height()
                        } else {
                            Default::default()
                        };
                        dispatcher.push(
                            BlockProducerVrfEvaluatorAction::FinalizeEvaluatorInitialization {
                                previous_epoch_and_height: Some((previous_epoch, last_height)),
                            },
                        );
                    }
                }
            }
            BlockProducerVrfEvaluatorAction::FinalizeEvaluatorInitialization {
                previous_epoch_and_height,
            } => {
                if let Some((epoch, last_height)) = previous_epoch_and_height {
                    state.initialize_evaluator(*epoch, *last_height);
                }
                state.status =
                    BlockProducerVrfEvaluatorStatus::InitialisationComplete { time: meta.time() }
            }
            BlockProducerVrfEvaluatorAction::CheckEpochEvaluability {
                current_epoch,
                is_next_epoch_seed_finalized,
                best_tip_epoch,
                root_block_epoch,
                staking_epoch_data,
                next_epoch_data,
                best_tip_slot,
                best_tip_global_slot,
                next_epoch_first_slot,
            } => {
                let best_tip_epoch = *best_tip_epoch;
                let current =
                    EpochFingerprint::new(best_tip_epoch, &(**staking_epoch_data).clone().into());
                let next = best_tip_epoch
                    .checked_add(1)
                    .map(|epoch| EpochFingerprint::new(epoch, &(**next_epoch_data).clone().into()));

                state.set_checked_clock_epoch(*current_epoch);
                state.status = BlockProducerVrfEvaluatorStatus::ReadinessCheck {
                    time: meta.time(),
                    current_epoch: *current_epoch,
                    // No next epoch can be evaluated after the terminal protocol epoch.
                    is_next_epoch_seed_finalized: *is_next_epoch_seed_finalized
                        && next_epoch_first_slot.is_some(),
                    best_tip_epoch,
                    root_block_epoch: *root_block_epoch,
                    is_current_epoch_evaluated: state.is_evaluated(&current),
                    is_next_epoch_evaluated: next.is_some_and(|next| state.is_evaluated(&next)),
                    last_evaluated_epoch: state.last_evaluated_epoch(),
                    staking_epoch_data: staking_epoch_data.clone(),
                    next_epoch_data: next_epoch_data.clone(),
                };

                state.set_epoch_context();

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let vrf_evaluator_state = state.block_producer.vrf_evaluator_with_config();

                if let Some((vrf_evaluator_state, config)) = vrf_evaluator_state {
                    if let Some(epoch_data) = vrf_evaluator_state.epoch_context().get_epoch_data() {
                        // Current or Next: the tip's epoch or the one after.
                        dispatcher.push(
                            BlockProducerVrfEvaluatorAction::InitializeEpochEvaluation {
                                staking_epoch_data: epoch_data,
                                producer: config.pub_key.clone().into(),
                                best_tip_global_slot: *best_tip_global_slot,
                                best_tip_epoch,
                                best_tip_slot: *best_tip_slot,
                                next_epoch_first_slot: *next_epoch_first_slot,
                            },
                        );
                    } else {
                        // If None is returned, than we are waiting for evaluation
                        dispatcher.push(BlockProducerVrfEvaluatorAction::WaitForNextEvaluation);
                    }

                    dispatcher
                        .push(BlockProducerVrfEvaluatorAction::CleanupOldSlots { best_tip_epoch });
                }
            }
            BlockProducerVrfEvaluatorAction::InitializeEpochEvaluation {
                best_tip_epoch,
                best_tip_slot,
                best_tip_global_slot,
                next_epoch_first_slot,
                staking_epoch_data,
                producer,
            } => {
                let epoch = match state.epoch_context() {
                    super::EpochContext::Next(_) => best_tip_epoch.checked_add(1),
                    _ => Some(*best_tip_epoch),
                };
                let Some(epoch) = epoch else {
                    bug_condition!("InitializeEpochEvaluation past the terminal epoch");
                    return;
                };
                let target = EpochFingerprint::new(epoch, staking_epoch_data);
                state.select_target(target);
                state.status = BlockProducerVrfEvaluatorStatus::ReadyToEvaluate {
                    time: meta.time(),
                    best_tip_epoch: *best_tip_epoch,
                    is_current_epoch_evaluated: state.completed_epoch(*best_tip_epoch).is_some(),
                    is_next_epoch_evaluated: best_tip_epoch
                        .checked_add(1)
                        .is_some_and(|next| state.completed_epoch(next).is_some()),
                    best_tip_slot: *best_tip_slot,
                    best_tip_global_slot: *best_tip_global_slot,
                    next_epoch_first_slot: *next_epoch_first_slot,
                    staking_epoch_data: staking_epoch_data.clone(),
                    producer: producer.clone(),
                };

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(BlockProducerVrfEvaluatorAction::BeginDelegatorTableConstruction);
            }
            BlockProducerVrfEvaluatorAction::BeginDelegatorTableConstruction => {
                let BlockProducerVrfEvaluatorStatus::ReadyToEvaluate {
                    best_tip_epoch,
                    best_tip_slot,
                    best_tip_global_slot,
                    next_epoch_first_slot,
                    staking_epoch_data,
                    producer,
                    time: _,
                    is_current_epoch_evaluated: _,
                    is_next_epoch_evaluated: _,
                } = &state.status
                else {
                    return;
                };
                state.status = BlockProducerVrfEvaluatorStatus::EpochDelegatorTablePending {
                    time: meta.time(),
                    best_tip_epoch: *best_tip_epoch,
                    staking_epoch_ledger_hash: staking_epoch_data.ledger.clone(),
                    best_tip_slot: *best_tip_slot,
                    best_tip_global_slot: *best_tip_global_slot,
                    next_epoch_first_slot: *next_epoch_first_slot,
                    staking_epoch_data: staking_epoch_data.clone(),
                    producer: producer.clone(),
                };

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let (staking_ledger_hash, producer) =
                    match state.block_producer.vrf_delegator_table_inputs() {
                        Some((v1, v2)) => (v1.clone(), v2.clone()),
                        None => return,
                    };

                dispatcher.push(LedgerReadAction::Init {
                    request: LedgerReadRequest::DelegatorTable(staking_ledger_hash, producer),
                    callback: LedgerReadInitCallback::None,
                })
            }
            BlockProducerVrfEvaluatorAction::FinalizeDelegatorTableConstruction {
                delegator_table,
            } => {
                let BlockProducerVrfEvaluatorStatus::EpochDelegatorTablePending {
                    best_tip_epoch,
                    best_tip_slot,
                    best_tip_global_slot,
                    next_epoch_first_slot,
                    staking_epoch_data,
                    producer,
                    time: _,
                    staking_epoch_ledger_hash: _,
                } = &state.status
                else {
                    bug_condition!("Invalid state for `BlockProducerVrfEvaluatorAction::FinalizeDelegatorTableConstruction` expected: `BlockProducerVrfEvaluatorStatus::EpochDelegatorTablePending`, found: {:?}", state.status);
                    return;
                };

                // No ledger with the selected hash: never evaluate it as a
                // ledger without our stake. Ask again on a later best tip, or
                // after a while.
                let Some(delegator_table) = delegator_table else {
                    mina_core::log::warn!(
                        meta.time();
                        kind = "BlockProducerVrfEvaluatorAction::FinalizeDelegatorTableConstruction",
                        message = "Staking ledger not available; waiting for it before evaluating",
                        ledger = staking_epoch_data.ledger.to_string(),
                    );
                    state.set_ledger_unavailable(meta.time());
                    state.status = BlockProducerVrfEvaluatorStatus::EpochEvaluationInterrupted {
                        time: meta.time(),
                        reason: super::InterruptReason::StakingLedgerUnavailable,
                    };
                    return;
                };
                if delegator_table.is_empty() {
                    mina_core::log::warn!(
                        meta.time();
                        kind = "BlockProducerVrfEvaluatorAction::FinalizeDelegatorTableConstruction",
                        message = "Empty delegator table, account may not exist yet in the staking ledger"
                    );
                }

                let mut staking_epoch_data = staking_epoch_data.clone();
                staking_epoch_data.delegator_table = delegator_table.clone();

                state.status = BlockProducerVrfEvaluatorStatus::EpochDelegatorTableSuccess {
                    time: meta.time(),
                    best_tip_epoch: *best_tip_epoch,
                    staking_epoch_ledger_hash: staking_epoch_data.ledger.clone(),
                    best_tip_slot: *best_tip_slot,
                    best_tip_global_slot: *best_tip_global_slot,
                    next_epoch_first_slot: *next_epoch_first_slot,
                    staking_epoch_data: staking_epoch_data.clone(),
                    producer: producer.clone(),
                };

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let get_slot_and_status = || {
                    let cur_global_slot = state.cur_global_slot()?;
                    let current_slot = state.current_slot()?;
                    let status = &state.block_producer.vrf_evaluator()?.status;

                    Some((cur_global_slot, current_slot, status))
                };

                let Some((
                    current_global_slot,
                    current_slot,
                    BlockProducerVrfEvaluatorStatus::EpochDelegatorTableSuccess {
                        best_tip_epoch,
                        best_tip_slot,
                        best_tip_global_slot,
                        next_epoch_first_slot,
                        staking_epoch_data,
                        ..
                    },
                )) = get_slot_and_status()
                else {
                    bug_condition!("Invalid state for `BlockProducerVrfEvaluatorAction::FinalizeDelegatorTableConstruction`");
                    return;
                };

                dispatcher.push(BlockProducerVrfEvaluatorAction::SelectInitialSlot {
                    current_global_slot,
                    current_slot,
                    best_tip_epoch: *best_tip_epoch,
                    best_tip_slot: *best_tip_slot,
                    best_tip_global_slot: *best_tip_global_slot,
                    next_epoch_first_slot: *next_epoch_first_slot,
                    staking_epoch_data: staking_epoch_data.clone(),
                });
            }
            BlockProducerVrfEvaluatorAction::BeginEpochEvaluation {
                best_tip_epoch: _,
                evaluation_epoch,
                latest_evaluated_global_slot,
                staking_epoch_data,
                best_tip_slot: _,
                best_tip_global_slot: _,
            } => {
                let latest_evaluated_global_slot = *latest_evaluated_global_slot;
                let epoch_number = *evaluation_epoch;

                state.set_pending_evaluation(PendingEvaluation {
                    epoch_number,
                    epoch_data: staking_epoch_data.clone(),
                    latest_evaluated_slot: latest_evaluated_global_slot,
                    generation: state.generation(),
                });
                state.status = BlockProducerVrfEvaluatorStatus::EpochEvaluationPending {
                    time: meta.time(),
                    epoch_number,
                    epoch_data: staking_epoch_data.clone(),
                    latest_evaluated_global_slot,
                };

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                if state.block_producer.vrf_evaluator().is_some() {
                    dispatcher.push(BlockProducerVrfEvaluatorAction::ContinueEpochEvaluation {
                        latest_evaluated_global_slot,
                        epoch_number,
                    });
                }
            }
            BlockProducerVrfEvaluatorAction::ContinueEpochEvaluation {
                epoch_number,
                latest_evaluated_global_slot,
            } => {
                if let Some(pending_evaluation) = state.current_evaluation() {
                    state.status = BlockProducerVrfEvaluatorStatus::EpochEvaluationPending {
                        time: meta.time(),
                        epoch_number: *epoch_number,
                        epoch_data: pending_evaluation.epoch_data,
                        latest_evaluated_global_slot: *latest_evaluated_global_slot,
                    };
                }

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                if let Some(vrf_evaluator_state) = state.block_producer.vrf_evaluator() {
                    if let Some(vrf_input) = vrf_evaluator_state.construct_vrf_input() {
                        dispatcher
                            .push(BlockProducerVrfEvaluatorAction::EvaluateSlot { vrf_input });
                    }
                }
            }
            BlockProducerVrfEvaluatorAction::FinishEpochEvaluation {
                epoch_number,
                latest_evaluated_global_slot: _,
            } => {
                let completed = state.current_evaluation().map(|p| p.fingerprint());
                state.clear_target();
                state.status = BlockProducerVrfEvaluatorStatus::EpochEvaluationSuccess {
                    time: meta.time(),
                    epoch_number: *epoch_number,
                };
                if let Some(completed) = completed {
                    state.mark_epoch_evaluated(completed);
                } else {
                    bug_condition!("FinishEpochEvaluation without a pending evaluation");
                }
            }
            BlockProducerVrfEvaluatorAction::WaitForNextEvaluation => {
                state.status =
                    BlockProducerVrfEvaluatorStatus::WaitingForNextEvaluation { time: meta.time() };
            }
            BlockProducerVrfEvaluatorAction::SelectInitialSlot {
                best_tip_epoch,
                staking_epoch_data,
                ..
            } => {
                let epoch_number = match state.epoch_context() {
                    super::EpochContext::Current(_) => *best_tip_epoch,
                    super::EpochContext::Next(_) => {
                        best_tip_epoch.checked_add(1).expect("overflow")
                    }
                    super::EpochContext::Waiting => {
                        // Setup can only follow a Current/Next readiness check.
                        bug_condition!(
                            "SelectInitialSlot requires a Current or Next epoch context"
                        );
                        return;
                    }
                };
                let mut state_context: Substate<crate::State> =
                    Substate::from_compatible_substate(state_context);
                let Ok(state) = state_context.get_substate_mut() else {
                    return;
                };
                let best_tip = state.transition_frontier.best_tip().cloned();
                let current_global_slot = state.cur_global_slot_at(meta.time());
                // The table may have been requested before a sleep or catch-up.
                // Bind its seed, ledger and stake to the epoch of the slot we
                // will actually evaluate, using the latest tip and clock.
                // OCaml: <https://github.com/MinaProtocol/mina/blob/73134bfd3a43c3f4d8ee564b3d04a0c6ed2e0b3c/src/lib/consensus/proof_of_stake.ml#L2902-L2950>
                // OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/vrf_evaluator/vrf_evaluator.ml#L393-L424>
                let selected = EpochFingerprint::new(epoch_number, staking_epoch_data);
                let valid = state
                    .block_producer
                    .vrf_evaluator()
                    .is_some_and(|e| e.target() == Some(&selected))
                    && best_tip
                        .as_ref()
                        .zip(current_global_slot)
                        .is_some_and(|(tip, slot)| {
                            let slots_per_epoch = tip.constants().slots_per_epoch.as_u32();
                            slot / slots_per_epoch <= epoch_number
                                && EpochFingerprint::select(tip.consensus_state(), epoch_number)
                                    == Some(selected)
                        });
                if !valid {
                    let dispatcher = state_context.into_dispatcher();
                    dispatcher.push(BlockProducerVrfEvaluatorAction::InterruptEpochEvaluation {
                        reason: super::InterruptReason::EpochDataStale,
                    });
                    if let Some(best_tip) = best_tip {
                        // Reuse readiness/recovery: wait if the tip still lags,
                        // otherwise request the correct epoch's table now.
                        dispatcher.push(BlockProducerAction::BestTipUpdate { best_tip });
                    }
                    return;
                }
                let best_tip = best_tip.expect("validated best tip");
                let slots_per_epoch = best_tip.constants().slots_per_epoch.as_u32();
                // Include a current-slot win, and retain a precomputed next
                // epoch when it becomes current. Slot zero is genesis. The
                // evaluator cursor is the slot BEFORE the first request.
                // OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/block_producer/block_producer.ml#L1133-L1209>
                let first_slot = current_global_slot
                    .expect("validated clock")
                    .max(epoch_number.checked_mul(slots_per_epoch).expect("overflow"))
                    .max(1);
                let initial_global_slot = first_slot - 1;
                let Some(evaluator) = state
                    .block_producer
                    .as_mut()
                    .map(|bp| &mut bp.vrf_evaluator)
                else {
                    return;
                };
                evaluator.status = BlockProducerVrfEvaluatorStatus::InitialSlotSelection {
                    time: meta.time(),
                    epoch_number,
                    initial_slot: initial_global_slot,
                };

                // The key's stake in the epoch's staking ledger: its delegators'
                // balances (its own included) against the ledger's total.
                let delegated_stake = staking_epoch_data
                    .delegator_table
                    .values()
                    .fold(0u64, |sum, (_, balance)| sum.saturating_add(*balance));
                let total_currency = staking_epoch_data.total_currency;
                let delegators =
                    u32::try_from(staking_epoch_data.delegator_table.len()).unwrap_or(u32::MAX);

                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(BlockProducerVrfEvaluatorEffectfulAction::InitializeStats {
                    epoch: epoch_number,
                    initial_slot: first_slot % slots_per_epoch,
                    delegated_stake,
                    total_currency,
                    delegators,
                });
                dispatcher.push(BlockProducerVrfEvaluatorAction::BeginEpochEvaluation {
                    best_tip_epoch: to_epoch_and_slot(best_tip.curr_global_slot_since_hard_fork())
                        .0,
                    best_tip_global_slot: best_tip.global_slot(),
                    best_tip_slot: best_tip.global_slot() % slots_per_epoch,
                    evaluation_epoch: epoch_number,
                    staking_epoch_data: staking_epoch_data.clone(),
                    latest_evaluated_global_slot: initial_global_slot,
                });
            }
            BlockProducerVrfEvaluatorAction::CleanupOldSlots { best_tip_epoch } => {
                state.cleanup_old_won_slots(best_tip_epoch);
            }
            BlockProducerVrfEvaluatorAction::InterruptEpochEvaluation { reason } => {
                state.clear_target();
                state.status = BlockProducerVrfEvaluatorStatus::EpochEvaluationInterrupted {
                    time: meta.time(),
                    reason: reason.clone(),
                };
            }
        }
    }
}
