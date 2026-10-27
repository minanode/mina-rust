//! The sync phase the node reports (F-dash3): `/status`, `/readyz`, GraphQL
//! `syncStatus`, the `mina_sync_status` metric and O1's `NodeSynced` line.
//!
//! The sync itself is `Synced` from the start, on the genesis block alone
//! (`GenesisInject`), and again once a restore from the frontier store
//! commits its stored tip, before any peer has sent a best tip. Neither is
//! synced to the network. OCaml's daemon says `CONNECTING` until a peer
//! connects, `LISTENING` until the first message from the network (the best
//! tips it asks peers for at start, or a gossip), then `BOOTSTRAP` while it
//! has no frontier, `CATCHUP` while catch-up jobs run, and `SYNCED`
//! otherwise. [`reported_sync_phase`] follows it.

use mina_core::{
    block::{AppliedBlock, ArcBlockWithHash},
    consensus::consensus_take,
};

use super::SyncPhase;
use crate::State;

/// The phase to report, from the sync's own phase (`internal`), our best
/// chain, the number of ready peers, whether any peer has sent us a best tip
/// since the start (`heard`, OCaml's first message), the best tips the ready
/// peers last sent, and the best candidate block whose proof we verified.
///
/// A syncing node reports its sync's phase. A `Synced` one reports:
///
/// 1. `Connecting` without a ready peer;
/// 2. `Listening` until a peer has sent a best tip, a genesis block
///    included: at the start, on the genesis block or on a restored tip;
/// 3. on the genesis block, `Bootstrap` when a ready peer's tip is not a
///    genesis block or a block's proof is verified (the sync bootstraps to
///    it), `Listening` when no ready peer has a tip, and `Synced` when every
///    ready peer's tip is a genesis block and that genesis is no more than
///    `k` block slots old. An older genesis stays `Listening` for a usable
///    network tip. This uses the genesis timestamp and constants, so a new
///    custom network can still be `Synced` at genesis;
/// 4. `Catchup` when a verified candidate is better than our tip: the sync
///    catches up to it;
/// 5. `Synced` otherwise.
///
/// Beyond the genesis block, peers' tips are not compared with ours: they are
/// unverified claims, and a peer behind our root, on a sibling fork, or still
/// at genesis while we produce the only blocks, says nothing against a tip
/// the node verified and applied. Only a verified better block makes a
/// synced node catch up. After a restore of a stale tip, the peers' better
/// tips are usually verified by the time the replay commits, so the node
/// reports `Catchup` until it has caught up.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L474-L545>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L2100-L2104>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_router/transition_router.ml#L186-L266>
pub fn reported_sync_phase<'a>(
    internal: SyncPhase,
    best_chain: &[AppliedBlock],
    ready_peers: usize,
    heard: bool,
    peer_tips: impl IntoIterator<Item = &'a ArcBlockWithHash>,
    best_candidate: Option<&ArcBlockWithHash>,
    now: redux::Timestamp,
) -> SyncPhase {
    if !matches!(internal, SyncPhase::Synced) {
        return internal;
    }
    if ready_peers == 0 {
        return SyncPhase::Connecting;
    }
    if !heard {
        return SyncPhase::Listening;
    }
    let Some(our_tip) = best_chain.last().map(AppliedBlock::block_with_hash) else {
        return SyncPhase::Bootstrap;
    };
    if our_tip.is_genesis() {
        let mut tips = peer_tips.into_iter().peekable();
        if tips.peek().is_none() {
            return SyncPhase::Listening;
        }
        if best_candidate.is_some() || tips.any(|tip| !tip.is_genesis()) {
            return SyncPhase::Bootstrap;
        }
        // Use the same k-slot freshness window as is_bootstrapping. This is
        // based on the actual genesis/config, including custom networks.
        return if tip_is_stale(our_tip, now) {
            SyncPhase::Listening
        } else {
            SyncPhase::Synced
        };
    }
    let better_candidate = best_candidate.is_some_and(|candidate| {
        candidate.hash() != our_tip.hash()
            && consensus_take(
                our_tip.consensus_state(),
                candidate.consensus_state(),
                our_tip.hash(),
                candidate.hash(),
            )
    });
    if better_candidate {
        return SyncPhase::Catchup;
    }
    SyncPhase::Synced
}

