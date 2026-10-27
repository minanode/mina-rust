use std::collections::{BTreeMap, BTreeSet};

use mina_p2p_messages::v2::StateHash;
use serde::{Deserialize, Serialize};

use mina_core::{
    block::ArcBlockWithHash,
    consensus::{
        consensus_take, ConsensusLongRangeForkDecisionReason, ConsensusShortRangeForkDecisionReason,
    },
};

use crate::{
    p2p::PeerId, snark::block_verify::SnarkBlockVerifyId,
    transition_frontier::TransitionFrontierState,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ConsensusShortRangeForkDecision {
    TakeNoBestTip,
    Take(ConsensusShortRangeForkDecisionReason),
    Keep(ConsensusShortRangeForkDecisionReason),
}

impl ConsensusShortRangeForkDecision {
    pub fn use_as_best_tip(&self) -> bool {
        matches!(self, Self::TakeNoBestTip | Self::Take(_))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ConsensusLongRangeForkDecision {
    Keep(ConsensusLongRangeForkDecisionReason),
    Take(ConsensusLongRangeForkDecisionReason),
}

impl ConsensusLongRangeForkDecision {
    pub fn use_as_best_tip(&self) -> bool {
        matches!(self, Self::Take(_))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TransitionFrontierCandidateStatus {
    Received {
        time: redux::Timestamp,
    },
    Prevalidated,
    SnarkVerifyPending {
        time: redux::Timestamp,
        req_id: SnarkBlockVerifyId,
    },
    SnarkVerifySuccess {
        time: redux::Timestamp,
    },
}

impl TransitionFrontierCandidateStatus {
    pub fn is_received(&self) -> bool {
        matches!(self, Self::Received { .. })
    }

    pub fn is_prevalidated(&self) -> bool {
        matches!(self, Self::Prevalidated)
    }

    pub fn is_snark_verify_pending(&self) -> bool {
        matches!(self, Self::SnarkVerifyPending { .. })
    }

    pub fn is_snark_verify_success(&self) -> bool {
        matches!(self, Self::SnarkVerifySuccess { .. })
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, Self::SnarkVerifyPending { .. })
    }

    /// When the block's proof was verified, if it has been.
    pub fn snark_verified_at(&self) -> Option<redux::Timestamp> {
        match self {
            Self::SnarkVerifySuccess { time } => Some(*time),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TransitionFrontierCandidateState {
    pub block: ArcBlockWithHash,
    pub status: TransitionFrontierCandidateStatus,
    pub chain_proof: Option<(Vec<StateHash>, ArcBlockWithHash)>,
    /// Peers asked for their best tip with proof for this block's chain
    /// proof. None is asked twice: one whose answer lacked the block won't
    /// have it as its best tip later either, as best tips only get better.
    #[serde(default)]
    pub chain_proof_asked: BTreeSet<PeerId>,
}

impl Ord for TransitionFrontierCandidateState {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if self.eq(other) {
            return std::cmp::Ordering::Equal;
        }
        let is_candidate_better = consensus_take(
            self.block.consensus_state(),
            other.block.consensus_state(),
            self.block.hash(),
            other.block.hash(),
        );
        match is_candidate_better {
            true => std::cmp::Ordering::Less,
            false => std::cmp::Ordering::Greater,
        }
    }
}

impl PartialOrd for TransitionFrontierCandidateState {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for TransitionFrontierCandidateState {}

impl PartialEq for TransitionFrontierCandidateState {
    fn eq(&self, other: &Self) -> bool {
        self.block.hash() == other.block.hash()
    }
}

impl TransitionFrontierCandidateState {
    pub fn height(&self) -> u32 {
        self.block.height()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TransitionFrontierCandidatesState {
    /// Maintains an ordered list of transition frontier Candidates,
    /// ordered using consensus rules worst to best.
    ordered: BTreeSet<TransitionFrontierCandidateState>,
    /// Candidate block hashes, which failed either the prevalidation
    /// or block proof verification. We move them here so that they
    /// consume less memory while still preventing us from triggering
    /// revalidation for an invalid block if we receive it on p2p again.
    invalid: BTreeMap<StateHash, u32>,
    /// When a peer first sent us a best tip, its answer to our request or a
    /// gossiped block, a genesis block included: OCaml's first message, which
    /// makes its node online (F-dash3).
    #[serde(default)]
    pub(super) first_peer_best_tip: Option<redux::Timestamp>,
}

impl TransitionFrontierCandidatesState {
    pub fn memory_debug_len(&self) -> usize {
        self.ordered.len()
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// When a peer first sent us a best tip since the start, if one has.
    pub fn first_peer_best_tip(&self) -> Option<redux::Timestamp> {
        self.first_peer_best_tip
    }

    pub fn contains(&self, hash: &StateHash) -> bool {
        self.invalid.contains_key(hash) || self.get(hash).is_some()
    }

    pub(super) fn get(&self, hash: &StateHash) -> Option<&TransitionFrontierCandidateState> {
        self.ordered.iter().rev().find(|s| s.block.hash() == hash)
    }

    pub(super) fn add(
        &mut self,
        time: redux::Timestamp,
        block: ArcBlockWithHash,
        chain_proof: Option<(Vec<StateHash>, ArcBlockWithHash)>,
    ) {
        self.ordered.insert(TransitionFrontierCandidateState {
            block,
            status: TransitionFrontierCandidateStatus::Received { time },
            chain_proof,
            chain_proof_asked: Default::default(),
        });
    }

    fn update(
        &mut self,
        hash: &StateHash,
        update: impl FnOnce(TransitionFrontierCandidateState) -> TransitionFrontierCandidateState,
    ) -> bool {
        let Some(state) = self.get(hash).cloned() else {
            return false;
        };
        self.ordered.remove(&state);
        self.ordered.insert(update(state));
        true
    }

    pub(super) fn update_status(
        &mut self,
        hash: &StateHash,
        update: impl FnOnce(TransitionFrontierCandidateStatus) -> TransitionFrontierCandidateStatus,
    ) -> bool {
        self.update(hash, move |mut state| {
            state.status = update(state.status);
            state
        })
    }

    pub(super) fn invalidate(&mut self, hash: &StateHash, is_forever_invalid: bool) {
        self.ordered.retain(|s| {
            if s.block.hash() == hash {
                if is_forever_invalid {
                    self.invalid.insert(hash.clone(), s.block.global_slot());
                }
                false
            } else {
                true
            }
        });
    }

    pub(super) fn set_chain_proof(
        &mut self,
        hash: &StateHash,
        chain_proof: (Vec<StateHash>, ArcBlockWithHash),
    ) -> bool {
        self.update(hash, move |mut s| {
            s.chain_proof = Some(chain_proof);
            s
        })
    }

    pub(super) fn prune(&mut self) {
        let mut has_reached_best_candidate = false;
        let Some(best_candidate_hash) = self.best_verified().map(|s| s.block.hash().clone()) else {
            return;
        };

        // prune all blocks that are worse(consensus-wise) than the best
        // verified candidate.
        self.ordered.retain(|s| {
            if s.block.hash() == &best_candidate_hash {
                // prune all invalid block hashes which are for older
                // slots than the current best candidate.
                let best_candidate_slot = s.block.global_slot();
                self.invalid.retain(|_, slot| *slot >= best_candidate_slot);

                has_reached_best_candidate = true;
            }

            has_reached_best_candidate
        });
    }

    pub(super) fn best(&self) -> Option<&TransitionFrontierCandidateState> {
        self.ordered.last()
    }

    pub fn best_verified(&self) -> Option<&TransitionFrontierCandidateState> {
        self.ordered
            .iter()
            .rev()
            .find(|s| s.status.is_snark_verify_success())
    }

    pub(super) fn add_chain_proof_asked(&mut self, hash: &StateHash, peer_id: PeerId) -> bool {
        self.update(hash, move |mut s| {
            s.chain_proof_asked.insert(peer_id);
            s
        })
    }

    /// The best verified candidate, if it should be our next sync target but
    /// lacks a chain proof: it doesn't extend our best chain, so the proof has
    /// to come from a peer's best tip answer.
    ///
    /// During a bootstrap there is no best chain yet, so every new best tip a
    /// peer gossips needs one.
    pub fn chain_proof_wanted(
        &self,
        transition_frontier: &TransitionFrontierState,
    ) -> Option<&TransitionFrontierCandidateState> {
        let candidate = self.best_verified()?;
        if candidate.chain_proof.is_some() {
            return None;
        }
        let better_than_ours = [
            transition_frontier.best_tip(),
            transition_frontier.sync.best_tip(),
        ]
        .into_iter()
        .flatten()
        .all(|b| {
            b.hash() != candidate.block.hash()
                && consensus_take(
                    b.consensus_state(),
                    candidate.block.consensus_state(),
                    b.hash(),
                    candidate.block.hash(),
                )
        });
        if !better_than_ours {
            return None;
        }
        // A block on our best tip or on its predecessor gets its proof from
        // our best chain (`block_chain_proof`), which is built only then.
        let extends_best_chain = transition_frontier.best_tip().is_some_and(|best_tip| {
            best_tip.hash() == candidate.block.pred_hash()
                || best_tip.pred_hash() == candidate.block.pred_hash()
        }) && self
            .block_chain_proof(candidate, transition_frontier)
            .is_some();
        (!extends_best_chain).then_some(candidate)
    }

    pub fn is_chain_proof_needed(&self, hash: &StateHash) -> bool {
        self.get(hash).is_some_and(|s| s.chain_proof.is_none())
    }

    pub fn best_verified_block(&self) -> Option<&ArcBlockWithHash> {
        self.best_verified().map(|s| &s.block)
    }

    pub fn best_verified_block_chain_proof(
        &self,
        transition_frontier: &TransitionFrontierState,
    ) -> Option<(Vec<StateHash>, ArcBlockWithHash)> {
        self.block_chain_proof(self.best_verified()?, transition_frontier)
    }

    fn block_chain_proof(
        &self,
        block_state: &TransitionFrontierCandidateState,
        transition_frontier: &TransitionFrontierState,
    ) -> Option<(Vec<StateHash>, ArcBlockWithHash)> {
        let pred_hash = block_state.block.pred_hash();
        block_state.chain_proof.clone().or_else(|| {
            let old_best_tip = transition_frontier.best_tip()?;
            let mut iter = transition_frontier.best_chain.iter();
            if old_best_tip.hash() == pred_hash {
                if old_best_tip.height() > old_best_tip.constants().k.as_u32() {
                    iter.next();
                }
                let root_block = iter.next()?.block_with_hash().clone();
                let hashes = iter.map(|b| b.hash().clone()).collect();
                Some((hashes, root_block))
            } else if old_best_tip.pred_hash() == pred_hash {
                let root_block = iter.next()?.block_with_hash().clone();
                let hashes = iter.rev().skip(1).rev().map(|b| b.hash().clone()).collect();
                Some((hashes, root_block))
            } else {
                None
            }
        })
    }
}

/// A ready peer, as [`choose_chain_proof_peer`] sees it.
#[derive(Debug, Clone, Copy)]
pub struct ChainProofPeer {
    pub peer_id: PeerId,
    /// Its best tip, as we know it, is the candidate.
    pub advertised: bool,
    /// Its RPC slot is free and its best tip budget allows a request now.
    pub available: bool,
    /// A best tip request of ours to it is waiting for an answer, and hasn't
    /// timed out.
    pub best_tip_request_pending: bool,
}

/// How long to wait for a peer that advertised a candidate, while it is busy
/// or out of best tip budget, before asking peers that didn't advertise it.
///
/// A busy peer's RPC slot frees within a round trip, and the chain proof query
/// takes it before ledger and block queries do. A peer out of budget may not be
/// asked for up to the 20 s interval: by then the block has spread, and most
/// peers that didn't send it to us have it too.
pub const CHAIN_PROOF_ADVERTISER_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The peer to ask next for a candidate's chain proof, if any should be asked
/// now, given the peers already asked for this candidate.
///
/// - One request at a time per candidate: none while an asked peer's request
///   is pending. After a timeout it is retried with another peer.
/// - A peer that advertised the candidate as its best tip has it, so those go
///   first. While `wait_for_advertisers`, and one that hasn't been asked is
///   busy or out of budget, we wait for it rather than ask a peer that may not
///   have the block yet ([`CHAIN_PROOF_ADVERTISER_WAIT`]).
/// - Then any other peer not yet asked: an OCaml peer forwards a block only
///   once it has it in its frontier, so a peer we didn't hear it from likely
///   has it or a better one.
pub fn choose_chain_proof_peer<R: rand::Rng + ?Sized>(
    asked: &BTreeSet<PeerId>,
    peers: impl IntoIterator<Item = ChainProofPeer>,
    wait_for_advertisers: bool,
    rng: &mut R,
) -> Option<PeerId> {
    use rand::seq::SliceRandom;

    let mut advertisers = Vec::new();
    let mut others = Vec::new();
    let mut advertiser_unavailable = false;
    for peer in peers {
        if asked.contains(&peer.peer_id) {
            if peer.best_tip_request_pending {
                return None;
            }
            continue;
        }
        match (peer.advertised, peer.available) {
            (true, true) => advertisers.push(peer.peer_id),
            (true, false) => advertiser_unavailable = true,
            (false, true) => others.push(peer.peer_id),
            (false, false) => {}
        }
    }
    if !advertisers.is_empty() {
        return advertisers.choose(rng).copied();
    }
    if advertiser_unavailable && wait_for_advertisers {
        return None;
    }
    others.choose(rng).copied()
}

#[cfg(test)]
mod tests {
    use rand::{rngs::StdRng, SeedableRng};

    use super::*;

    fn peer(n: u8, advertised: bool, available: bool) -> ChainProofPeer {
        ChainProofPeer {
            peer_id: PeerId::from_bytes([n; 32]),
            advertised,
            available,
            best_tip_request_pending: false,
        }
    }

    fn choose(asked: &[u8], peers: &[ChainProofPeer]) -> Option<u8> {
        choose_waiting(asked, peers, true)
    }

    fn choose_waiting(
        asked: &[u8],
        peers: &[ChainProofPeer],
        wait_for_advertisers: bool,
    ) -> Option<u8> {
        let asked = asked.iter().map(|n| PeerId::from_bytes([*n; 32])).collect();
        let mut rng = StdRng::seed_from_u64(0);
        choose_chain_proof_peer(
            &asked,
            peers.iter().copied(),
            wait_for_advertisers,
            &mut rng,
        )
        .map(|id| {
            (1..=255)
                .find(|n| PeerId::from_bytes([*n; 32]) == id)
                .expect("a test peer")
        })
    }

    #[test]
    fn asks_a_peer_that_advertised_the_candidate_first() {
        let peers = [
            peer(1, false, true),
            peer(2, true, true),
            peer(3, false, true),
        ];
        assert_eq!(choose(&[], &peers), Some(2));
    }

    #[test]
    fn waits_for_a_busy_advertiser_rather_than_ask_others() {
        let peers = [peer(1, false, true), peer(2, true, false)];
        assert_eq!(choose(&[], &peers), None);
    }

    /// After [`CHAIN_PROOF_ADVERTISER_WAIT`], a busy or out of budget
    /// advertiser no longer holds the request back.
    #[test]
    fn stops_waiting_for_an_unavailable_advertiser() {
        let peers = [peer(1, false, true), peer(2, true, false)];
        assert_eq!(choose_waiting(&[], &peers, false), Some(1));
        // An available advertiser still goes first.
        let peers = [peer(1, false, true), peer(2, true, true)];
        assert_eq!(choose_waiting(&[], &peers, false), Some(2));
    }

    #[test]
    fn a_peer_whose_answer_lacked_the_candidate_is_not_asked_again() {
        // Peer 2 advertised the candidate and was asked; its answer had
        // another best tip. Peer 1 is asked next, peer 2 never again.
        let peers = [peer(1, false, true), peer(2, true, true)];
        assert_eq!(choose(&[2], &peers), Some(1));
        assert_eq!(choose(&[1, 2], &peers), None);
    }

    #[test]
    fn one_request_at_a_time() {
        let mut asked_peer = peer(2, true, false);
        asked_peer.best_tip_request_pending = true;
        let peers = [peer(1, true, true), asked_peer];
        assert_eq!(choose(&[2], &peers), None);
        // Once it has answered, or timed out, the next peer is asked.
        asked_peer.best_tip_request_pending = false;
        let peers = [peer(1, true, true), asked_peer];
        assert_eq!(choose(&[2], &peers), Some(1));
    }

    #[test]
    fn nobody_to_ask() {
        assert_eq!(choose(&[], &[]), None);
        assert_eq!(choose(&[], &[peer(1, false, false)]), None);
    }
}
