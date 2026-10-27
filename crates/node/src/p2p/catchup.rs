//! Serve the applied frontier and its bounded history of retired roots.

use crate::transition_frontier::root_history::RootHistory;
use mina_core::{
    block::{AppliedBlock, ArcBlockWithHash},
    consensus::consensus_take,
};
use mina_p2p::channels::rpc::{CatchupRequest, CatchupResponse, MAX_CATCHUP_CHAIN_LENGTH};
use mina_p2p_messages::{rpc, v2};

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/sync_handler/sync_handler.ml#L169-L236>
pub fn respond(
    chain: &[AppliedBlock],
    history: &RootHistory,
    request: CatchupRequest,
) -> CatchupResponse {
    let find = |hash: &v2::StateHash| {
        chain
            .iter()
            .find(|b| b.hash() == hash)
            .map(|b| &b.block)
            .or_else(|| history.get(hash))
    };
    match request {
        CatchupRequest::Ancestry(query) => {
            let response = (|| {
                let root = chain.first()?;
                let tip = chain.last()?;
                let proof = chain
                    .iter()
                    .skip(1)
                    .map(|b| b.header().protocol_state.body.try_hash().map(|h| h.0))
                    .collect::<Result<_, _>>()
                    .ok()?;
                ancestry(&root.block, &tip.block, proof, *query)
            })();
            CatchupResponse::Ancestry(response)
        }
        CatchupRequest::TransitionChain(hashes) => {
            // Super catchup deliberately omits missing blocks, preserving
            // the request's order and duplicates. Too many hashes is None.
            let blocks =
                (!chain.is_empty() && hashes.len() <= MAX_CATCHUP_CHAIN_LENGTH).then(|| {
                    hashes
                        .iter()
                        .filter_map(|hash| {
                            find(&v2::StateHash::from(v2::DataHashLibStateHashStableV1(
                                hash.clone(),
                            )))
                            .map(|b| b.block.as_ref().clone())
                        })
                        .collect()
                });
            CatchupResponse::TransitionChain(blocks)
        }
        CatchupRequest::TransitionChainProof(hash) => {
            // Merkle_list_prover excludes the first block's body and lists
            // every subsequent body in ascending chain order, including the
            // requested block. The oldest available ancestor has an empty proof.
            // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_chain_prover/transition_chain_prover.ml#L53-L66>
            let proof = (|| {
                if chain.is_empty() {
                    return None;
                }
                let hash = v2::StateHash::from(v2::DataHashLibStateHashStableV1(hash));
                let mut block = find(&hash)?;
                let mut bodies = Vec::new();
                // Follow parent hashes, never infer ancestry from vector positions.
                while let Some(parent) = find(block.pred_hash()) {
                    bodies.push(block.header().protocol_state.body.try_hash().ok()?.0);
                    block = parent;
                    if bodies.len() > chain.len() + history.iter().count() {
                        return None;
                    }
                }
                bodies.reverse();
                Some((block.hash().inner().0.clone(), bodies.into_iter().collect()))
            })();
            CatchupResponse::TransitionChainProof(proof)
        }
        CatchupRequest::TransitionKnowledge => CatchupResponse::TransitionKnowledge(
            chain.iter().map(|b| b.hash().inner().0.clone()).collect(),
        ),
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/best_tip_prover/best_tip_prover.ml#L47-L77>
fn ancestry(
    root: &ArcBlockWithHash,
    tip: &ArcBlockWithHash,
    proof: mina_p2p_messages::list::List<mina_p2p_messages::common::StateBodyHashV1>,
    query: rpc::GetAncestryV2Query,
) -> rpc::GetAncestryV2Response {
    // The query hash is only used for consensus tie-breaking, never lookup.
    if proof.len() != tip.constants().k.as_u32() as usize
        && root.hash() != tip.header().genesis_state_hash()
    {
        return None;
    }
    // The query's consensus state comes from the peer unverified. Unless its
    // epoch length and window size are ours, consensus selection could divide
    // by zero or index past the window, so treat it like an unknown request.
    if !same_consensus_shape(tip.consensus_state(), &query.data) {
        return None;
    }
    let seen_hash = v2::StateHash::from(v2::DataHashLibStateHashStableV1(query.hash));
    if consensus_take(tip.consensus_state(), &query.data, tip.hash(), &seen_hash) {
        return None;
    }
    Some(rpc::ProofCarryingDataWithHashV1 {
        data: tip.block.as_ref().clone(),
        proof: (proof, root.block.as_ref().clone()),
    })
}

fn same_consensus_shape(
    ours: &v2::ConsensusProofOfStakeDataConsensusStateValueStableV2,
    theirs: &v2::ConsensusProofOfStakeDataConsensusStateValueStableV2,
) -> bool {
    theirs.curr_global_slot_since_hard_fork.slots_per_epoch
        == ours.curr_global_slot_since_hard_fork.slots_per_epoch
        && theirs.sub_window_densities.len() == ours.sub_window_densities.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn respond(chain: &[AppliedBlock], request: CatchupRequest) -> CatchupResponse {
        super::respond(chain, &RootHistory::default(), request)
    }
    use mina_core::block::BlockWithHash;
    use mina_p2p_messages::{binprot::BinProtRead, list::List, rpc_kernel::ResponsePayload};
    use std::sync::Arc;

    const ANCESTRY: &[u8] =
        include_bytes!("../../../p2p-messages/tests/files/mesa/get_ancestry_v3.bin");

    fn captured() -> rpc::ProofCarryingDataWithHashV1<
        v2::MinaBlockBlockStableV2,
        (
            List<mina_p2p_messages::common::StateBodyHashV1>,
            v2::MinaBlockBlockStableV2,
        ),
    > {
        ResponsePayload::<rpc::GetAncestryV2Response>::binprot_read(&mut &ANCESTRY[..])
            .unwrap()
            .0
            .unwrap()
            .0
            .unwrap()
    }

    fn block(block: v2::MinaBlockBlockStableV2) -> AppliedBlock {
        AppliedBlock {
            block: BlockWithHash::try_new(Arc::new(block)).unwrap(),
            just_emitted_a_proof: false,
        }
    }

    #[test]
    fn retired_root_chain_and_proof_match_ocaml_capture() {
        let bytes = include_bytes!(
            "../../../p2p-messages/tests/files/mesa/get_transition_chain_v2_adjacent.bin"
        );
        let blocks = ResponsePayload::<Option<List<v2::MinaBlockBlockStableV2>>>::binprot_read(
            &mut &bytes[..],
        )
        .unwrap()
        .0
        .unwrap()
        .0
        .unwrap();
        let old_chain = blocks
            .iter()
            .take(2)
            .cloned()
            .map(block)
            .collect::<Vec<_>>();
        let mut history = RootHistory::default();
        history.advance(&old_chain, &old_chain[1].block);
        let chain = &old_chain[1..];
        let hashes = [
            old_chain[0].hash().inner().0.clone(),
            old_chain[1].hash().inner().0.clone(),
            old_chain[0].hash().inner().0.clone(),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            super::respond(chain, &history, CatchupRequest::TransitionChain(hashes)).into_bytes(),
            bytes
        );
        let proof_bytes = include_bytes!(
            "../../../p2p-messages/tests/files/mesa/get_transition_chain_proof_v1.bin"
        );
        let proof = ResponsePayload::<rpc::GetTransitionChainProofV1ForV2Response>::binprot_read(
            &mut &proof_bytes[..],
        )
        .unwrap()
        .0
        .unwrap()
        .0
        .unwrap();
        let CatchupResponse::TransitionChainProof(Some((base, bodies))) = super::respond(
            chain,
            &history,
            CatchupRequest::TransitionChainProof(chain[0].hash().inner().0.clone()),
        ) else {
            panic!("missing proof")
        };
        assert_eq!(base, old_chain[0].hash().inner().0);
        assert_eq!(bodies, List::one(proof.1.back().unwrap().clone()));
        let CatchupResponse::TransitionChainProof(Some((base, bodies))) = super::respond(
            chain,
            &history,
            CatchupRequest::TransitionChainProof(old_chain[0].hash().inner().0.clone()),
        ) else {
            panic!("missing historical proof")
        };
        assert_eq!(base, old_chain[0].hash().inner().0);
        assert!(bodies.is_empty());
        let outside = old_chain[0].pred_hash().inner().0.clone();
        assert!(matches!(
            super::respond(
                chain,
                &history,
                CatchupRequest::TransitionChainProof(outside.clone())
            ),
            CatchupResponse::TransitionChainProof(None)
        ));
        assert!(
            matches!(super::respond(chain, &history, CatchupRequest::TransitionChain(List::one(outside))), CatchupResponse::TransitionChain(Some(blocks)) if blocks.is_empty())
        );
        // Knowledge remains the current best path, excluding history.
        assert!(
            matches!(super::respond(chain, &history, CatchupRequest::TransitionKnowledge), CatchupResponse::TransitionKnowledge(hashes) if hashes == List::one(chain[0].hash().inner().0.clone()))
        );
    }

    #[test]
    fn ancestry_matches_ocaml_and_enforces_consensus_and_depth() {
        let capture = captured();
        let tip = block(capture.data);
        let root = block(capture.proof.1);
        let query = rpc::GetAncestryV2Query {
            data: tip.consensus_state().clone(),
            hash: tip.hash().inner().0.clone(),
        };
        let response = ancestry(
            &root.block,
            &tip.block,
            capture.proof.0.clone(),
            query.clone(),
        );
        assert_eq!(CatchupResponse::Ancestry(response).into_bytes(), ANCESTRY);
        // The hash may be unknown or below the retained root; a weaker
        // observed consensus state is still eligible for our proof.
        let below_root = rpc::GetAncestryV2Query {
            data: root.consensus_state().clone(),
            hash: root.pred_hash().inner().0.clone(),
        };
        let response = ancestry(&root.block, &tip.block, capture.proof.0.clone(), below_root);
        assert_eq!(CatchupResponse::Ancestry(response).into_bytes(), ANCESTRY);
        // A non-genesis root needs exactly k links, including at the boundary.
        let mut short = capture.proof.0.clone();
        short.pop_back();
        assert!(ancestry(&root.block, &tip.block, short, query.clone()).is_none());
        let mut long = capture.proof.0.clone();
        long.push_back(capture.proof.0.iter().next().unwrap().clone());
        assert!(ancestry(&root.block, &tip.block, long, query.clone()).is_none());
        // A real newer consensus state must not be answered with an older tip.
        assert!(ancestry(
            &root.block,
            &root.block,
            capture.proof.0.clone(),
            query.clone()
        )
        .is_none());
        // Malformed queries get no answer instead of a panic: a zero epoch
        // length with a different epoch, and an empty window.
        let mut zero_epoch_length = query.clone();
        zero_epoch_length
            .data
            .curr_global_slot_since_hard_fork
            .slots_per_epoch = 0.into();
        zero_epoch_length.data.epoch_count =
            (zero_epoch_length.data.epoch_count.as_u32() + 1).into();
        assert!(ancestry(
            &root.block,
            &tip.block,
            capture.proof.0.clone(),
            zero_epoch_length
        )
        .is_none());
        let mut empty_window = query;
        empty_window.data.sub_window_densities = List::new();
        assert!(ancestry(&root.block, &tip.block, capture.proof.0, empty_window).is_none());
    }

    #[test]
    fn chain_omits_unknown_and_below_root_preserves_duplicates_and_caps_at_twenty() {
        let root = block(captured().proof.1);
        let known = root.hash().inner().0.clone();
        let below_root = root.pred_hash().inner().0.clone();
        let chain = vec![root];
        let hashes = [below_root.clone(), known.clone(), known.clone()]
            .into_iter()
            .collect();
        let CatchupResponse::TransitionChain(Some(blocks)) =
            respond(&chain, CatchupRequest::TransitionChain(hashes))
        else {
            panic!("chain unavailable")
        };
        assert_eq!(blocks.len(), 2);
        assert!(blocks.iter().all(|b| b == chain[0].block().as_ref()));
        for (length, available) in [(0, true), (20, true), (21, false)] {
            let request = CatchupRequest::TransitionChain(
                std::iter::repeat_n(known.clone(), length).collect(),
            );
            let CatchupResponse::TransitionChain(blocks) = respond(&chain, request) else {
                unreachable!()
            };
            assert_eq!(blocks.is_some(), available);
            if let Some(blocks) = blocks {
                assert_eq!(blocks.len(), length);
            }
        }
        let CatchupResponse::TransitionChain(Some(missing)) = respond(
            &chain,
            CatchupRequest::TransitionChain(List::one(below_root)),
        ) else {
            panic!("expected Some []")
        };
        assert!(missing.is_empty());
        assert!(matches!(
            respond(&[], CatchupRequest::TransitionChain(List::new())),
            CatchupResponse::TransitionChain(None)
        ));
    }

    #[test]
    fn multi_hash_response_and_nonempty_proof_match_ocaml_capture() {
        let bytes = include_bytes!(
            "../../../p2p-messages/tests/files/mesa/get_transition_chain_v2_adjacent.bin"
        );
        let blocks = ResponsePayload::<Option<List<v2::MinaBlockBlockStableV2>>>::binprot_read(
            &mut &bytes[..],
        )
        .unwrap()
        .0
        .unwrap()
        .0
        .unwrap();
        assert_eq!(blocks.len(), 3);
        let chain = blocks
            .iter()
            .take(2)
            .cloned()
            .map(block)
            .collect::<Vec<_>>();
        assert_eq!(chain[1].pred_hash(), chain[0].hash());
        // A delayed proof may be adopted locally after gossip expires, as in
        // OCaml's producer. It must remain fetchable as a descendant's parent.
        let late = chain[1]
            .timestamp()
            .checked_add(24 * 60 * 60 * 1_000_000_000)
            .unwrap();
        assert!(!mina_p2p::network::pubsub::P2pNetworkPubsubState::default()
            .block_gossip
            .can_announce(
                &mina_p2p_messages::gossip::GossipNetMessageV2::NewState(chain[1].block().clone()),
                late,
            ));
        assert!(mina_p2p::P2pCatchupRpcConfig::default().allows(
            &mina_p2p::identity::SecretKey::deterministic(7)
                .public_key()
                .peer_id()
        ));
        let hashes = [
            chain[0].hash().inner().0.clone(),
            chain[1].hash().inner().0.clone(),
            chain[0].hash().inner().0.clone(),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            respond(&chain, CatchupRequest::TransitionChain(hashes)).into_bytes(),
            bytes
        );
        let proof_bytes = include_bytes!(
            "../../../p2p-messages/tests/files/mesa/get_transition_chain_proof_v1.bin"
        );
        let proof = ResponsePayload::<rpc::GetTransitionChainProofV1ForV2Response>::binprot_read(
            &mut &proof_bytes[..],
        )
        .unwrap()
        .0
        .unwrap()
        .0
        .unwrap();
        let CatchupResponse::TransitionChainProof(Some((base, bodies))) = respond(
            &chain,
            CatchupRequest::TransitionChainProof(chain[1].hash().inner().0.clone()),
        ) else {
            panic!("missing proof")
        };
        assert_eq!(base, chain[0].hash().inner().0);
        // OCaml's proof reaches further into its root history; the common
        // suffix must be exactly the final body hash captured from OCaml.
        assert_eq!(bodies, List::one(proof.1.back().unwrap().clone()));
        let mut hash = chain[0].hash().clone();
        for body in bodies {
            hash = v2::StateHash::try_from_hashes(&hash, &v2::MinaBaseStateBodyHashStableV1(body))
                .unwrap();
        }
        assert_eq!(hash, *chain[1].hash());
    }

    #[test]
    fn root_proof_is_empty_and_unknown_or_below_root_is_none() {
        let root = block(captured().proof.1);
        let hash = root.hash().inner().0.clone();
        let below_root = root.pred_hash().inner().0.clone();
        let chain = vec![root];
        let CatchupResponse::TransitionChainProof(Some((base, bodies))) =
            respond(&chain, CatchupRequest::TransitionChainProof(hash.clone()))
        else {
            panic!("no root proof")
        };
        assert_eq!(base, hash);
        assert!(bodies.is_empty());
        assert!(matches!(
            respond(&chain, CatchupRequest::TransitionChainProof(below_root)),
            CatchupResponse::TransitionChainProof(None)
        ));
        assert!(matches!(
            respond(&[], CatchupRequest::TransitionChainProof(hash)),
            CatchupResponse::TransitionChainProof(None)
        ));
    }

    #[test]
    fn knowledge_is_root_to_tip_and_empty_without_frontier() {
        let capture = captured();
        let chain = vec![block(capture.proof.1), block(capture.data)];
        let CatchupResponse::TransitionKnowledge(hashes) =
            respond(&chain, CatchupRequest::TransitionKnowledge)
        else {
            unreachable!()
        };
        assert_eq!(
            hashes.into_iter().collect::<Vec<_>>(),
            vec![
                chain[0].hash().inner().0.clone(),
                chain[1].hash().inner().0.clone()
            ]
        );
        assert!(
            matches!(respond(&[], CatchupRequest::TransitionKnowledge), CatchupResponse::TransitionKnowledge(hashes) if hashes.is_empty())
        );
    }
}
