use crate::p2p::{
    channels::rpc::{P2pChannelsRpcAction, P2pRpcId},
    P2pNetworkPubsubAction, PeerId,
};
use mina_core::block::{AppliedBlock, ArcBlockWithHash};
use mina_p2p_messages::v2::LedgerHash;
use redux::ActionMeta;

use crate::{
    ledger::write::{LedgerWriteAction, LedgerWriteRequest, LedgersToKeep},
    p2p::channels::rpc::P2pRpcRequest,
    p2p_ready,
    service::TransitionFrontierSyncLedgerSnarkedService,
    transition_frontier::persist::TransitionFrontierPersistAction,
    Service, Store, TransitionFrontierAction,
};

use super::{
    ledger::{
        snarked::TransitionFrontierSyncLedgerSnarkedAction,
        staged::TransitionFrontierSyncLedgerStagedAction, try_complete_snarked_ledger_locally,
        SyncLedgerTarget, TransitionFrontierSyncLedgerAction,
    },
    SyncError, TransitionFrontierSyncAction, TransitionFrontierSyncState,
};

impl TransitionFrontierSyncAction {
    pub fn effects<S>(&self, meta: &ActionMeta, store: &mut Store<S>)
    where
        S: Service,
    {
        match self {
            TransitionFrontierSyncAction::Init { best_tip, .. } => {
                let protocol_state_body = &best_tip.block.header.protocol_state.body;
                let genesis_ledger_hash = &protocol_state_body.blockchain_state.genesis_ledger_hash;
                let staking_epoch_ledger_hash = &protocol_state_body
                    .consensus_state
                    .staking_epoch_data
                    .ledger
                    .hash;
                let next_epoch_ledger_hash = &protocol_state_body
                    .consensus_state
                    .next_epoch_data
                    .ledger
                    .hash;

                // TODO(tizoc): if root ledger matches genesis, should anything special be done?
                // snarked ledger will not need to be synced but staged ledger parts are still
                // required

                if genesis_ledger_hash != staking_epoch_ledger_hash {
                    store.dispatch(TransitionFrontierSyncAction::LedgerStakingPending);
                } else if genesis_ledger_hash != next_epoch_ledger_hash {
                    store.dispatch(TransitionFrontierSyncAction::LedgerNextEpochPending);
                } else {
                    store.dispatch(TransitionFrontierSyncAction::LedgerRootPending);
                }
            }
            TransitionFrontierSyncAction::BestTipUpdate {
                previous_root_snarked_ledger_hash,
                best_tip,
                on_success,
                ..
            } => {
                // TODO(tizoc): this is currently required because how how complicated the BestTipUpdate reducer is,
                // once that is simplified this should be handled in separate actions.
                let prepared_copy = maybe_copy_ledgers_for_sync(
                    store,
                    previous_root_snarked_ledger_hash.clone(),
                    best_tip,
                )
                .unwrap();

                // BestTipUpdate can enter NumAccountsPending directly. When
                // the exact ledger is committed locally, finish without
                // waiting for an eligible peer.
                try_complete_snarked_ledger_locally(store, prepared_copy);

                // if root snarked ledger changed.
                store.dispatch(TransitionFrontierSyncLedgerAction::Init);
                // if root snarked ledger stayed same but root block changed
                // while reconstructing staged ledger.
                store.dispatch(TransitionFrontierSyncLedgerStagedAction::PartsFetchPending);
                store.dispatch(TransitionFrontierSyncLedgerSnarkedAction::PeersQuery);
                // if we don't need to sync root staged ledger.
                store.dispatch(TransitionFrontierSyncAction::BlocksPeersQuery);
                // if we already have a block ready to be applied.
                store.dispatch(TransitionFrontierSyncAction::BlocksNextApplyInit);

                // TODO(binier): cleanup ledgers
                if let Some(callback) = on_success {
                    store.dispatch_callback(callback.clone(), ());
                }
            }
            // TODO(tizoc): this action is never called with the current implementation,
            // either remove it or figure out how to recover it as a reaction to
            // `BestTipUpdate` above. Currently this logic is handled by
            // `maybe_copy_ledgers_for_sync` at the end of this file.
            // Same kind of applies to `LedgerNextEpochPending` and `LedgerRootPending`
            // in some cases, but issue is mostly about `LedgerStakingPending` because
            // it is the one most likely to be affected by the first `BestTipUpdate`
            // action processed by the state machine.
            TransitionFrontierSyncAction::LedgerStakingPending => {
                let prepared_copy =
                    prepare_staking_epoch_ledger_for_sync(store, &sync_best_tip(store.state()))
                        .unwrap();
                if !try_complete_snarked_ledger_locally(store, prepared_copy) {
                    store.dispatch(TransitionFrontierSyncLedgerAction::Init);
                }
            }
            TransitionFrontierSyncAction::LedgerStakingSuccess => {
                if store.dispatch(TransitionFrontierSyncAction::LedgerNextEpochPending) {
                } else if store.dispatch(TransitionFrontierSyncAction::LedgerRootPending) {
                }
            }
            TransitionFrontierSyncAction::LedgerNextEpochPending => {
                let prepared_copy =
                    prepare_next_epoch_ledger_for_sync(store, &sync_best_tip(store.state()))
                        .unwrap();
                if !try_complete_snarked_ledger_locally(store, prepared_copy) {
                    store.dispatch(TransitionFrontierSyncLedgerAction::Init);
                }
            }
            TransitionFrontierSyncAction::LedgerNextEpochSuccess => {
                store.dispatch(TransitionFrontierSyncAction::LedgerRootPending);
            }
            TransitionFrontierSyncAction::LedgerRootPending => {
                let prepared_copy = prepare_transition_frontier_root_ledger_for_sync(
                    store,
                    None,
                    &sync_best_tip(store.state()),
                )
                .unwrap();
                if !try_complete_snarked_ledger_locally(store, prepared_copy) {
                    store.dispatch(TransitionFrontierSyncLedgerAction::Init);
                }
            }
            TransitionFrontierSyncAction::LedgerRootSuccess => {
                store.dispatch(TransitionFrontierSyncAction::BlocksPending);
            }
            TransitionFrontierSyncAction::BlocksPending => {
                if !store.dispatch(TransitionFrontierSyncAction::BlocksSuccess) {
                    store.dispatch(TransitionFrontierSyncAction::BlocksPeersQuery);
                }
            }
            TransitionFrontierSyncAction::BlocksPeersQuery => {
                let p2p = p2p_ready!(store.state().p2p, meta.time());
                // Cached tips can still be genesis after a peer has synced.
                // Query by hash, validate the answer, and back off on failure;
                // an old hint must not exclude a healthy serving sibling.
                let mut peer_ids = p2p
                    .ready_peers_iter()
                    .filter(|(id, p)| {
                        p.channels.rpc.can_send_request()
                            && store
                                .state()
                                .transition_frontier
                                .block_peer_query_ready(id, meta.time())
                    })
                    .map(|(id, p)| {
                        (
                            *id,
                            p.connected_since,
                            store
                                .state()
                                .transition_frontier
                                .block_peer_query_errors
                                .get(id)
                                .copied(),
                        )
                    })
                    .collect::<Vec<_>>();
                // Unfailed peers first, then the least recently failed. Keep
                // the existing connection preference only as a tie breaker.
                peer_ids.sort_by(|(_, t1, e1), (_, t2, e2)| e1.cmp(e2).then_with(|| t2.cmp(t1)));

                let mut retry_hashes = store
                    .state()
                    .transition_frontier
                    .sync
                    .blocks_fetch_retry_iter(meta.time())
                    .collect::<Vec<_>>();
                retry_hashes.reverse();

                for (peer_id, _, _) in peer_ids {
                    if let Some(hash) = retry_hashes.last() {
                        if store.dispatch(TransitionFrontierSyncAction::BlocksPeerQueryRetry {
                            peer_id,
                            hash: hash.clone(),
                        }) {
                            retry_hashes.pop();
                            continue;
                        }
                    }

                    match store.state().transition_frontier.sync.blocks_fetch_next() {
                        Some(hash) => {
                            store.dispatch(TransitionFrontierSyncAction::BlocksPeerQueryInit {
                                peer_id,
                                hash,
                            });
                        }
                        None if retry_hashes.is_empty() => break,
                        None => {}
                    }
                }
            }
            TransitionFrontierSyncAction::BlocksPeerQueryInit { hash, peer_id } => {
                let p2p = p2p_ready!(store.state().p2p, meta.time());
                let Some(rpc_id) = p2p
                    .get_ready_peer(peer_id)
                    .map(|v| v.channels.next_local_rpc_id())
                else {
                    return;
                };

                store.dispatch(P2pChannelsRpcAction::RequestSend {
                    peer_id: *peer_id,
                    id: rpc_id,
                    request: Box::new(P2pRpcRequest::Block(hash.clone())),
                    on_init: Some(redux::callback!(
                        on_send_p2p_block_rpc_request(
                            (peer_id: PeerId, rpc_id: P2pRpcId, request: P2pRpcRequest)
                        ) -> crate::Action {
                            let P2pRpcRequest::Block(hash) = request else {
                                unreachable!()
                            };
                            TransitionFrontierSyncAction::BlocksPeerQueryPending {
                                hash,
                                peer_id,
                                rpc_id,
                            }
                        }
                    )),
                });
            }
            TransitionFrontierSyncAction::BlocksPeerQueryRetry { hash, peer_id } => {
                let p2p = p2p_ready!(store.state().p2p, meta.time());
                let Some(rpc_id) = p2p
                    .get_ready_peer(peer_id)
                    .map(|v| v.channels.next_local_rpc_id())
                else {
                    return;
                };

                store.dispatch(P2pChannelsRpcAction::RequestSend {
                    peer_id: *peer_id,
                    id: rpc_id,
                    request: Box::new(P2pRpcRequest::Block(hash.clone())),
                    on_init: Some(redux::callback!(
                        on_send_p2p_block_rpc_request_retry(
                            (peer_id: PeerId, rpc_id: P2pRpcId, request: P2pRpcRequest)
                        ) -> crate::Action {
                            let P2pRpcRequest::Block(hash) = request else {
                                unreachable!()
                            };
                            TransitionFrontierSyncAction::BlocksPeerQueryPending {
                                hash,
                                peer_id,
                                rpc_id,
                            }
                        }
                    )),
                });
            }
            TransitionFrontierSyncAction::BlocksPeerQueryPending { .. } => {}
            TransitionFrontierSyncAction::BlocksPeerQueryError { .. } => {
                store.dispatch(TransitionFrontierSyncAction::BlocksPeersQuery);
            }
            TransitionFrontierSyncAction::BlocksPeerQuerySuccess { response, .. } => {
                store.dispatch(TransitionFrontierSyncAction::BlocksPeersQuery);
                store.dispatch(TransitionFrontierSyncAction::BlocksFetchSuccess {
                    hash: response.hash.clone(),
                });
            }
            TransitionFrontierSyncAction::BlocksFetchSuccess { .. } => {
                let _ = store;
                store.dispatch(TransitionFrontierSyncAction::BlocksNextApplyInit {});
            }
            TransitionFrontierSyncAction::BlocksNextApplyInit => {
                let Some((block, pred_block)) = store
                    .state()
                    .transition_frontier
                    .sync
                    .blocks_apply_next()
                    .map(|v| (v.0.clone(), v.1.clone()))
                else {
                    return;
                };
                let hash = block.hash.clone();

                let is_our_block;

                if let Some(stats) = store.service.stats() {
                    stats.block_producer().block_apply_start(meta.time(), &hash);
                    // TODO(tizoc): try a better approach that doesn't need
                    // to make use of the collected stats.
                    is_our_block = stats.block_producer().is_our_just_produced_block(&hash);
                } else {
                    is_our_block = false;
                }

                // During catchup, we skip the verificationf of completed work and zkApp txn proofs
                // until get closer to the best tip, at which point full verification is enabled.
                // We also skip verification of completed works if we produced this block.
                // A restore's replay (I5d1) verifies as catch-up does: the
                // staged ledger hash of every block, and the proofs of the
                // last few.
                let skip_verification = is_our_block
                    || super::CATCHUP_BLOCK_VERIFY_TAIL_LENGTH
                        < store.state().transition_frontier.sync.pending_count();
                let restored = store.state().transition_frontier.persist.is_replaying();

                store.dispatch(LedgerWriteAction::Init {
                    request: LedgerWriteRequest::BlockApply {
                        block,
                        pred_block,
                        skip_verification,
                        restored,
                    },
                    on_init: redux::callback!(
                        on_block_next_apply_init(request: LedgerWriteRequest) -> crate::Action {
                            let LedgerWriteRequest::BlockApply {
                                block,
                                ..
                            } = request
                            else {
                                unreachable!()
                            };
                            let hash = block.hash().clone();
                            TransitionFrontierSyncAction::BlocksNextApplyPending { hash }
                        }
                    ),
                });
                // Our block injected: kept for a restart (F-producer), on
                // the service's own thread.
                if is_our_block {
                    store.service.save_block_production();
                }
            }
            TransitionFrontierSyncAction::BlocksNextApplyPending { .. } => {}
            TransitionFrontierSyncAction::BlocksNextApplyError { hash, error } => {
                let Some((best_tip, failed_block)) = None.or_else(|| {
                    Some((
                        store.state().transition_frontier.sync.best_tip()?.clone(),
                        store
                            .state()
                            .transition_frontier
                            .sync
                            .block_state(hash)?
                            .block()?,
                    ))
                }) else {
                    return;
                };
                let failed_block = failed_block.clone();
                if store.state().transition_frontier.persist.is_replaying() {
                    // A block of our own store: not blacklisted, nor
                    // rejected as gossip. The store is dropped and the node
                    // bootstraps (R20 D5).
                    let reason = format!(
                        "block {} at {}: {error}",
                        failed_block.hash(),
                        failed_block.height()
                    );
                    store.dispatch(TransitionFrontierAction::SyncFailed {
                        best_tip,
                        error: SyncError::RestoredBlockApplyFailed(failed_block, error.clone()),
                    });
                    store.dispatch(TransitionFrontierPersistAction::ReplayFailed { error: reason });
                    return;
                }
                let error = SyncError::BlockApplyFailed(failed_block, error.clone());
                store.dispatch(TransitionFrontierAction::SyncFailed { best_tip, error });
                // TODO this should be handled by a callback
                store.dispatch(P2pNetworkPubsubAction::RejectMessage {
                    message_id: Some(crate::p2p::BroadcastMessageId::BlockHash {
                        hash: hash.clone(),
                    }),
                    peer_id: None,
                    reason: "Failed to apply block".to_owned(),
                });
            }
            TransitionFrontierSyncAction::BlocksNextApplySuccess {
                hash,
                just_emitted_a_proof: _,
            } => {
                if let Some(stats) = store.service.stats() {
                    stats.block_producer().block_apply_end(meta.time(), hash);
                }

                if !store.dispatch(TransitionFrontierSyncAction::BlocksNextApplyInit) {
                    store.dispatch(TransitionFrontierSyncAction::BlocksSuccess);
                }
                // Only our block's apply changes the history: a no-op
                // otherwise (F-producer).
                store.service.save_block_production();
            }
            TransitionFrontierSyncAction::BlocksSendToArchive { data, .. } => {
                store.service().send_to_archive(data.clone());
            }
            TransitionFrontierSyncAction::BlocksSuccess => {}
            // Bootstrap/Catchup is practically complete at this point.
            // This effect is where the finalization part needs to be
            // executed, which is mostly to grab some data that we need
            // from previous chain, before it's discarded after dispatching
            // `TransitionFrontierSyncedAction`.
            TransitionFrontierSyncAction::CommitInit => {
                let transition_frontier = &store.state.get().transition_frontier;
                let TransitionFrontierSyncState::BlocksSuccess {
                    chain,
                    root_snarked_ledger_updates,
                    needed_protocol_states,
                    ..
                } = &transition_frontier.sync
                else {
                    return;
                };
                let Some(new_root) = chain.first() else {
                    return;
                };
                let Some(new_best_tip) = chain.last() else {
                    return;
                };
                let ledgers_to_keep = chain
                    .iter()
                    .map(|block| &block.block)
                    .collect::<LedgersToKeep>();
                let mut root_snarked_ledger_updates = root_snarked_ledger_updates.clone();
                if transition_frontier
                    .best_chain
                    .iter()
                    .any(|b| b.hash() == new_root.hash())
                {
                    let old_chain = transition_frontier
                        .best_chain
                        .iter()
                        .map(AppliedBlock::block_with_hash);
                    root_snarked_ledger_updates
                        .extend_with_needed(new_root.block_with_hash(), old_chain);
                }

                let mut root_history = transition_frontier.root_history.clone();
                root_history.advance(&transition_frontier.best_chain, &new_root.block);
                // Historical scan states also need states preceding the current root.
                let needed_protocol_states = needed_protocol_states
                    .iter()
                    .chain(&transition_frontier.needed_protocol_states)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .chain(
                        transition_frontier
                            .best_chain
                            .iter()
                            .chain(chain.iter())
                            .map(|b| (b.hash().clone(), b.header().protocol_state.clone())),
                    )
                    .collect();

                store.dispatch(LedgerWriteAction::Init {
                    request: LedgerWriteRequest::Commit {
                        root_history,
                        ledgers_to_keep,
                        root_snarked_ledger_updates,
                        needed_protocol_states,
                        new_root: new_root.clone(),
                        new_best_tip: new_best_tip.clone(),
                    },
                    on_init: redux::callback!(
                        on_frontier_commit_init(_request: LedgerWriteRequest) -> crate::Action {
                            TransitionFrontierSyncAction::CommitPending
                        }
                    ),
                });
            }
            TransitionFrontierSyncAction::CommitPending => {}
            TransitionFrontierSyncAction::CommitSuccess { .. } => {
                unreachable!("handled in parent effects to avoid cloning")
            }
            TransitionFrontierSyncAction::Ledger(_) => {}
            TransitionFrontierSyncAction::RestoreInit { .. } => {
                if !store.dispatch(TransitionFrontierSyncAction::BlocksNextApplyInit) {
                    // The root alone: commit it.
                    store.dispatch(TransitionFrontierSyncAction::BlocksSuccess);
                }
            }
        }
    }
}

