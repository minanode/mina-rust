use std::collections::BTreeMap;

use ledger::transaction_pool::diff::BestTipDiff;
use mina_core::{
    block::{AppliedBlock, ArcBlockWithHash},
    bug_condition,
};
use mina_p2p_messages::v2::{
    LedgerHash, MinaStateProtocolStateBodyValueStableV2, MinaStateProtocolStateValueStableV2,
    StateHash, TransactionHash,
};
use serde::{Deserialize, Serialize};

use super::{
    candidate::TransitionFrontierCandidatesState, genesis::TransitionFrontierGenesisState,
    persist::TransitionFrontierPersistState, sync::TransitionFrontierSyncState,
    TransitionFrontierConfig,
};

/// Local proof read failures per fetched block that don't back its peer off.
pub const LOCAL_BLOCK_CHECK_RETRIES: u8 = 3;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TransitionFrontierState {
    pub config: TransitionFrontierConfig,
    /// Genesis block generation/proving state
    pub genesis: TransitionFrontierGenesisState,
    /// Current best known chain, from root of the transition frontier to best tip
    pub best_chain: Vec<AppliedBlock>,
    #[serde(skip)]
    pub root_history: super::root_history::RootHistory,
    /// Needed protocol states for applying transactions in the root
    /// scan state that we don't have in the `best_chain` list.
    pub needed_protocol_states: BTreeMap<StateHash, MinaStateProtocolStateValueStableV2>,
    pub candidates: TransitionFrontierCandidatesState,
    /// Transition frontier synchronization state
    pub sync: TransitionFrontierSyncState,
    /// Block-fetch failures are peer-wide, across hashes and sync targets.
    #[serde(default)]
    pub block_peer_query_errors: BTreeMap<crate::p2p::PeerId, redux::Timestamp>,
    /// Local proof read failures while checking a fetched block's body, per
    /// block (BP-SAFE). See [`Self::block_local_check_failed`].
    #[serde(default)]
    pub block_local_check_errors: BTreeMap<StateHash, u8>,

    /// Blocks which had valid proof but failed block application or
    /// other validations after it reached transition frontier.
    pub blacklist: BTreeMap<StateHash, u32>,
    /// The diff of `Self::best_chain` with the previous one
    pub chain_diff: Option<BestTipDiff>,
    /// Archive mode enabled
    pub archive_enabled: bool,
    /// The frontier store (R20), off unless configured.
    pub persist: TransitionFrontierPersistState,
    /// The genesis ledgers, when their load was deferred (I5d2).
    #[serde(default)]
    pub genesis_ledgers: GenesisLedgersState,
}

/// The genesis ledgers of a node that took its genesis from the genesis
/// cache (I5d2, `TransitionFrontierConfig::genesis_ledgers_deferred`).
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub enum GenesisLedgersState {
    /// Loaded with the genesis config, or loaded later (the usual case).
    #[default]
    Loaded,
    /// Not loaded: a restore from the frontier store replaces them. If the
    /// node bootstraps instead, they are loaded first, so that the bootstrap
    /// is the one a node started from the full config makes.
    Deferred,
    /// Being loaded, on a thread of the service.
    Pending { time: redux::Timestamp },
    /// The load failed: keep all ledger-dependent work held until shutdown.
    Error { error: String },
}

impl TransitionFrontierState {
    pub fn new(config: TransitionFrontierConfig, archive_enabled: bool) -> Self {
        let genesis_ledgers = if config.genesis_ledgers_deferred.is_some() {
            GenesisLedgersState::Deferred
        } else {
            GenesisLedgersState::Loaded
        };
        Self {
            persist: TransitionFrontierPersistState::new(&config.persist),
            config,
            genesis: TransitionFrontierGenesisState::Idle,
            candidates: TransitionFrontierCandidatesState::new(),
            best_chain: Vec::with_capacity(290),
            root_history: Default::default(),
            needed_protocol_states: Default::default(),
            sync: TransitionFrontierSyncState::Idle,
            block_peer_query_errors: Default::default(),
            block_local_check_errors: Default::default(),
            blacklist: Default::default(),
            chain_diff: None,
            archive_enabled,
            genesis_ledgers,
        }
    }

    pub fn best_tip(&self) -> Option<&ArcBlockWithHash> {
        self.best_chain.last().map(|b| &b.block)
    }

