use super::{
    genesis_effectful::TransitionFrontierGenesisEffectfulAction,
    sync::{SyncError, TransitionFrontierSyncState},
    GenesisLedgersState, TransitionFrontierAction, TransitionFrontierActionWithMetaRef,
    TransitionFrontierState,
};
use mina_core::block::AppliedBlock;

impl TransitionFrontierState {
    pub fn reducer(
        mut state_context: crate::Substate<Self>,
        action: TransitionFrontierActionWithMetaRef<'_>,
        skip_proof_verification: bool,
    ) {
        let Ok(state) = state_context.get_substate_mut() else {
            // TODO: log or propagate
            return;
        };
        let (action, meta) = action.split();

        // Drop the diff, it's been processed in the effect
        state.chain_diff.take();

        match action {
            TransitionFrontierAction::VerificationRecorded { .. } => {}
            TransitionFrontierAction::Genesis(a) => {
                super::genesis::TransitionFrontierGenesisState::reducer(
                    mina_core::Substate::from_compatible_substate(state_context),
                    meta.with_action(a),
                    skip_proof_verification,
                )
            }
            TransitionFrontierAction::GenesisEffect(_) => {}
            TransitionFrontierAction::GenesisInject => {
                let Some(genesis) = state.genesis.block_with_real_or_dummy_proof() else {
                    return;
                };
                let genesis = AppliedBlock {
                    block: genesis,
                    just_emitted_a_proof: true,
                };
                state.best_chain = vec![genesis];
                state.sync = TransitionFrontierSyncState::Synced { time: meta.time() };
            }
            TransitionFrontierAction::GenesisProvenInject => {
                let Some(genesis) = state.genesis.proven_block() else {
                    return;
                };
                if let Some(block) = state.best_chain.get_mut(0) {
                    block.block = genesis.clone();
                } else {
                    let genesis = AppliedBlock {
                        block: genesis.clone(),
                        just_emitted_a_proof: true,
                    };
                    state.best_chain = vec![genesis];
                }
                if !state.sync.is_pending() {
                    state.sync = TransitionFrontierSyncState::Synced { time: meta.time() };
                }
            }
            TransitionFrontierAction::Candidate(a) => {
                super::candidate::TransitionFrontierCandidatesState::reducer(
                    mina_core::Substate::from_compatible_substate(state_context),
                    meta.with_action(a),
                );
            }
            TransitionFrontierAction::Sync(a) => {
                use super::sync::{PeerBlockFetchError, TransitionFrontierSyncAction as Sync};
                match a {
                    Sync::BlocksPeerQueryError {
                        peer_id,
                        rpc_id,
                        error,
                    } => {
                        // A local proof read failure is not the peer's: up
                        // to a bound, it doesn't back the peer off (BP-SAFE).
                        let hash = matches!(error, PeerBlockFetchError::LocalProofUnavailable(_))
                            .then(|| {
                                state
                                    .sync
                                    .blocks_iter()
                                    .find(|b| b.is_fetch_pending_from_peer(peer_id, *rpc_id))
                                    .map(|b| b.block_hash().clone())
                            })
                            .flatten();
                        let spared = hash.is_some_and(|hash| state.block_local_check_failed(&hash));
                        if !spared {
                            state.block_peer_query_failed(*peer_id, meta.time());
                        }
                    }
                    Sync::BlocksPeerQuerySuccess { response, .. } => {
                        state.block_local_check_errors.remove(&response.hash);
                    }
                    _ => {}
                }
                let best_chain = state.best_chain.clone();
                super::sync::TransitionFrontierSyncState::reducer(
                    mina_core::Substate::from_compatible_substate(state_context),
                    meta.with_action(a),
                    &best_chain,
                );
            }
            TransitionFrontierAction::Persist(a) => {
                let best_chain = state.best_chain.clone();
                let config = state.config.persist.clone();
                super::persist::TransitionFrontierPersistState::reducer(
                    mina_core::Substate::from_compatible_substate(state_context),
                    meta.with_action(a),
                    &best_chain,
                    &config,
                );
            }
            TransitionFrontierAction::PersistEffect(_) => {}
            TransitionFrontierAction::Synced {
                needed_protocol_states: needed_protocol_state_hashes,
            } => {
                let TransitionFrontierSyncState::CommitSuccess {
                    chain,
                    needed_protocol_states,
                    ..
                } = &mut state.sync
                else {
                    return;
                };
                let mut needed_protocol_state_hashes = needed_protocol_state_hashes.clone();
                let new_chain = std::mem::take(chain);
                let needed_protocol_states = std::mem::take(needed_protocol_states);

                state.needed_protocol_states.extend(needed_protocol_states);
                state
                    .needed_protocol_states
                    .retain(|k, _| needed_protocol_state_hashes.remove(k));

                for hash in needed_protocol_state_hashes {
                    let block = state
                        .best_chain
                        .iter()
                        .find(|b| b.hash() == &hash)
                        .or_else(|| new_chain.iter().find(|b| b.hash() == &hash));
                    // TODO(binier): error log instead.
                    let block = block.expect("we lack needed block!");
                    let protocol_state = block.header().protocol_state.clone();
                    state.needed_protocol_states.insert(hash, protocol_state);
                }

                state.blacklist.retain(|_, height| {
                    // prune blocks from black list that can't end up
                    // into transition frontier anymore due to consensus
                    // reasons.
                    let tip = new_chain.last().unwrap();
                    height
                        .checked_add(tip.constants().k.as_u32())
                        .expect("overflow")
                        > tip.height()
                });
                state.chain_diff = state.maybe_make_chain_diff(&new_chain);
                if let Some(root) = new_chain.first() {
                    if let Err(error) = root.cache_proofs() {
                        mina_core::log::inner::warn!("root proof cache: {error}");
                    }
                    state.root_history.advance(&state.best_chain, &root.block);
                }
                state.best_chain = new_chain;
                state.sync = TransitionFrontierSyncState::Synced { time: meta.time() };
            }
            TransitionFrontierAction::SyncFailed { error, .. } => {
                match error {
                    SyncError::BlockApplyFailed(block, _) => {
                        state.blacklist.insert(block.hash().clone(), block.height());
                    }
                    // Our store's fault, not the block's (R20 D5).
                    SyncError::RestoredBlockApplyFailed(..) => {}
                }
                state.sync = TransitionFrontierSyncState::Synced { time: meta.time() };
            }
            TransitionFrontierAction::GenesisLedgersLoadInit => {
                let Some(config) = state.config.genesis_ledgers_deferred.clone() else {
                    return;
                };
                let expected = state.config.genesis.clone();
                state.genesis_ledgers = GenesisLedgersState::Pending { time: meta.time() };
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(TransitionFrontierGenesisEffectfulAction::LedgersLoadInit {
                    config,
                    expected,
                });
            }
            TransitionFrontierAction::GenesisLedgersLoadSuccess => {
                state.genesis_ledgers = GenesisLedgersState::Loaded;
            }
            TransitionFrontierAction::GenesisLedgersLoadError { error } => {
                state.genesis_ledgers = GenesisLedgersState::Error {
                    error: error.clone(),
                };
            }
        }
    }
}