// Helper functions

/// Gets from the current state the best tip sync target
fn sync_best_tip(state: &crate::State) -> ArcBlockWithHash {
    state.transition_frontier.sync.best_tip().unwrap().clone()
}

/// For snarked ledger sync targets, copy the previous snarked ledger if required
fn maybe_copy_ledgers_for_sync<S>(
    store: &mut Store<S>,
    previous_root_snarked_ledger_hash: Option<LedgerHash>,
    best_tip: &ArcBlockWithHash,
) -> Result<bool, String>
where
    S: TransitionFrontierSyncLedgerSnarkedService,
{
    let sync = &store.state().transition_frontier.sync;

    match sync {
        TransitionFrontierSyncState::StakingLedgerPending(_) => {
            prepare_staking_epoch_ledger_for_sync(store, best_tip)
        }
        TransitionFrontierSyncState::NextEpochLedgerPending(_) => {
            prepare_next_epoch_ledger_for_sync(store, best_tip)
        }

        TransitionFrontierSyncState::RootLedgerPending(_) => {
            prepare_transition_frontier_root_ledger_for_sync(
                store,
                previous_root_snarked_ledger_hash,
                best_tip,
            )
        }
        _ => Ok(true),
    }
}

/// Copies (if necessary) the genesis ledger into the sync ledger state
/// for the staking epoch ledger to use as a starting point.
fn prepare_staking_epoch_ledger_for_sync<S>(
    store: &mut Store<S>,
    best_tip: &ArcBlockWithHash,
) -> Result<bool, String>
where
    S: TransitionFrontierSyncLedgerSnarkedService,
{
    let target = SyncLedgerTarget::staking_epoch(best_tip).snarked_ledger_hash;
    let origin = best_tip.genesis_ledger_hash().clone();

    store
        .service()
        .copy_snarked_ledger_contents_for_sync(vec![origin], target, false)
}

