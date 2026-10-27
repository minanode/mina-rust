use crate::{Service, Store};
use redux::ActionMeta;

use super::BlockProducerVrfEvaluatorEffectfulAction;

impl BlockProducerVrfEvaluatorEffectfulAction {
    pub fn effects<S: Service>(self, _: &ActionMeta, store: &mut Store<S>) {
        match self {
            BlockProducerVrfEvaluatorEffectfulAction::EvaluateSlot { vrf_input } => {
                store.service.evaluate(vrf_input);
            }
            BlockProducerVrfEvaluatorEffectfulAction::SlotEvaluated { epoch, won } => {
                if let Some(stats) = store.service.stats() {
                    stats.block_producer().increment_slot_evaluated(epoch, won);
                }
            }
            BlockProducerVrfEvaluatorEffectfulAction::InitializeStats {
                epoch,
                initial_slot,
                delegated_stake,
                total_currency,
                delegators,
            } => {
                if let Some(stats) = store.service.stats() {
                    let slots_per_epoch =
                        store.state.get().config.consensus_constants.slots_per_epoch;
                    // We add 1 because the evaluation starts from the next slot, except for the first slot 0
                    let initial_slot = if initial_slot == 0 {
                        0
                    } else {
                        initial_slot.saturating_add(1)
                    };
                    let remaining_slots = slots_per_epoch.saturating_sub(initial_slot);
                    stats.block_producer().new_epoch_evaluation(
                        epoch,
                        remaining_slots,
                        crate::stats::block_producer::EpochStake {
                            delegated_stake,
                            total_currency,
                            delegators,
                        },
                    );
                }
                // The epoch's stake, kept for a restart (F-producer).
                store.service.save_block_production();
            }
        }
    }
}
