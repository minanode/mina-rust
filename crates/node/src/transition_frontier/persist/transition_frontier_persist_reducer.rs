use std::time::Duration;

use mina_core::block::AppliedBlock;
use mina_p2p_messages::v2::LedgerHash;

use super::{
    PersistCheckpointState, PersistLastCheckpoint, PersistReadyState, PersistRestoreInfo,
    PersistRestoreOutcome, PersistRestoreState, TransitionFrontierPersistAction,
    TransitionFrontierPersistActionWithMetaRef, TransitionFrontierPersistConfig,
    TransitionFrontierPersistState, CHECKPOINT_RETRY_SECS, RESTORE_CHECKPOINT_SLACK_BLOCKS,
    STALE_SLOTS_PER_K,
};
use crate::{
    ledger::{
        write::{LedgerWriteAction, LedgerWriteRequest},
        PersistCheckpointRequest,
    },
    persist::{event::PersistStoreSummary, load::PersistLoadRequest, ChainEntry, Checkpoint},
    transition_frontier::{
        persist_effectful::TransitionFrontierPersistEffectfulAction,
        sync::TransitionFrontierSyncAction,
    },
};

fn ms_since(now: redux::Timestamp, since: redux::Timestamp) -> f64 {
    now.checked_sub(since)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1000.0)
}

fn checkpoint_ledger_hashes(root: &AppliedBlock, tip: &AppliedBlock) -> Vec<LedgerHash> {
    let mut ledgers = vec![
        root.snarked_ledger_hash().clone(),
        tip.staking_epoch_ledger_hash().clone(),
        tip.next_epoch_ledger_hash().clone(),
    ];
    // I5e must retain the old staking snapshot while the tip has crossed
    // but the root has not. Deduplicate the old next / new staking hash.
    for hash in [
        root.staking_epoch_ledger_hash(),
        root.next_epoch_ledger_hash(),
    ] {
        if !ledgers.contains(hash) {
            ledgers.push(hash.clone());
        }
    }
    ledgers
}