/// Copies (if necessary) the staking ledger into the sync ledger state
/// for the next epoch ledger to use as a starting point.
fn prepare_next_epoch_ledger_for_sync<S>(
    store: &mut Store<S>,
    best_tip: &ArcBlockWithHash,
) -> Result<bool, String>
where
    S: TransitionFrontierSyncLedgerSnarkedService,
{
    let sync = &store.state().transition_frontier.sync;
    let root_block = sync.root_block().unwrap();
    let Some(next_epoch_sync) = SyncLedgerTarget::next_epoch(best_tip, root_block) else {
        return Ok(false);
    };
    let target = next_epoch_sync.snarked_ledger_hash;
    let origin = SyncLedgerTarget::staking_epoch(best_tip).snarked_ledger_hash;

    store
        .service()
        .copy_snarked_ledger_contents_for_sync(vec![origin], target, false)
}

/// Copies (if necessary) the next epoch ledger into the sync ledger state
/// for the transition frontier root ledger to use as a starting point.
fn prepare_transition_frontier_root_ledger_for_sync<S>(
    store: &mut Store<S>,
    previous_root_snarked_ledger_hash: Option<LedgerHash>,
    best_tip: &ArcBlockWithHash,
) -> Result<bool, String>
where
    S: TransitionFrontierSyncLedgerSnarkedService,
{
    let transition_frontier = &store.state().transition_frontier;
    let root_block = transition_frontier
        .sync
        .root_block()
        .expect("Sync root block cannot be missing");

    let candidate_origins = root_ledger_copy_candidates(
        previous_root_snarked_ledger_hash,
        transition_frontier.root(),
        best_tip,
        root_block,
    );

    let target = root_block.snarked_ledger_hash().clone();

    store
        .service()
        .copy_snarked_ledger_contents_for_sync(candidate_origins, target, false)
}

