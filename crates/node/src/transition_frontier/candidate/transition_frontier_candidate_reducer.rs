use mina_core::{
    block::{ArcBlockWithHash, BlockHash},
    bug_condition,
};
use mina_snark::block_verify::{SnarkBlockVerifyAction, SnarkBlockVerifyError, SnarkBlockVerifyId};

use crate::{
    p2p::channels::rpc::{P2pChannelsRpcAction, P2pRpcRequest},
    transition_frontier::sync::{
        ledger::{
            snarked::TransitionFrontierSyncLedgerSnarkedAction,
            staged::TransitionFrontierSyncLedgerStagedAction,
        },
        TransitionFrontierSyncAction,
    },
    WatchedAccountsAction,
};

use super::{
    choose_chain_proof_peer, ChainProofPeer, TransitionFrontierCandidateAction,
    TransitionFrontierCandidateActionWithMetaRef, TransitionFrontierCandidateStatus,
    TransitionFrontierCandidatesState, CHAIN_PROOF_ADVERTISER_WAIT,
};

impl TransitionFrontierCandidatesState {
    pub fn reducer(
        mut state_context: crate::Substate<Self>,
        action: TransitionFrontierCandidateActionWithMetaRef<'_>,
    ) {
        let Ok(state) = state_context.get_substate_mut() else {
            // TODO: log or propagate
            return;
        };
        let (action, meta) = action.split();

        match action {
            TransitionFrontierCandidateAction::P2pBestTipUpdate { best_tip } => {
                state.first_peer_best_tip.get_or_insert(meta.time());
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(TransitionFrontierCandidateAction::BlockReceived {
                    block: best_tip.clone(),
                    chain_proof: None,
                });

                dispatcher.push(TransitionFrontierSyncLedgerSnarkedAction::PeersQuery);
                dispatcher.push(TransitionFrontierSyncLedgerStagedAction::PartsPeerFetchInit);
                dispatcher.push(TransitionFrontierSyncAction::BlocksPeersQuery);
            }
            TransitionFrontierCandidateAction::BlockReceived { block, chain_proof } => {
                state.add(meta.time(), block.clone(), chain_proof.clone());

                // Dispatch
                let (dispatcher, state) = state_context.into_dispatcher_and_state();

                let allow_block_too_late = allow_block_too_late(state, block);

                let result = state.prevalidate_block(block, allow_block_too_late);
                dispatcher.push(
                    crate::verification::VerificationUpdate::headers(
                        block,
                        crate::verification::Arrival::BestTip,
                        &result,
                    )
                    .action(),
                );
                match result {
                    Ok(()) => {
                        dispatcher.push(
                            TransitionFrontierCandidateAction::BlockPrevalidateSuccess {
                                hash: block.hash().clone(),
                            },
                        );
                    }
                    Err(error) => {
                        dispatcher.push(TransitionFrontierCandidateAction::BlockPrevalidateError {
                            hash: block.hash().clone(),
                            error,
                        });
                    }
                }
            }
            TransitionFrontierCandidateAction::BlockPrevalidateError { hash, error } => {
                state.invalidate(hash, error.is_forever_invalid());
            }
            TransitionFrontierCandidateAction::BlockPrevalidateSuccess { hash } => {
                state.update_status(hash, |_| TransitionFrontierCandidateStatus::Prevalidated);
                let Some(block) = state.get(hash).map(|s| s.block.clone()) else {
                    bug_condition!("TransitionFrontierCandidateAction::BlockPrevalidateSuccess block not found but action enabled");
                    return;
                };

                // Dispatch
                let dispatcher = state_context.into_dispatcher();
                dispatcher.push(SnarkBlockVerifyAction::Init {
                    block: block.into(),
                    on_init: redux::callback!(
                        on_received_block_snark_verify_init((hash: BlockHash, req_id: SnarkBlockVerifyId)) -> crate::Action {
                            TransitionFrontierCandidateAction::BlockSnarkVerifyPending { hash, req_id }
                        }),
                    on_success: redux::callback!(
                        on_received_block_snark_verify_success(hash: BlockHash) -> crate::Action {
                            TransitionFrontierCandidateAction::BlockSnarkVerifySuccess { hash }
                        }),
                    on_error: redux::callback!(
                        on_received_block_snark_verify_error((hash: BlockHash, error: SnarkBlockVerifyError)) -> crate::Action {
                            TransitionFrontierCandidateAction::BlockSnarkVerifyError { hash, error }
                        }),
                });
            }
            TransitionFrontierCandidateAction::BlockChainProofUpdate { hash, chain_proof } => {
                state.set_chain_proof(hash, chain_proof.clone());

                let dispatcher = state_context.into_dispatcher();
                dispatcher
                    .push(TransitionFrontierCandidateAction::TransitionFrontierSyncTargetUpdate);
            }
            TransitionFrontierCandidateAction::BlockSnarkVerifyPending { req_id, hash } => {
                state.update_status(hash, |_| {
                    TransitionFrontierCandidateStatus::SnarkVerifyPending {
                        time: meta.time(),
                        req_id: *req_id,
                    }
                });
            }
            TransitionFrontierCandidateAction::BlockSnarkVerifyError { hash, .. } => {
                state.invalidate(hash, true);
            }
            TransitionFrontierCandidateAction::BlockSnarkVerifySuccess { hash } => {
                state.update_status(hash, |_| {
                    TransitionFrontierCandidateStatus::SnarkVerifySuccess { time: meta.time() }
                });

                // Dispatch
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let Some(block) = global_state
                    .transition_frontier
                    .candidates
                    .best_verified_block()
                else {
                    return;
                };
                for pub_key in global_state.watched_accounts.accounts() {
                    dispatcher.push(WatchedAccountsAction::LedgerInitialStateGetInit {
                        pub_key: pub_key.clone(),
                    });
                    dispatcher.push(WatchedAccountsAction::TransactionsIncludedInBlock {
                        pub_key,
                        block: block.clone(),
                    });
                }

                dispatcher
                    .push(TransitionFrontierCandidateAction::TransitionFrontierSyncTargetUpdate);
                // If it can't be the sync target for want of a chain proof,
                // ask for one now rather than on the next `CheckTimeouts`.
                dispatcher.push(TransitionFrontierCandidateAction::ChainProofPeersQuery);
            }
            TransitionFrontierCandidateAction::TransitionFrontierSyncTargetUpdate => {
                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let Some(best_tip) = state.transition_frontier.candidates.best_verified_block()
                else {
                    bug_condition!(
                        "ConsensusAction::TransitionFrontierSyncTargetUpdate | no chosen best tip"
                    );
                    return;
                };

                let Some((blocks_inbetween, root_block)) = state
                    .transition_frontier
                    .candidates
                    .best_verified_block_chain_proof(&state.transition_frontier)
                else {
                    bug_condition!("ConsensusAction::TransitionFrontierSyncTargetUpdate | no best tip chain proof");
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
                    on_success: None,
                });
            }
            TransitionFrontierCandidateAction::ChainProofPeersQuery => {
                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let Some(candidate) = global_state
                    .transition_frontier
                    .candidates
                    .chain_proof_wanted(&global_state.transition_frontier)
                else {
                    return;
                };
                let Some(p2p) = global_state.p2p.ready() else {
                    return;
                };
                let time = meta.time();
                let peers = p2p
                    .ready_peers_iter()
                    .map(|(peer_id, peer)| ChainProofPeer {
                        peer_id: *peer_id,
                        advertised: peer
                            .best_tip
                            .as_ref()
                            .is_some_and(|b| b.hash() == candidate.block.hash()),
                        available: peer.channels.rpc.can_send_request()
                            && p2p.is_best_tip_request_within_budget(peer_id, time),
                        best_tip_request_pending: matches!(
                            peer.channels.rpc.pending_local_rpc(),
                            Some(P2pRpcRequest::BestTipWithProof)
                        ),
                    });
                // Wait a little for a peer that advertised the candidate,
                // counted from when the candidate was verified.
                let wait_for_advertisers =
                    candidate.status.snark_verified_at().is_none_or(|verified| {
                        time.checked_sub(verified).unwrap_or_default() < CHAIN_PROOF_ADVERTISER_WAIT
                    });
                let chosen = choose_chain_proof_peer(
                    &candidate.chain_proof_asked,
                    peers,
                    wait_for_advertisers,
                    &mut global_state.pseudo_rng(),
                );
                if let Some(peer_id) = chosen {
                    dispatcher.push(TransitionFrontierCandidateAction::ChainProofPeerQueryInit {
                        hash: candidate.block.hash().clone(),
                        peer_id,
                    });
                }
            }
            TransitionFrontierCandidateAction::ChainProofPeerQueryInit { hash, peer_id } => {
                state.add_chain_proof_asked(hash, *peer_id);

                let (dispatcher, global_state) = state_context.into_dispatcher_and_state();
                let Some(peer) = global_state.p2p.get_ready_peer(peer_id) else {
                    bug_condition!("ChainProofPeerQueryInit: peer {peer_id} not ready");
                    return;
                };
                dispatcher.push(P2pChannelsRpcAction::RequestSend {
                    peer_id: *peer_id,
                    id: peer.channels.next_local_rpc_id(),
                    request: Box::new(P2pRpcRequest::BestTipWithProof),
                    on_init: None,
                });
            }
            TransitionFrontierCandidateAction::Prune => {
                state.prune();
            }
        }
    }
}

/// Decide if the time-reception check should be done for this block or not.
///
/// The check is skipped if the block's global_slot is greater than the
/// current best tip and the difference greater than 2.
///
/// Ideally we would differentiate between requested blocks and blocks
/// received from gossip, but this difference doesn't really exist
/// in the WebRTC transport, hence this heuristic.
pub fn allow_block_too_late(state: &crate::State, block: &ArcBlockWithHash) -> bool {
    let (has_greater_blobal_slot, diff_with_best_tip) = state
        .transition_frontier
        .best_tip()
        .map(|b| {
            (
                block.global_slot() > b.global_slot(),
                b.global_slot().abs_diff(block.global_slot()),
            )
        })
        .unwrap_or((false, 0));

    has_greater_blobal_slot && diff_with_best_tip > 2
}