impl TransitionFrontierPersistState {
    pub fn reducer(
        mut state_context: crate::Substate<Self>,
        action: TransitionFrontierPersistActionWithMetaRef<'_>,
        best_chain: &[AppliedBlock],
        config: &TransitionFrontierPersistConfig,
    ) {
        let Ok(state) = state_context.get_substate_mut() else {
            return;
        };
        let (action, meta) = action.split();

        match action {
            TransitionFrontierPersistAction::OpenInit => {
                *state = Self::OpenPending { time: meta.time() };
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let (Some(p2p), Some(genesis)) = (
                    global_state.p2p.ready(),
                    global_state
                        .transition_frontier
                        .genesis
                        .block_with_real_or_dummy_proof(),
                ) else {
                    return;
                };
                dispatcher.push(TransitionFrontierPersistEffectfulAction::Open {
                    chain_id: p2p.chain_id.clone(),
                    genesis_state_hash: genesis.hash().clone(),
                });
            }
            TransitionFrontierPersistAction::OpenSuccess { opened } => {
                *state = Self::Ready(Box::new(PersistReadyState {
                    opened_at: meta.time(),
                    dir: opened.dir.clone(),
                    open_outcome: opened.outcome.clone(),
                    store: opened.store.clone(),
                    chain_put_sent: None,
                    chain_puts_pending: 0,
                    checkpoint: PersistCheckpointState::Idle,
                    last_checkpoint: None,
                    retry_after: None,
                    stats: Default::default(),
                    checkpoint_chain_len_limit: None,
                    restore: if config.restore {
                        PersistRestoreState::Idle
                    } else {
                        PersistRestoreState::Done {
                            time: meta.time(),
                            outcome: PersistRestoreOutcome::Off,
                        }
                    },
                }));
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(TransitionFrontierPersistAction::LoadInit);
                dispatcher.push(TransitionFrontierPersistAction::ChainPutInit);
            }
            TransitionFrontierPersistAction::OpenError { error } => {
                *state = Self::Disabled {
                    reason: error.clone(),
                };
            }
            TransitionFrontierPersistAction::OpenTimeout => {
                *state = Self::Disabled {
                    reason: format!("the store didn't open within {}s", config.load_timeout_secs),
                };
            }
            TransitionFrontierPersistAction::LoadInit => {
                let Some(ready) = state.ready_mut() else {
                    return;
                };
                ready.restore = PersistRestoreState::LoadPending {
                    time: meta.time(),
                    completed_stages: 0,
                    last_progress_time: None,
                };
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let Some(genesis) = global_state.genesis_block() else {
                    return;
                };
                let k = genesis.constants().k.as_u32();
                dispatcher.push(TransitionFrontierPersistEffectfulAction::Load {
                    request: PersistLoadRequest {
                        current_slot: global_state.cur_global_slot(),
                        stale_slots: k.saturating_mul(STALE_SLOTS_PER_K),
                        genesis: Some(genesis.clone()),
                    },
                });
            }
            TransitionFrontierPersistAction::LoadSuccess { restored } => {
                let Some(ready) = state.ready_mut() else {
                    return;
                };
                let PersistRestoreState::LoadPending {
                    time: load_started, ..
                } = ready.restore
                else {
                    return;
                };
                let tip = restored.tip();
                ready.restore = PersistRestoreState::Replaying {
                    time: meta.time(),
                    info: Box::new(PersistRestoreInfo {
                        generation: restored.generation,
                        attempt: restored.attempt,
                        root: restored.root.hash().clone(),
                        root_height: restored.root.height(),
                        tip: tip.hash().clone(),
                        tip_height: tip.height(),
                        blocks: restored.blocks.len() as u32,
                        ledgers: restored.ledgers.len() as u32,
                        load_started,
                        timings: restored.timings.clone(),
                    }),
                };
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(TransitionFrontierSyncAction::RestoreInit {
                    root: restored.root.clone(),
                    blocks: restored.blocks.clone(),
                    needed_protocol_states: restored.needed_protocol_states.clone(),
                });
            }
            TransitionFrontierPersistAction::LoadLedgersOnly { reason, ledgers } => {
                restore_done(
                    state,
                    meta.time(),
                    PersistRestoreOutcome::LedgersOnly {
                        reason: reason.clone(),
                        ledgers: *ledgers,
                    },
                );
            }
            TransitionFrontierPersistAction::LoadAbsent { reason } => {
                restore_done(
                    state,
                    meta.time(),
                    PersistRestoreOutcome::Absent {
                        reason: reason.clone(),
                    },
                );
            }
            TransitionFrontierPersistAction::LoadDiscarded { reason } => {
                if let Some(ready) = state.ready_mut() {
                    // The store thread deleted it.
                    ready.store = PersistStoreSummary::default();
                }
                restore_done(
                    state,
                    meta.time(),
                    PersistRestoreOutcome::Discarded {
                        reason: reason.clone(),
                    },
                );
            }
            TransitionFrontierPersistAction::LoadError { error } => {
                restore_done(
                    state,
                    meta.time(),
                    PersistRestoreOutcome::Failed {
                        error: error.clone(),
                    },
                );
            }
            TransitionFrontierPersistAction::StoreSummary { store } => {
                if let Some(ready) = state.ready_mut() {
                    ready.store = (**store).clone();
                }
            }
            TransitionFrontierPersistAction::LoadTimeout => {
                if matches!(
                    state.restore(),
                    Some(PersistRestoreState::Cancelling { .. })
                ) {
                    // Cancellation already won the atomic handoff gate. The
                    // worker can finish later, but cannot install ledgers.
                    // Disable writes too: do not queue checkpoints behind a
                    // read that may never return.
                    let reason = "frontier store load cancellation did not drain within 30 seconds; bootstrapping with persistence disabled";
                    mina_core::warn!(meta.time(); kind = "PersistLoadAbandoned", summary = reason);
                    *state = TransitionFrontierPersistState::Disabled {
                        reason: reason.to_owned(),
                    };
                } else {
                    let dispatcher = state_context.into_dispatcher();
                    dispatcher.push(TransitionFrontierPersistEffectfulAction::LoadCancel);
                }
            }
            TransitionFrontierPersistAction::LoadCancelPending => {
                if let Some(ready) = state.ready_mut() {
                    ready.restore = PersistRestoreState::Cancelling { time: meta.time() };
                }
            }
            TransitionFrontierPersistAction::LoadCancelled => {
                restore_done(state, meta.time(), PersistRestoreOutcome::TimedOut);
            }
            TransitionFrontierPersistAction::LoadProgress { completed_stages } => {
                if let Some(PersistRestoreState::LoadPending {
                    completed_stages: current,
                    last_progress_time,
                    ..
                }) = state.ready_mut().map(|ready| &mut ready.restore)
                {
                    *current = *completed_stages;
                    *last_progress_time = Some(meta.time());
                }
            }
            TransitionFrontierPersistAction::ReplayDone => {
                let Some(ready) = state.ready_mut() else {
                    return;
                };
                let PersistRestoreState::Replaying { time, info } = &ready.restore else {
                    return;
                };
                let replay_ms = ms_since(meta.time(), *time);
                let info = info.clone();
                mina_core::info!(
                    meta.time();
                    kind = "PersistRestored",
                    summary = format!(
                        "restored from the frontier store: {} blocks replayed from {} to {}, the node follows the network from there",
                        info.blocks, info.root_height, info.tip_height
                    ),
                    tip = info.tip.to_string(),
                    generation = info.generation,
                    attempt = info.attempt,
                    load_ms = info.timings.total_ms,
                    replay_ms = replay_ms,
                    total_ms = ms_since(meta.time(), info.load_started),
                );
                // The stored checkpoint counts as this run's: the next one is
                // due an interval from now (I5c's note), or as soon as the
                // node is synced if the stored chain is more than a few
                // blocks longer than `k + 1` (I5d3). Otherwise a node
                // restarted more often than the interval never moves its
                // checkpoint, and each replay is longer than the last.
                ready.last_checkpoint = Some(PersistLastCheckpoint {
                    time: meta.time(),
                    root: info.root.clone(),
                    root_height: info.root_height,
                    generation: info.generation,
                });
                ready.checkpoint_chain_len_limit = best_chain.last().map(|tip| {
                    let k = tip.constants().k.as_u32();
                    k.saturating_add(1)
                        .saturating_add(RESTORE_CHECKPOINT_SLACK_BLOCKS)
                });
                ready.restore = PersistRestoreState::Done {
                    time: meta.time(),
                    outcome: PersistRestoreOutcome::Restored { info, replay_ms },
                };
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(TransitionFrontierPersistEffectfulAction::RestoreDone);
            }
            TransitionFrontierPersistAction::ReplayFailed { error } => {
                if let Some(ready) = state.ready_mut() {
                    // The store thread deletes it.
                    ready.store = PersistStoreSummary::default();
                }
                restore_done(
                    state,
                    meta.time(),
                    PersistRestoreOutcome::Failed {
                        error: format!("replay: {error}"),
                    },
                );
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(TransitionFrontierPersistEffectfulAction::Discard {
                    reason: format!("a restored block failed to apply: {error}"),
                });
            }
            TransitionFrontierPersistAction::ChainPutInit => {
                let (Some(ready), Some(tip)) = (state.ready_mut(), best_chain.last()) else {
                    return;
                };
                ready.chain_put_sent = Some(tip.hash().clone());
                ready.chain_puts_pending += 1;
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(TransitionFrontierPersistEffectfulAction::ChainPut);
                // The chain is handed over: a due checkpoint may start.
                dispatcher.push(TransitionFrontierPersistAction::CheckpointInit);
            }
            TransitionFrontierPersistAction::ChainPutSuccess { done } => {
                let Some(ready) = state.ready_mut() else {
                    return;
                };
                ready.chain_puts_pending = ready.chain_puts_pending.saturating_sub(1);
                ready.store = done.store.clone();
                ready.stats.chain_puts += 1;
                ready.stats.bytes_written += done.written.bytes + done.written.manifest_bytes;
                if done.checkpoint_invalidated {
                    // The chain no longer starts on the stored one: a
                    // bootstrap or a deep reorganisation. Checkpoint now.
                    ready.retry_after = None;
                    let dispatcher = state_context.into_dispatcher();
                    dispatcher.push(TransitionFrontierPersistAction::CheckpointInit);
                }
            }
            TransitionFrontierPersistAction::ChainPutError { error } => {
                if error.disable {
                    *state = Self::Disabled {
                        reason: format!("chain put: {}", error.error),
                    };
                    return;
                }
                let Some(ready) = state.ready_mut() else {
                    return;
                };
                ready.chain_puts_pending = ready.chain_puts_pending.saturating_sub(1);
                ready.stats.errors += 1;
                ready.stats.last_error = Some(format!("chain put: {}", error.error));
            }
            TransitionFrontierPersistAction::CheckpointInit => {
                let (Some(ready), Some(root), Some(tip)) =
                    (state.ready_mut(), best_chain.first(), best_chain.last())
                else {
                    return;
                };
                ready.checkpoint = PersistCheckpointState::Init {
                    time: meta.time(),
                    root: root.hash().clone(),
                };
                let stored = ready
                    .store
                    .ledgers
                    .iter()
                    .map(|ledger| (ledger.hash.clone(), ledger.base.clone()))
                    .collect();
                let checkpoint = Checkpoint {
                    root: root.hash().clone(),
                    root_height: root.height(),
                    root_snarked_ledger: root.snarked_ledger_hash().clone(),
                    root_staged_ledger_hash: root.staged_ledger_hashes().clone(),
                };
                let chain = best_chain.iter().map(chain_entry).collect();
                let ledgers = checkpoint_ledger_hashes(root, tip);
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let needed_protocol_states = global_state
                    .transition_frontier
                    .needed_protocol_states
                    .clone();
                dispatcher.push(LedgerWriteAction::Init {
                    request: LedgerWriteRequest::PersistCheckpoint(Box::new(
                        PersistCheckpointRequest {
                            checkpoint,
                            chain,
                            ledgers,
                            stored,
                            needed_protocol_states,
                        },
                    )),
                    on_init: redux::callback!(
                        on_persist_checkpoint_init(_request: LedgerWriteRequest) -> crate::Action {
                            TransitionFrontierPersistAction::CheckpointSnapshotSent
                        }
                    ),
                });
            }
            TransitionFrontierPersistAction::CheckpointSnapshotSent => {}
            TransitionFrontierPersistAction::CheckpointPending { root, snapshot } => {
                let Some(ready) = state.ready_mut() else {
                    return;
                };
                ready.checkpoint = PersistCheckpointState::Pending {
                    time: meta.time(),
                    root: root.clone(),
                    snapshot: snapshot.clone(),
                };
            }
            TransitionFrontierPersistAction::CheckpointSuccess { done } => {
                let Some(ready) = state.ready_mut() else {
                    return;
                };
                ready.checkpoint = PersistCheckpointState::Idle;
                ready.retry_after = None;
                ready.store = done.store.clone();
                ready.last_checkpoint = Some(PersistLastCheckpoint {
                    time: meta.time(),
                    root: done.root.clone(),
                    root_height: done.root_height,
                    generation: done.generation,
                });
                ready.checkpoint_chain_len_limit = None;
                ready.stats.checkpoints += 1;
                ready.stats.bytes_written += done.written.bytes + done.written.manifest_bytes;
            }
            TransitionFrontierPersistAction::CheckpointError { error } => {
                if error.disable {
                    *state = Self::Disabled {
                        reason: format!("checkpoint: {}", error.error),
                    };
                    return;
                }
                let Some(ready) = state.ready_mut() else {
                    return;
                };
                ready.checkpoint = PersistCheckpointState::Idle;
                ready.retry_after = Some(meta.time() + Duration::from_secs(CHECKPOINT_RETRY_SECS));
                ready.stats.errors += 1;
                ready.stats.last_error = Some(format!("checkpoint: {}", error.error));
            }
        }
    }
}