    pub fn block_peer_query_ready(&self, peer: &crate::p2p::PeerId, now: redux::Timestamp) -> bool {
        self.block_peer_query_errors.get(peer).is_none_or(|last| {
            now.checked_sub(*last).is_some_and(|elapsed| {
                elapsed >= super::sync::ledger::snarked::LEDGER_QUERY_RETRY_DELAY
            })
        })
    }

    pub fn block_peer_query_failed(&mut self, peer: crate::p2p::PeerId, now: redux::Timestamp) {
        let delay = super::sync::ledger::snarked::LEDGER_QUERY_RETRY_DELAY;
        self.block_peer_query_errors
            .retain(|_, last| now.checked_sub(*last).is_none_or(|elapsed| elapsed < delay));
        self.block_peer_query_errors.insert(peer, now);
        // Bound history even if identities churn faster than the retry delay.
        if self.block_peer_query_errors.len() > 1024 {
            if let Some(oldest) = self
                .block_peer_query_errors
                .iter()
                .min_by_key(|(_, last)| **last)
                .map(|(peer, _)| *peer)
            {
                self.block_peer_query_errors.remove(&oldest);
            }
        }
    }

    /// Counts a local proof read failure checking a fetched block. The
    /// first [`LOCAL_BLOCK_CHECK_RETRIES`] for a block don't back its peer
    /// off, so a sole peer is asked again at once; after them a failure
    /// counts as the peer's, so the fetch can't loop without a delay.
    /// Returns whether the peer is spared.
    pub fn block_local_check_failed(&mut self, hash: &StateHash) -> bool {
        // Bound the map: blocks that are never fetched again stay in it.
        if self.block_local_check_errors.len() >= 64
            && !self.block_local_check_errors.contains_key(hash)
        {
            self.block_local_check_errors.clear();
        }
        let count = self
            .block_local_check_errors
            .entry(hash.clone())
            .or_default();
        *count = count.saturating_add(1);
        *count <= LOCAL_BLOCK_CHECK_RETRIES
    }

    pub fn root(&self) -> Option<&ArcBlockWithHash> {
        self.best_chain.first().map(|b| &b.block)
    }

    /// Whether the best tip can be served with a proof, as OCaml's
    /// `Best_tip_prover.prove` decides: only if the chain from the root to the
    /// best tip is k + 1 blocks, or the root is the genesis block. OCaml
    /// rejects a shorter proof and bans the sender, so a node that restored a
    /// shorter chain from its frontier store answers nothing until it has k + 1
    /// blocks (R20 D5).
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/best_tip_prover/best_tip_prover.ml#L47-L62>
    pub fn can_prove_best_tip(&self) -> bool {
        let (Some(root), Some(tip)) = (self.root(), self.best_tip()) else {
            return false;
        };
        let k = tip.constants().k.as_u32() as usize;
        root.is_genesis() || self.best_chain.len() == k.saturating_add(1)
    }

    /// Whether the sync must not take a new target: a restore from the
    /// frontier store is still to be decided, or replaying (R20 D5, I5d1).
    /// Whether the sync must not take a new target yet: while a restore
    /// from the frontier store is to be decided or replaying (R20 D5), and,
    /// for a node that deferred its genesis ledgers (I5d2), until they are
    /// loaded if the restore didn't happen.
    pub fn persist_holds_sync(&self) -> bool {
        self.persist.holds_sync(self.config.persist.restore) || self.genesis_ledgers_hold_sync()
    }

    /// Whether the deferred genesis ledgers are to be loaded before the node
    /// bootstraps (I5d2): the restore didn't replace them.
    pub fn genesis_ledgers_needed(&self) -> bool {
        matches!(self.genesis_ledgers, GenesisLedgersState::Deferred)
            && !self.persist.holds_sync(self.config.persist.restore)
            && !self.persist.is_restored()
    }

    /// A cached genesis best tip does not imply its ledger is available.
    /// A successful restore supplies the frontier ledgers instead; otherwise
    /// the full genesis load must succeed before reads or production resume.
    pub fn genesis_ledgers_ready(&self) -> bool {
        matches!(self.genesis_ledgers, GenesisLedgersState::Loaded) || self.persist.is_restored()
    }

    fn genesis_ledgers_hold_sync(&self) -> bool {
        !self.genesis_ledgers_ready()
    }