/// Compare ages without converting an unverified peer's millisecond timestamp
/// to a nanosecond Timestamp or adding a window that could overflow it.
pub(crate) fn tip_is_stale(tip: &ArcBlockWithHash, now: redux::Timestamp) -> bool {
    use std::time::Duration;
    let timestamp = Duration::from_millis(
        tip.header()
            .protocol_state
            .body
            .blockchain_state
            .timestamp
            .as_u64(),
    );
    let window = Duration::from_millis(
        u64::from(tip.constants().k.as_u32())
            .saturating_mul(mina_core::constants::constraint_constants().block_window_duration_ms),
    );
    Duration::from_nanos(u64::from(now))
        .checked_sub(timestamp)
        .is_some_and(|age| age > window)
}

impl State {
    /// The sync phase the node reports: [`reported_sync_phase`] on this
    /// state.
    pub fn reported_sync_phase(&self) -> SyncPhase {
        let frontier = &self.transition_frontier;
        let peers = || self.p2p.ready_peers_iter().map(|(_, peer)| peer);
        reported_sync_phase(
            frontier.sync.sync_phase(),
            &frontier.best_chain,
            peers().count(),
            frontier.candidates.first_peer_best_tip().is_some(),
            peers().filter_map(|peer| peer.best_tip.as_ref()),
            frontier.candidates.best_verified_block(),
            self.time(),
        )
    }