/// The ledgers to start a root ledger sync from, closest to the target first.
/// The ledger service copies the first one it has, committed or among the sync
/// ledgers.
///
/// - The previous root's snarked ledger: while a sync is in progress, the
///   previous sync target's, synced or being synced
///   ([`crate::transition_frontier::TransitionFrontierState::previous_root_snarked_ledger_hash`]).
/// - The frontier root's, which a synced node has committed. A node that falls
///   behind its sync target's root while catching up has it, where it may not
///   have the previous sync target's. During a bootstrap the frontier root is
///   the genesis block, whose ledger is far from the target, or absent with a
///   genesis config without ledgers: it is left out.
/// - The next epoch ledger, then the staking ledger.
pub(super) fn root_ledger_copy_candidates(
    previous_root_snarked_ledger_hash: Option<LedgerHash>,
    frontier_root: Option<&ArcBlockWithHash>,
    best_tip: &ArcBlockWithHash,
    root_block: &ArcBlockWithHash,
) -> Vec<LedgerHash> {
    let frontier_root_snarked_ledger_hash = frontier_root
        .map(|block| block.snarked_ledger_hash())
        .filter(|hash| *hash != best_tip.genesis_ledger_hash())
        .cloned();
    let next_epoch_ledger_hash =
        SyncLedgerTarget::next_epoch(best_tip, root_block).map(|target| target.snarked_ledger_hash);
    let staking_epoch_ledger_hash = SyncLedgerTarget::staking_epoch(best_tip).snarked_ledger_hash;

    let mut candidates: Vec<LedgerHash> = Vec::with_capacity(4);
    for hash in previous_root_snarked_ledger_hash
        .into_iter()
        .chain(frontier_root_snarked_ledger_hash)
        .chain(next_epoch_ledger_hash)
        .chain([staking_epoch_ledger_hash])
    {
        if !candidates.contains(&hash) {
            candidates.push(hash);
        }
    }
    candidates
}
