use redux::ActionMeta;

use super::{TransitionFrontierPersistEffectfulAction, TransitionFrontierPersistService};
use crate::{
    persist::event::PersistWriteError,
    transition_frontier::persist::{chain_entry, TransitionFrontierPersistAction},
    Store,
};

impl TransitionFrontierPersistEffectfulAction {
    pub fn effects<S>(&self, _: &ActionMeta, store: &mut Store<S>)
    where
        S: redux::Service + TransitionFrontierPersistService,
    {
        match self {
            TransitionFrontierPersistEffectfulAction::Open {
                chain_id,
                genesis_state_hash,
            } => {
                if !store
                    .service
                    .persist_open(chain_id.clone(), genesis_state_hash.clone())
                {
                    store.dispatch(TransitionFrontierPersistAction::OpenError {
                        error: "no frontier store thread".to_owned(),
                    });
                }
            }
            TransitionFrontierPersistEffectfulAction::ChainPut => {
                // Cheap on the node thread: the entries, and a reference to
                // each block; the store thread encodes the blocks it lacks.
                let best_chain = &store.state().transition_frontier.best_chain;
                let chain = best_chain.iter().map(chain_entry).collect();
                let blocks = best_chain
                    .iter()
                    .map(|block| block.block_with_hash().clone())
                    .collect();
                if !store.service.persist_chain_put(chain, blocks) {
                    store.dispatch(TransitionFrontierPersistAction::ChainPutError {
                        error: PersistWriteError {
                            error: "no frontier store thread".to_owned(),
                            disable: true,
                        },
                    });
                }
            }
            TransitionFrontierPersistEffectfulAction::Load { request } => {
                // The node's own blockchain verifier, unless it runs with
                // proof verification off.
                let verifier = (!store.state().config.skip_proof_verification).then(|| {
                    let block_verify = &store.state().snark.block_verify;
                    crate::persist::load::BlockProofVerifier {
                        index: block_verify.verifier_index.clone(),
                        srs: block_verify.verifier_srs.clone(),
                    }
                });
                if !store.service.persist_load(request.clone(), verifier) {
                    store.dispatch(TransitionFrontierPersistAction::LoadError {
                        error: "no frontier store thread".to_owned(),
                    });
                }
            }
            TransitionFrontierPersistEffectfulAction::LoadCancel => {
                if store.service.persist_load_cancel() {
                    store.dispatch(TransitionFrontierPersistAction::LoadCancelPending);
                }
            }
            TransitionFrontierPersistEffectfulAction::RestoreDone => {
                store.service.persist_restore_done();
            }
            TransitionFrontierPersistEffectfulAction::Discard { reason } => {
                store.service.persist_discard(reason.clone());
            }
        }
    }
}