    /// The snarked ledger to offer first, as the closest to copy, when a best
    /// tip update may change the target of the root ledger sync. While a sync
    /// is in progress, that is the sync target root's snarked ledger: synced
    /// or being synced, and one or a few blocks from the new root. Otherwise
    /// it is the frontier root's, which during a bootstrap is the genesis
    /// block's: absent with a genesis config without ledgers, and the fork's
    /// ledger with one, both far from the target.
    pub fn previous_root_snarked_ledger_hash(&self) -> Option<LedgerHash> {
        self.sync
            .root_block()
            .or_else(|| self.root())
            .map(|block| block.snarked_ledger_hash().clone())
    }

    pub fn best_tip_breadcrumb(&self) -> Option<&AppliedBlock> {
        self.best_chain.last()
    }

    pub fn root_breadcrumb(&self) -> Option<&AppliedBlock> {
        self.best_chain.first()
    }

    /// Whether the node has no usable frontier yet, so must not judge
    /// transactions: OCaml answers "Daemon is bootstrapping" then, re-bootstraps
    /// included, and its pool refuses gossip.
    ///
    /// True when there is no best tip; when the sync is (re)bootstrapping its
    /// ledgers, during which `best_chain` still holds the old, stale chain; or
    /// when the best tip is a genesis block more than `k` slots old.
    /// `GenesisInject` marks the sync `Synced` with the genesis block alone, so
    /// until the first real sync the pool would otherwise work against the
    /// genesis ledger. A fresh private network whose genesis is recent can
    /// take transactions on its genesis block.
    ///
    /// Catch-up (fetching and applying blocks) does not count: it runs for
    /// every new block.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L392-L395>
    pub fn is_bootstrapping(&self, now: redux::Timestamp) -> bool {
        if !self.genesis_ledgers_ready() {
            return true;
        }
        let best_tip = self.best_tip().map(|tip| {
            let k = u64::from(tip.constants().k.as_u32());
            let window_ms = k.saturating_mul(
                mina_core::constants::constraint_constants().block_window_duration_ms,
            );
            (
                tip.is_genesis(),
                tip.timestamp() + std::time::Duration::from_millis(window_ms),
            )
        });
        bootstrapping(
            best_tip,
            matches!(self.sync.sync_phase(), super::sync::SyncPhase::Bootstrap),
            now,
        )
    }

    /// FIXME
    /// Note(adonagy): This can be expensive, keep a map with all the tx hashis in the best chain
    pub fn contains_transaction(&self, hash: &TransactionHash) -> bool {
        self.best_chain.iter().any(|block| {
            block
                .body()
                .transactions()
                .any(|transaction| transaction.hash().as_ref().ok() == Some(hash))
        })
    }

    /// Looks up state body by state hash.
    pub fn get_state_body(
        &self,
        hash: &StateHash,
    ) -> Option<&MinaStateProtocolStateBodyValueStableV2> {
        self.best_chain
            .iter()
            .find_map(|block| {
                if block.hash() == hash {
                    Some(&block.header().protocol_state.body)
                } else {
                    None
                }
            })
            .or_else(|| {
                self.needed_protocol_states
                    .iter()
                    .find_map(|(block_hash, state)| {
                        if block_hash == hash {
                            Some(&state.body)
                        } else {
                            None
                        }
                    })
            })
    }