/// Ends the restore: the node bootstraps (or, for `Restored`, follows).
fn restore_done(
    state: &mut TransitionFrontierPersistState,
    time: redux::Timestamp,
    outcome: PersistRestoreOutcome,
) {
    if let Some(ready) = state.ready_mut() {
        ready.restore = PersistRestoreState::Done { time, outcome };
    }
}

/// A block of the best chain, as the store records it.
pub fn chain_entry(block: &AppliedBlock) -> ChainEntry {
    ChainEntry {
        hash: block.hash().clone(),
        height: block.height(),
        global_slot: block.global_slot(),
        just_emitted_a_proof: block.just_emitted_a_proof,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mina_curves::pasta::Fp;
    use mina_p2p_messages::v2::LedgerHash;

    use super::*;

    #[test]
    fn checkpoint_keeps_old_staking_until_root_crosses() {
        let mut blocks = crate::persist::test_fixtures::mainnet_blocks();
        let mut root = blocks.remove(0);
        let mut tip = blocks.remove(0);
        let hash = |n| LedgerHash::from_fp(Fp::from(n));
        let root_state = &mut Arc::make_mut(&mut root.block).header.protocol_state.body;
        root_state
            .blockchain_state
            .ledger_proof_statement
            .target
            .first_pass_ledger = hash(1);
        root_state.consensus_state.staking_epoch_data.ledger.hash = hash(2);
        root_state.consensus_state.next_epoch_data.ledger.hash = hash(3);
        let tip_state = &mut Arc::make_mut(&mut tip.block).header.protocol_state.body;
        tip_state.consensus_state.staking_epoch_data.ledger.hash = hash(3);
        tip_state.consensus_state.next_epoch_data.ledger.hash = hash(4);
        let applied = |block| AppliedBlock {
            block,
            just_emitted_a_proof: false,
        };
        assert_eq!(
            checkpoint_ledger_hashes(&applied(root), &applied(tip)),
            [1, 3, 4, 2].map(hash)
        );
    }
}
