use crate::{
    block_producer::{block_prover_input, BlockProducerCurrentState},
    ledger::write::{LedgerWriteAction, LedgerWriteRequest},
    BlockProducerAction, Store,
};
use redux::ActionWithMeta;

use super::BlockProducerEffectfulAction;

pub fn block_producer_effects<S: crate::Service>(
    store: &mut Store<S>,
    action: ActionWithMeta<BlockProducerEffectfulAction>,
) {
    let (action, meta) = action.split();

    match action {
        BlockProducerEffectfulAction::VrfEvaluator(a) => {
            a.effects(&meta, store);
        }
        BlockProducerEffectfulAction::WonSlot { won_slot } => {
            if let Some(stats) = store.service.stats() {
                stats.block_producer().scheduled(meta.time(), &won_slot);
            }
            if !store.dispatch(BlockProducerAction::WonSlotWait) {
                store.dispatch(BlockProducerAction::WonSlotProduceInit);
            }
            // Each status change is kept for a restart (F-producer). The
            // service hands the history to a thread of its own: no file I/O
            // here.
            store.service.save_block_production();
        }
        BlockProducerEffectfulAction::StagedLedgerDiffCreateInit => {
            if let Some(stats) = store.service.stats() {
                stats
                    .block_producer()
                    .staged_ledger_diff_create_start(meta.time());
            }
            let state = store.state.get();
            let Some((won_slot, pred_block, producer, coinbase_receiver)) = None.or_else(|| {
                let pred_block = state.block_producer.current_parent_chain()?.last()?;
                let won_slot = state.block_producer.current_won_slot()?;
                let config = state.block_producer.config()?;
                Some((
                    won_slot,
                    pred_block,
                    &config.pub_key,
                    config.coinbase_receiver(),
                ))
            }) else {
                return;
            };

            let completed_snarks = state
                .snark_pool
                .completed_snarks_iter()
                .map(|snark| (snark.job_id(), snark.clone()))
                .collect();
            // TODO(binier)
            let supercharge_coinbase = true;
            // We want to know if this is a new epoch to decide which staking ledger to use
            // (staking epoch ledger or next epoch ledger).
            let is_new_epoch = won_slot.epoch()
                > pred_block
                    .header()
                    .protocol_state
                    .body
                    .consensus_state
                    .epoch_count
                    .as_u32();

            let transactions_by_fee = state.block_producer.pending_transactions();

            store.dispatch(LedgerWriteAction::Init {
                request: LedgerWriteRequest::StagedLedgerDiffCreate {
                    pred_block: pred_block.clone(),
                    global_slot_since_genesis: won_slot
                        .global_slot_since_genesis(pred_block.global_slot_diff()),
                    is_new_epoch,
                    producer: producer.clone(),
                    delegator: won_slot.delegator.0.clone(),
                    coinbase_receiver: coinbase_receiver.clone(),
                    completed_snarks,
                    supercharge_coinbase,
                    transactions_by_fee,
                },
                on_init: redux::callback!(
                    on_staged_ledger_diff_create_init(_request: LedgerWriteRequest) -> crate::Action {
                        BlockProducerAction::StagedLedgerDiffCreatePending
                    }
                ),
            });
        }
        BlockProducerEffectfulAction::StagedLedgerDiffCreateSuccess => {
            if let Some(stats) = store.service.stats() {
                stats
                    .block_producer()
                    .staged_ledger_diff_create_end(meta.time());
            }
            store.dispatch(BlockProducerAction::BlockUnprovenBuild);
        }
        BlockProducerEffectfulAction::BlockUnprovenBuild => {
            if let Some(stats) = store.service.stats() {
                let bp = &store.state.get().block_producer;
                if let Some((block_hash, block, just_emitted_ledger_proof)) =
                    bp.with(None, |bp| match &bp.current {
                        BlockProducerCurrentState::BlockUnprovenBuilt {
                            block,
                            block_hash,
                            emitted_ledger_proof,
                            ..
                        } => Some((block_hash, block, emitted_ledger_proof.is_some())),
                        _ => None,
                    })
                {
                    let ps = &block.protocol_state;
                    let cs = &ps.body.consensus_state;
                    let previous_state_hash = ps.previous_state_hash.to_string();
                    let state_hash = block_hash.to_string();
                    let global_slot_since_genesis = cs.global_slot_since_genesis.as_u32();
                    let height = cs.blockchain_length.as_u32();
                    mina_core::info!(
                        meta.time();
                        message = "Unproven block built",
                        state_hash = state_hash,
                        previous_state_hash = previous_state_hash,
                        global_slot_since_genesis = global_slot_since_genesis,
                        height = height,
                        just_emitted_ledger_proof = just_emitted_ledger_proof,
                    );

                    stats
                        .block_producer()
                        .produced(meta.time(), block_hash, block);
                }
            }

            // Our block's consensus state exists now: it is proved only if
            // it beats the best tip, which may have changed meanwhile.
            let state = store.state.get();
            let discard = state.transition_frontier.best_tip().and_then(|best_tip| {
                state.block_producer.with(None, |bp| {
                    bp.current.won_slot_should_discard(best_tip, meta.time())
                })
            });
            match discard {
                Some(reason) => store.dispatch(BlockProducerAction::WonSlotDiscard { reason }),
                None => store.dispatch(BlockProducerAction::BlockProveInit),
            };
        }
        BlockProducerEffectfulAction::BlockProveInit => {
            let service = &mut store.service;

            if let Some(stats) = service.stats() {
                stats.block_producer().proof_create_start(meta.time());
            }
            let Some((block_hash, input, deadline)) =
                store.state.get().block_producer.with(None, |bp| {
                    let BlockProducerCurrentState::BlockUnprovenBuilt {
                        won_slot,
                        chain,
                        emitted_ledger_proof,
                        pending_coinbase_update,
                        pending_coinbase_witness,
                        stake_proof_sparse_ledger,
                        block,
                        block_hash,
                        ..
                    } = &bp.current
                    else {
                        return None;
                    };

                    let input = block_prover_input(
                        won_slot,
                        chain,
                        emitted_ledger_proof.clone(),
                        pending_coinbase_update,
                        pending_coinbase_witness,
                        stake_proof_sparse_ledger,
                        block,
                    )?;
                    Some((
                        block_hash.clone(),
                        Box::new(input),
                        won_slot.next_slot_time(),
                    ))
                })
            else {
                return;
            };
            service.prove(block_hash, input, deadline);
            store.dispatch(BlockProducerAction::BlockProvePending);
            // Built and proving, once the proof is asked for.
            store.service.save_block_production();
        }
        BlockProducerEffectfulAction::BlockProveSuccess => {
            if let Some(stats) = store.service.stats() {
                stats.block_producer().proof_create_end(meta.time());
            }
            store.dispatch(BlockProducerAction::BlockProduced);
            store.service.save_block_production();
        }
        BlockProducerEffectfulAction::WonSlotDiscard { reason } => {
            if let Some(stats) = store.service.stats() {
                stats.block_producer().discarded(meta.time(), reason);
            }
            store.dispatch(BlockProducerAction::WonSlotSearch);
            store.service.save_block_production();
        }
        BlockProducerEffectfulAction::BlockProduced { block } => {
            if let Some(stats) = store.service.stats() {
                stats.block_producer().last_produced_block = Some(block.clone());
            }
        }
    }
}
