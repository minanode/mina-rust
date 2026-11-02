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
            BlockProducerVrfEvaluatorEffectfulAction::InitializeStats { .. } => {
                if let Some(stats) = store.service.stats() {
                    let slots_per_epoch =
                        store.state.get().config.consensus_constants.slots_per_epoch;
                    self.record_epoch_stats(stats.block_producer(), slots_per_epoch);
                }
                // The epoch's stake, kept for a restart (F-producer).
                store.service.save_block_production();
            }
        }
    }

    /// Apply the epoch initialization effect to the statistics store. Keeping
    /// this separate from the node service lets reducer tests consume the
    /// actual InitializeStats action without starting ledger or VRF workers.
    pub(crate) fn record_epoch_stats(
        self,
        stats: &mut crate::stats::block_producer::BlockProducerStats,
        slots_per_epoch: u32,
    ) {
        if let Self::InitializeStats {
            epoch,
            initial_slot,
            delegated_stake,
            total_currency,
            delegators,
        } = self
        {
            stats.new_epoch_evaluation(
                epoch,
                slots_per_epoch.saturating_sub(initial_slot),
                crate::stats::block_producer::EpochStake {
                    delegated_stake,
                    total_currency,
                    delegators,
                },
            );
        }
    }
}