    /// The sync's own state name, as `/status` reports it, unless the sync
    /// is `Synced` and [`Self::reported_sync_phase`] says otherwise: then
    /// that phase, so that the two never disagree.
    pub fn reported_sync_status(&self) -> String {
        let sync = &self.transition_frontier.sync;
        match self.reported_sync_phase() {
            phase if sync.is_synced() && !matches!(phase, SyncPhase::Synced) => phase.to_string(),
            _ => sync.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use mina_p2p_messages::v2;

    use super::*;
    use crate::persist::test_fixtures::{block_after, synthetic_chain, ChainLedgers, ROOT_SLOT};

    fn ledgers() -> ChainLedgers {
        let hash = |n: u64| v2::LedgerHash::from_fp(n.into());
        ChainLedgers {
            staking: hash(1),
            next: hash(2),
            snarked: hash(3),
            staged: v2::MinaBaseStagedLedgerHashStableV1::zero(
                hash(4),
                v2::PendingCoinbaseHash::from_fp(5u64.into()),
            ),
        }
    }

    /// Blocks at heights 100 to 106, each the next one's parent.
    fn chain() -> Vec<ArcBlockWithHash> {
        synthetic_chain(v2::StateHash::from_fp(9u64.into()), 7, &ledgers())
    }

    fn applied(blocks: &[ArcBlockWithHash]) -> Vec<AppliedBlock> {
        blocks
            .iter()
            .map(|block| AppliedBlock {
                block: block.clone(),
                just_emitted_a_proof: false,
            })
            .collect()
    }

    fn genesis() -> ArcBlockWithHash {
        let genesis = block_after(v2::StateHash::from_fp(8u64.into()), 1, 0, &ledgers());
        assert!(genesis.is_genesis());
        genesis
    }

    /// A block after `parent` at `height`, other than the chain's (another
    /// slot): a sibling fork's.
    fn fork_after(parent: &ArcBlockWithHash, height: u32) -> ArcBlockWithHash {
        block_after(
            parent.hash().clone(),
            height,
            ROOT_SLOT + 2 * (height - 100) + 1,
            &ledgers(),
        )
    }

    /// The phase of a node whose sync is `Synced`, with `ready_peers` ready
    /// peers whose last tips are `peer_tips`, having heard a best tip since
    /// the start or not.
    fn phase(
        best_chain: &[AppliedBlock],
        ready_peers: usize,
        heard: bool,
        peer_tips: &[&ArcBlockWithHash],
        best_candidate: Option<&ArcBlockWithHash>,
    ) -> SyncPhase {
        reported_sync_phase(
            SyncPhase::Synced,
            best_chain,
            ready_peers,
            heard,
            peer_tips.iter().copied(),
            best_candidate,
            redux::Timestamp::ZERO,
        )
    }

    #[test]
    fn without_a_ready_peer_it_is_connecting() {
        let blocks = chain();
        let genesis = genesis();
        assert_eq!(
            phase(&applied(&blocks[..4]), 0, true, &[], None),
            SyncPhase::Connecting
        );
        assert_eq!(
            phase(&applied(&[genesis]), 0, false, &[], None),
            SyncPhase::Connecting
        );
    }

    /// The start of every run: the sync is `Synced` on the genesis block
    /// (`GenesisInject`) before any peer has sent a best tip.
    #[test]
    fn genesis_before_any_best_tip_is_listening() {
        let ours = applied(&[genesis()]);
        assert_eq!(phase(&ours, 3, false, &[], None), SyncPhase::Listening);
    }

    /// After a restore from the frontier store: the stored tip (#103 here),
    /// committed before any peer has sent a best tip.
    #[test]
    fn a_restored_tip_before_any_best_tip_is_listening() {
        let blocks = chain();
        let ours = applied(&blocks[..4]);
        assert_eq!(phase(&ours, 2, false, &[], None), SyncPhase::Listening);
        // Once a peer has, the tip counts, as OCaml's does once online.
        assert_eq!(
            phase(&ours, 2, true, &[&blocks[3]], None),
            SyncPhase::Synced
        );
    }

    #[test]
    fn genesis_with_a_peer_beyond_it_is_bootstrapping() {
        let genesis = genesis();
        let ours = applied(std::slice::from_ref(&genesis));
        let network_tip = chain().pop().unwrap();
        assert_eq!(
            phase(&ours, 3, true, &[&genesis, &network_tip], None),
            SyncPhase::Bootstrap
        );
        // A verified block, its peer gone: the sync bootstraps to it.
        assert_eq!(
            phase(&ours, 3, true, &[&genesis], Some(&network_tip)),
            SyncPhase::Bootstrap
        );
        // Peers that reconnected and haven't sent their tip yet say nothing
        // about where the network is.
        assert_eq!(phase(&ours, 3, true, &[], None), SyncPhase::Listening);
    }

    /// A network at its genesis block: every peer's tip is the genesis block.
    #[test]
    fn genesis_when_every_peer_is_at_genesis_is_synced() {
        let genesis = genesis();
        let ours = applied(std::slice::from_ref(&genesis));
        assert_eq!(
            phase(&ours, 2, true, &[&genesis, &genesis], None),
            SyncPhase::Synced
        );
    }

    /// The only producer of a new network: its peers stay at genesis while
    /// it makes the blocks.
    #[test]
    fn a_sole_producer_with_peers_at_genesis_is_synced() {
        let genesis = genesis();
        let blocks = chain();
        let ours = applied(&blocks[..4]);
        assert_eq!(
            phase(&ours, 2, true, &[&genesis, &genesis], None),
            SyncPhase::Synced
        );
    }

    /// No peer's last tip is on our best chain: behind our root, or a block
    /// we don't have. The node verified and applied its tip; unverified
    /// claims don't keep it out of `Synced`.
    #[test]
    fn peer_tips_off_our_chain_are_synced() {
        let blocks = chain();
        let ours = applied(&blocks[2..5]);
        let behind_root = &blocks[0];
        let elsewhere = fork_after(&blocks[6], 107);
        assert_eq!(
            phase(&ours, 2, true, &[behind_root, &elsewhere], None),
            SyncPhase::Synced
        );
    }

    /// Peers on a sibling fork of our tip, for a moment.
    #[test]
    fn peers_on_a_sibling_fork_are_synced() {
        let blocks = chain();
        let ours = applied(&blocks[..4]);
        let sibling = fork_after(&blocks[2], 103);
        assert_ne!(sibling.hash(), blocks[3].hash());
        assert_eq!(
            phase(&ours, 3, true, &[&sibling, &sibling], None),
            SyncPhase::Synced
        );
    }

    /// A verified candidate better than our tip: the sync is about to catch
    /// up to it (or failed to, and the node is behind).
    #[test]
    fn a_better_verified_candidate_is_catching_up() {
        let blocks = chain();
        let ours = applied(&blocks[..4]);
        let agree = [&blocks[3]];
        assert_eq!(
            phase(&ours, 1, true, &agree, Some(&blocks[4])),
            SyncPhase::Catchup
        );
        // Not one at our tip, nor a worse one.
        assert_eq!(
            phase(&ours, 1, true, &agree, Some(&blocks[3])),
            SyncPhase::Synced
        );
        assert_eq!(
            phase(&ours, 1, true, &agree, Some(&blocks[2])),
            SyncPhase::Synced
        );
        // A peer claiming a better tip whose proof isn't verified yet is not
        // enough.
        assert_eq!(
            phase(&ours, 1, true, &[&blocks[6]], None),
            SyncPhase::Synced
        );
    }

    /// While the sync runs, its own phase is the answer, peers or not.
    #[test]
    fn a_syncing_node_reports_its_sync() {
        let blocks = chain();
        let ours = applied(&blocks[..1]);
        for internal in [SyncPhase::Bootstrap, SyncPhase::Catchup] {
            assert_eq!(
                reported_sync_phase(internal, &ours, 0, false, [], None, redux::Timestamp::ZERO),
                internal
            );
            assert_eq!(
                reported_sync_phase(
                    internal,
                    &[],
                    5,
                    true,
                    [&blocks[6]],
                    None,
                    redux::Timestamp::ZERO
                ),
                internal
            );
        }
    }
}
