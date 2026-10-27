use redux::ActionMeta;

use crate::Store;

use super::{TransitionFrontierGenesisEffectfulAction, TransitionFrontierGenesisService};

impl TransitionFrontierGenesisEffectfulAction {
    pub fn effects<S>(&self, _: &ActionMeta, store: &mut Store<S>)
    where
        S: redux::Service + TransitionFrontierGenesisService,
    {
        match self {
            TransitionFrontierGenesisEffectfulAction::LedgerLoadInit { config } => {
                store.service.load_genesis(config.clone());
            }
            TransitionFrontierGenesisEffectfulAction::LedgersLoadInit { config, expected } => {
                store
                    .service
                    .load_genesis_ledgers(config.clone(), expected.clone());
            }
            TransitionFrontierGenesisEffectfulAction::ProveInit {
                block_hash,
                input,
                deadline,
            } => {
                store
                    .service
                    .prove(block_hash.clone(), input.clone(), *deadline);
            }
        }
    }
}