    /// Create a diff between the old best chain and the new one
    /// This is used to update the transaction pool
    pub fn maybe_make_chain_diff(&self, new_chain: &[AppliedBlock]) -> Option<BestTipDiff> {
        let old_chain = self.best_chain.as_slice();
        let new_root = new_chain.first();

        if old_chain.last() == new_chain.last() {
            // Both chains are equal
            return None;
        }

        // Look for the new root in the old chain, get its index
        let new_chain_start_at = match new_root {
            None => None,
            Some(new_root) => old_chain
                .iter()
                .enumerate()
                .rev()
                .find(|(_index, block)| *block == new_root),
        };

        let (diff_old_chain, diff_new_chain) = match new_chain_start_at {
            None => {
                // The new chain has a root not present in the old chain,
                // so the diff is the 2 wholes chains
                (old_chain, new_chain)
            }
            Some((new_chain_start_at, _)) => {
                // `new_chain_start_at` is the index of `new_root` in `old_chain`
                let Some(old_chain_advanced) = &old_chain.get(new_chain_start_at..) else {
                    bug_condition!("old_chain[{}] out of bounds", new_chain_start_at);
                    return None;
                };

                // Common length
                let len = old_chain_advanced.len().min(new_chain.len());

                // Find the first different block, search from the end
                let diff_start_at = old_chain_advanced
                    .get(..len)
                    .unwrap() // can't fail because len is the minimum len
                    .iter()
                    .rev()
                    .zip(
                        new_chain
                            .get(..len)
                            .unwrap() // can't fail because len is the minimum len
                            .iter()
                            .rev(),
                    )
                    .position(|(old_block, new_block)| old_block == new_block)
                    .map(|index| len.saturating_sub(index)) // we started from the end
                    .unwrap(); // Never panics because we know there is the common root block

                let Some(diff_old_chain) = old_chain_advanced.get(diff_start_at..) else {
                    bug_condition!("old_chain[{}] out of bounds", diff_start_at);
                    return None;
                };
                let Some(diff_new_chain) = new_chain.get(diff_start_at..) else {
                    bug_condition!("new_chain[{}] out of bounds", diff_start_at);
                    return None;
                };

                (diff_old_chain, diff_new_chain)
            }
        };

        // Collect commands and convert them to type `WithStatus::<UserCommand>`
        let collect = |chain: &[AppliedBlock]| {
            chain
                .iter()
                .flat_map(|breadcrumb| breadcrumb.commands_iter())
                .filter_map(|cmd| {
                    use ledger::scan_state::transaction_logic::{UserCommand, WithStatus};
                    Some(
                        WithStatus::<UserCommand>::try_from(cmd)
                            .ok()?
                            .into_map(UserCommand::to_valid_unsafe),
                    )
                })
                .collect::<Vec<_>>()
        };

        let removed_commands = collect(diff_old_chain);
        let new_commands = collect(diff_new_chain);

        if removed_commands.is_empty() && new_commands.is_empty() {
            return None;
        }

        Some(BestTipDiff {
            new_commands,
            removed_commands,
            reorg_best_tip: false, // TODO: Unused for now
        })
    }

    pub fn resources_usage(&self) -> serde_json::Value {
        serde_json::json!({
            "best_chain_size": self.best_chain.len(),
            "root_history_size": self.root_history.iter().count(),
            "needed_protocol_states_size": self
                .needed_protocol_states
                .len(),
            "blacklist_size": self.blacklist.len(),
            "diff_tx_size": self
                .chain_diff
                .as_ref()
                // `saturating_add` is not needed here as collection size cannot overflow usize
                // but it makes clippy satisfied
                .map(|d| d.new_commands.len().saturating_add(d.removed_commands.len()))
                .unwrap_or_default()
        })
    }
}

/// The rule of [`TransitionFrontierState::is_bootstrapping`]. `best_tip` is
/// whether the best tip is a genesis block, and until when such a genesis
/// counts as fresh; `syncing_ledgers` whether the sync is in its bootstrap
/// (ledger) phase.
fn bootstrapping(
    best_tip: Option<(bool, redux::Timestamp)>,
    syncing_ledgers: bool,
    now: redux::Timestamp,
) -> bool {
    match best_tip {
        None => true,
        Some(_) if syncing_ledgers => true,
        Some((true, fresh_until)) => fresh_until < now,
        Some((false, _)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::bootstrapping;
    use redux::Timestamp;

    fn at(secs: u64) -> Timestamp {
        Timestamp::new(secs * 1_000_000_000)
    }

    #[test]
    fn no_best_tip_is_bootstrapping() {
        assert!(bootstrapping(None, false, at(10)));
        assert!(bootstrapping(None, true, at(10)));
    }

    /// Mainnet and devnet, before the first sync: the genesis block is the
    /// only block, and months old.
    #[test]
    fn an_old_genesis_tip_is_bootstrapping() {
        assert!(bootstrapping(Some((true, at(100))), false, at(101)));
    }

    /// A fresh private network can take transactions on its genesis block.
    #[test]
    fn a_fresh_genesis_tip_is_not() {
        assert!(!bootstrapping(Some((true, at(100))), false, at(100)));
        assert!(!bootstrapping(Some((true, at(100))), false, at(50)));
    }

    /// A re-bootstrap: `best_chain` still holds the old chain while the sync
    /// fetches ledgers, so the tip is stale.
    #[test]
    fn a_ledger_sync_is_bootstrapping_whatever_the_tip() {
        assert!(bootstrapping(Some((false, at(0))), true, at(10)));
        assert!(bootstrapping(Some((true, at(100))), true, at(10)));
    }

    #[test]
    fn a_synced_tip_is_not() {
        assert!(!bootstrapping(Some((false, at(0))), false, at(10)));
    }
}
