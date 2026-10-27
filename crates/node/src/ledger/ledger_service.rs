use super::{
    ledger_empty_hash_at_depth,
    read::{LedgerReadId, LedgerReadRequest, LedgerReadResponse},
    write::{CommitResult, LedgerWriteRequest, LedgerWriteResponse, LedgersToKeep},
    LedgerAddress, LedgerEvent, LEDGER_DEPTH,
};
use crate::{
    account::AccountPublicKey,
    block_producer::vrf_evaluator::DelegatorTable,
    block_producer_effectful::StagedLedgerDiffCreateOutput,
    ledger::{
        ledger_manager::{LedgerManager, LedgerRequest},
        write::{BlockApplyResult, BlockApplyResultArchive},
    },
    p2p::channels::rpc::StagedLedgerAuxAndPendingCoinbases,
    rpc::{
        DelegatorsQuery, HardForkExportRequest, HardForkLedger, LedgerExportBestTip,
        LedgerExportKind, LedgerExportRequest, LightAccountRead, LightMerklePathElement,
        RpcHardForkExport, RpcLedgerExport, RpcLightAccount, RpcLightAccountEntry,
        RpcLightAccountResponse, RpcLightError, RpcScanStateSummaryBlockTransaction,
        RpcScanStateSummaryScanStateJob, RpcScanStateSummaryScanStateJobKind,
    },
    transition_frontier::{
        genesis::empty_pending_coinbase_hash,
        sync::{
            ledger::staged::StagedLedgerAuxAndPendingCoinbasesValid,
            TransitionFrontierRootSnarkedLedgerUpdates,
        },
    },
};
use ledger::{
    scan_state::{
        currency::Slot,
        scan_state::{AvailableJobMessage, JobValueBase, JobValueMerge, JobValueWithIndex, Pass},
        transaction_logic::{
            local_state::LocalState,
            protocol_state::{protocol_state_view, ProtocolStateView},
            transaction_partially_applied::TransactionPartiallyApplied,
            valid, Transaction,
        },
    },
    sparse_ledger::SparseLedger,
    staged_ledger::{
        diff::Diff,
        staged_ledger::{SkipVerification, StagedLedger},
        validate_block::block_body_hash,
    },
    verifier::Verifier,
    Account, AccountId, BaseLedger, Database, Mask, TokenId,
};
use mina_core::{
    block::{AppliedBlock, ArcBlockWithHash},
    bug_condition,
    constants::constraint_constants,
    snark::{Snark, SnarkJobId},
};
use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    bigint::InvalidBigInt,
    binprot::BinProtRead,
    list::List,
    v2::{
        self, DataHashLibStateHashStableV1, LedgerHash, MinaBaseLedgerHash0StableV1,
        MinaBasePendingCoinbaseStableV2, MinaBasePendingCoinbaseWitnessStableV2,
        MinaBaseStagedLedgerHashStableV1,
        MinaStateBlockchainStateValueStableV2LedgerProofStatement,
        MinaStateProtocolStateValueStableV2, MinaTransactionTransactionStableV2, NonZeroCurvePoint,
        StateHash,
    },
};
use mina_signer::CompressedPubKey;
use rayon::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

// A child module, so that it can use `LedgerCtx`'s ledgers directly.
#[path = "ledger_persist.rs"]
mod ledger_persist;
pub use ledger_persist::PersistCheckpointRequest;

#[cfg(test)]
#[path = "ledger_reclamation_tests.rs"]
mod ledger_reclamation_tests;

fn merkle_root(mask: &mut Mask) -> LedgerHash {
    MinaBaseLedgerHash0StableV1(mask.merkle_root().into()).into()
}

/// OCaml folds the ascending State_hash.Set list with List.cons, so the
/// protocol states in a staged-ledger answer are in descending field order.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/sync_handler/sync_handler.ml#L120-L135>
fn ordered_required_protocol_states(
    hashes: impl IntoIterator<Item = Fp>,
    states: &BTreeMap<StateHash, MinaStateProtocolStateValueStableV2>,
) -> Option<List<MinaStateProtocolStateValueStableV2>> {
    let hashes: BTreeSet<StateHash> = hashes
        .into_iter()
        .map(|fp| DataHashLibStateHashStableV1(fp.into()).into())
        .collect();
    hashes
        .iter()
        .rev()
        .map(|hash| states.get(hash).cloned())
        .collect()
}

/// Count serialized bytes without allocating an answer buffer, stopping at
/// the serving ceiling. Unlike BinProtSize this also propagates codec errors.
struct LedgerAnswerSize(usize);
impl std::io::Write for LedgerAnswerSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .filter(|size| *size <= crate::p2p::channels::rpc::MAX_SERVED_LEDGER_ANSWER_BYTES)
            .ok_or_else(|| std::io::Error::other("Sync ledger answer exceeds byte limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Hash of a subtree of `height` whose leaves are `leaf_hashes`, left to
/// right, followed by empty accounts.
fn subtree_hash(mut level: Vec<Fp>, height: usize) -> Fp {
    use ledger::{TreeVersion, V2};
    let mut empty = Account::empty().hash();
    for height in 0..height {
        level = level
            .chunks(2)
            .map(|pair| V2::hash_node(height, pair[0], pair.get(1).copied().unwrap_or(empty)))
            .collect();
        empty = V2::hash_node(height, empty, empty);
    }
    level.first().copied().unwrap_or(empty)
}

fn error_to_string(e: InvalidBigInt) -> String {
    format!("{:?}", e)
}

/// The producer and coverage rehearsal must use the same full-statement lookup.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool.ml#L277-L277>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/staged_ledger/staged_ledger.ml#L691-L705>
pub(crate) fn completed_work_for_statement(
    completed_snarks: &BTreeMap<SnarkJobId, Snark>,
    statement: &ledger::scan_state::scan_state::transaction_snark::work::Statement,
) -> Option<ledger::scan_state::scan_state::transaction_snark::work::Checked> {
    let work = completed_snarks.get(&SnarkJobId::from(statement))?;
    let checked: ledger::scan_state::scan_state::transaction_snark::work::Checked =
        work.try_into().ok()?;
    (checked.statement() == *statement).then_some(checked)
}

/// Indexing `StagedLedger` both by their "merkle root hash" and their "staged ledger hash"
#[derive(Default)]
struct StagedLedgersStorage {
    /// 1 merkle root hash can refer to 1 or more `StagedLedger`
    by_merkle_root_hash: BTreeMap<LedgerHash, Vec<Arc<MinaBaseStagedLedgerHashStableV1>>>,
    staged_ledgers: BTreeMap<Arc<MinaBaseStagedLedgerHashStableV1>, StagedLedger>,
}

impl StagedLedgersStorage {
    /// Slow, it will recompute the full staged ledger hash
    /// Prefer `Self::insert` when you have the "staged ledger hash" around
    fn insert_by_recomputing_hash(&mut self, mut staged_ledger: StagedLedger) {
        let staged_ledger_hash: MinaBaseStagedLedgerHashStableV1 = (&staged_ledger.hash()).into();
        self.insert(Arc::new(staged_ledger_hash), staged_ledger);
    }

    fn insert(
        &mut self,
        staged_ledger_hash: Arc<MinaBaseStagedLedgerHashStableV1>,
        staged_ledger: StagedLedger,
    ) {
        let merkle_root_hash: LedgerHash = merkle_root(&mut staged_ledger.ledger());
        let hashes = self
            .by_merkle_root_hash
            .entry(merkle_root_hash)
            .or_insert_with(|| Vec::with_capacity(1));
        if !hashes.contains(&staged_ledger_hash) {
            hashes.push(staged_ledger_hash.clone());
        }
        self.staged_ledgers
            .insert(staged_ledger_hash, staged_ledger);
    }

    fn get_mask(&self, root_hash: &LedgerHash) -> Option<Mask> {
        let staged_ledger_hashes: &Vec<_> = self.by_merkle_root_hash.get(root_hash)?;
        // Note: there can be multiple `staged_ledger_hashes`, but they all have the same
        // mask, so we just take the 1st one
        self.staged_ledgers
            .get(staged_ledger_hashes.first()?)
            .map(|staged_ledger| staged_ledger.ledger())
    }

    fn get(&self, staged_ledger_hash: &MinaBaseStagedLedgerHashStableV1) -> Option<&StagedLedger> {
        self.staged_ledgers.get(staged_ledger_hash)
    }

    fn get_mut(
        &mut self,
        staged_ledger_hash: &MinaBaseStagedLedgerHashStableV1,
    ) -> Option<&mut StagedLedger> {
        self.staged_ledgers.get_mut(staged_ledger_hash)
    }

    fn retain<F>(&mut self, fun: F)
    where
        F: Fn(&MinaBaseStagedLedgerHashStableV1) -> bool,
    {
        self.by_merkle_root_hash.retain(|_, staged_ledger_hashes| {
            staged_ledger_hashes.retain(|hash| {
                if fun(hash) {
                    return true;
                }
                self.staged_ledgers.remove(hash);
                false
            });
            !staged_ledger_hashes.is_empty()
        });
    }

    fn extend<I>(&mut self, iterator: I)
    where
        I: IntoIterator<Item = (Arc<MinaBaseStagedLedgerHashStableV1>, StagedLedger)>,
    {
        for (hash, staged_ledger) in iterator.into_iter() {
            self.insert(hash, staged_ledger);
        }
    }

    fn take(&mut self) -> BTreeMap<Arc<MinaBaseStagedLedgerHashStableV1>, StagedLedger> {
        let Self {
            by_merkle_root_hash,
            staged_ledgers,
        } = self;

        let _ = std::mem::take(by_merkle_root_hash);
        std::mem::take(staged_ledgers)
    }
}

#[derive(Default)]
pub struct LedgerCtx {
    snarked_ledgers: BTreeMap<LedgerHash, Mask>,
    /// Additional snarked ledgers specified at startup (loaded from disk)
    additional_snarked_ledgers: BTreeMap<LedgerHash, Mask>,
    staged_ledgers: StagedLedgersStorage,
    root_history: BTreeMap<StateHash, HistoricalRoot>,
    root_history_protocol_states: BTreeMap<StateHash, MinaStateProtocolStateValueStableV2>,
    sync: LedgerSyncState,
    /// Returns more data on block application necessary for archive node
    archive_mode: bool,
    event_sender: Option<mina_core::channels::mpsc::UnboundedSender<crate::event_source::Event>>,
    /// The frontier store's writer and the ledgers pinned for it.
    persist: ledger_persist::LedgerPersist,
    /// Complete token-owner index for one ledger snapshot, built lazily for
    /// private RPCs. Both hits and misses reuse it; a new hash replaces it.
    token_owners: Option<(LedgerHash, BTreeMap<TokenId, AccountId>)>,
    /// The block producer's delegators in the staking ledgers, for
    /// `lastEpochDelegators`.
    epoch_delegatees: super::epoch_delegatees::EpochDelegatees,
    /// Where the epoch ledgers' accounts go (PC3a), if not the process's
    /// directory ([`ledger::database::disk::global_dir`]).
    ledger_disk_dir: Option<std::path::PathBuf>,
    /// The delegating accounts of each frozen epoch ledger, for delegator
    /// lookups that read only those accounts (PC3a).
    delegate_index: BTreeMap<LedgerHash, Arc<super::delegate_index::DelegateIndex>>,
    /// Tests: the root snarked ledger's refreeze and detach thresholds,
    /// instead of [`ledger::database::disk::refreeze_threshold`] and
    /// [`root_detach_threshold`].
    #[cfg(test)]
    root_disk_limits: Option<(usize, usize)>,
}

/// Keep scan-state structure with shared transaction/proof Arcs, never a Mask.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_frontier/frontier_base/root_data.ml#L43-L70>
struct HistoricalRoot {
    scan_state: ledger::scan_state::scan_state::ScanState,
    pending_coinbase: MinaBasePendingCoinbaseStableV2,
    ledger_hash: MinaBaseStagedLedgerHashStableV1,
}

#[derive(Default)]
struct LedgerSyncState {
    snarked_ledgers: BTreeMap<LedgerHash, Mask>,
    staged_ledgers: StagedLedgersStorage,
}

impl LedgerCtx {
    pub(super) fn memory_debug(&self) -> serde_json::Value {
        // Attribute shared chunks to the frontier first, then charge history
        // only for additional retained allocations. The sum is the live union.
        let mut seen = BTreeSet::new();
        let staged_tree_bytes = self
            .staged_ledgers
            .staged_ledgers
            .values()
            .map(|ledger| ledger.scan_state().scan_state.tree_heap_bytes(&mut seen))
            .sum::<usize>();
        let history_tree_bytes = self
            .root_history
            .values()
            .map(|root| root.scan_state.scan_state.tree_heap_bytes(&mut seen))
            .sum::<usize>();
        serde_json::json!({
            "snarked_ledgers": self.snarked_ledgers.len(),
            "additional_snarked_ledgers": self.additional_snarked_ledgers.len(),
            "staged_ledgers": self.staged_ledgers.staged_ledgers.len(),
            "staged_scan_tree_bytes": staged_tree_bytes,
            "root_history": self.root_history.len(),
            "root_history_protocol_states": self.root_history_protocol_states.len(),
            "root_history_scan_tree_bytes": history_tree_bytes,
            "root_history_protocol_states_json_bytes": crate::stats::serialized_size(&self.root_history_protocol_states),
            "sync_snarked_ledgers": self.sync.snarked_ledgers.len(),
            "sync_staged_ledgers": self.sync.staged_ledgers.staged_ledgers.len(),
            "token_owner_cache_entries": self.token_owners.as_ref().map(|(_, owners)| owners.len()).unwrap_or(0),
            "alive_masks": ::ledger::mask::alive_len(),
            "ledger_disk": ::ledger::database::disk::stats(),
            "delegate_index_bytes": self.delegate_index.values().map(|index| index.heap_bytes()).sum::<usize>(),
            "snarked_ledgers_on_disk": self.snarked_ledgers.iter().filter_map(|(hash, mask)| {
                mask.disk_state().map(|overlay| serde_json::json!({
                    "ledger_hash": hash.to_string(),
                    "accounts": mask.num_accounts(),
                    "changed_since_frozen": overlay,
                }))
            }).collect::<Vec<_>>(),
        })
    }

    /// Where the epoch ledgers' accounts go: this context's directory, or the
    /// process's. `None` keeps them in memory, as the web node does.
    fn ledger_disk_dir(&self) -> Option<std::path::PathBuf> {
        self.ledger_disk_dir
            .clone()
            .or_else(|| ::ledger::database::disk::global_dir().map(Path::to_owned))
    }

    #[cfg(test)]
    pub(crate) fn set_ledger_disk_dir(&mut self, dir: std::path::PathBuf) {
        self.ledger_disk_dir = Some(dir);
    }

    /// Moves the accounts of the root's and the best tip's staking and next
    /// epoch ledgers to files (PC3a), as OCaml keeps its epoch ledgers in
    /// RocksDB. Neither changes once its epoch has started; what reads them
    /// (VRF delegator tables, RPC and GraphQL accounts, serving to syncing
    /// peers, the masks above them) reads through the file, and their hash
    /// matrices stay in memory. A ledger that is a mask, not a database of its
    /// own, holds only its changes and stays as it is. A frozen ledger that
    /// has had changes since, as when the next epoch ledger's mask is folded
    /// into the staking ledger it replaces, is written again. A ledger that
    /// can't be written stays in memory.
    ///
    /// The root snarked ledger is left to [`Self::freeze_root_snarked_ledger`]
    /// while it is the root's, even when it is an epoch ledger too: it changes
    /// with every root move, and is written again only once enough has
    /// changed. Once the root has moved on it is a fixed epoch ledger, and its
    /// remaining changes are written here.
    fn freeze_epoch_ledgers(
        &mut self,
        new_root: &ArcBlockWithHash,
        new_best_tip: &ArcBlockWithHash,
    ) {
        let mut hashes = BTreeSet::from([
            new_root.staking_epoch_ledger_hash(),
            new_root.next_epoch_ledger_hash(),
            new_best_tip.staking_epoch_ledger_hash(),
            new_best_tip.next_epoch_ledger_hash(),
        ]);
        hashes.remove(new_root.snarked_ledger_hash());
        self.freeze_snarked_ledgers(hashes);
    }

    /// Moves the root snarked ledger's accounts to a file too (PC3b), as OCaml
    /// keeps the root snarked ledger in RocksDB with the frontier's masks
    /// above it. Every root move commits the new snarked ledger's mask into
    /// it ([`Self::compact_snarked_ledgers`]), which for a frozen database
    /// goes to its overlay; once the overlay holds a sixteenth of the
    /// accounts ([`ledger::database::disk::refreeze_threshold`]) the file is
    /// written again with them, copying the unchanged records as they are.
    ///
    /// After an epoch boundary the root snarked ledger is a mask over the
    /// next epoch ledger, which must not change, so every account the root
    /// changes during the epoch, and the hashes on their paths, would stay
    /// in the mask on the heap. Once it holds a sixty-fourth of the accounts,
    /// it becomes a database of its own instead
    /// ([`Mask::detach_into_root`]): a copy of the next epoch ledger's,
    /// sharing its file until its own first refreeze, with the mask's
    /// changes in its overlay. That costs a second compacted hash matrix
    /// (about 9 MB on devnet, 28 MB on mainnet) and a second file, as OCaml
    /// takes the next epoch ledger as a checkpoint of the root's database.
    ///
    /// Every one of these steps keeps the ledger's contents, so the masks
    /// above see no change and a crash in the middle of one loses nothing:
    /// the files are this process's, never read after a restart, which
    /// restores from the frontier store.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_frontier/full_frontier/full_frontier.ml#L549-L554>
    fn freeze_root_snarked_ledger(&mut self, new_root: &ArcBlockWithHash) {
        self.freeze_root_snarked_ledger_hash(new_root.snarked_ledger_hash());
    }

    /// The thresholds of [`Self::freeze_root_snarked_ledger`] for a ledger of
    /// `num_accounts`: the overlay that makes a refreeze, and the accounts of
    /// a mask that make it a database of its own.
    fn root_disk_limits(&self, num_accounts: usize) -> (usize, usize) {
        #[cfg(test)]
        if let Some(limits) = self.root_disk_limits {
            return limits;
        }
        (
            ::ledger::database::disk::refreeze_threshold(num_accounts as u64),
            root_detach_threshold(num_accounts),
        )
    }

    fn freeze_root_snarked_ledger_hash(&mut self, hash: &LedgerHash) {
        if !::ledger::database::disk::root_on_disk() {
            return;
        }
        let Some(dir) = self.ledger_disk_dir() else {
            return;
        };
        let Some(mask) = self.snarked_ledgers.get(hash).cloned() else {
            return;
        };
        if !mask.is_root() {
            let frozen_parent = mask
                .get_parent()
                .is_some_and(|parent| parent.is_root() && parent.disk_state().is_some());
            let own = mask.num_own_accounts();
            if !frozen_parent || own < self.root_disk_limits(mask.num_accounts()).1 {
                return;
            }
            let started = redux::Instant::now();
            let moved = mask.detach_into_root();
            mina_core::info!(
                mina_core::log::system_time();
                kind = "LedgerService::detach_root_snarked_ledger",
                summary = format!("root snarked ledger {hash} detached from the next epoch ledger"),
                accounts = moved.unwrap_or(0),
                ms = started.elapsed().as_secs_f64() * 1000.0,
            );
            if moved.is_none() {
                return;
            }
        }
        let refreeze_at = self.root_disk_limits(mask.num_accounts()).0;
        if mask
            .disk_state()
            .is_some_and(|changed| changed < refreeze_at)
        {
            return;
        }
        match mask.freeze_to_disk(&dir) {
            Ok(Some(report)) => mina_core::info!(
                mina_core::log::system_time();
                kind = "LedgerService::freeze_root_snarked_ledger",
                summary = format!("root snarked ledger {hash} written to disk"),
                accounts = report.accounts,
                changed = report.changed,
                file_bytes = report.file_bytes,
                index_bytes = report.index_bytes,
                refreeze = report.refreeze,
                ms = report.ms,
            ),
            Ok(None) => {}
            Err(error) => mina_core::warn!(
                mina_core::log::system_time();
                kind = "LedgerFreezeFailed",
                summary = "the root snarked ledger could not be written to disk; it stays as it is",
                ledger = hash.to_string(),
                dir = dir.display().to_string(),
                error = error.to_string(),
            ),
        }
    }

    fn freeze_snarked_ledgers(&mut self, hashes: BTreeSet<&LedgerHash>) {
        let Some(dir) = self.ledger_disk_dir() else {
            return;
        };
        // Databases of their own first: a mask over one is detached once that
        // one is on disk.
        let mut hashes = hashes.into_iter().collect::<Vec<_>>();
        hashes.sort_by_key(|hash| {
            !self
                .snarked_ledgers
                .get(*hash)
                .is_some_and(|mask| mask.is_root())
        });
        for hash in hashes {
            let Some(mask) = self.snarked_ledgers.get(hash) else {
                continue;
            };
            let derived = self.detach_epoch_ledger(hash, mask);
            // Indexed before the move, while the accounts are in memory; a
            // detached ledger's from its parent's index.
            let index = derived.or_else(|| {
                (mask.is_root() && !self.delegate_index.contains_key(hash))
                    .then(|| super::delegate_index::DelegateIndex::build(mask))
                    .flatten()
            });
            // A ledger already on disk is written again only once its overlay
            // holds a sixteenth of its accounts (PC3b's refreeze rule): until
            // then it reads its changes from memory, and a commit doesn't
            // write a file for a few changes (PC3c: no write a block producer
            // could wait behind that PC3b didn't make).
            let refreeze_at = self.root_disk_limits(mask.num_accounts()).0;
            let freeze = if mask
                .disk_state()
                .is_some_and(|changed| changed < refreeze_at)
            {
                Ok(None)
            } else {
                mask.freeze_to_disk(&dir)
            };
            match freeze {
                Ok(Some(report)) => mina_core::info!(
                    mina_core::log::system_time();
                    kind = "LedgerService::freeze_epoch_ledger",
                    summary = format!("epoch ledger {hash} moved to disk"),
                    accounts = report.accounts,
                    file_bytes = report.file_bytes,
                    index_bytes = report.index_bytes,
                    refreeze = report.refreeze,
                    ms = report.ms,
                ),
                Ok(None) => {}
                Err(error) => mina_core::warn!(
                    mina_core::log::system_time();
                    kind = "LedgerFreezeFailed",
                    summary = "an epoch ledger could not be written to disk; it stays in memory",
                    ledger = hash.to_string(),
                    dir = dir.display().to_string(),
                    error = error.to_string(),
                ),
            }
            if let Some(mut index) = index {
                if let Some(overlay) = mask.disk_state() {
                    index.overlay = overlay;
                    self.delegate_index.insert(hash.clone(), Arc::new(index));
                }
            }
        }
        let ledgers = &self.snarked_ledgers;
        self.delegate_index.retain(|hash, index| {
            ledgers
                .get(hash)
                .is_some_and(|mask| mask.disk_state() == Some(index.overlay))
        });
    }

    /// Makes an epoch ledger that is a mask over a database on disk a database
    /// of its own (PC3c), a copy of its parent's sharing its file, with the
    /// mask's changes in the overlay, which the freeze that follows writes.
    ///
    /// A bootstrap syncs the next epoch ledger as a mask over the staking
    /// ledger and the root's over that ([`Self::copy_for_sync`]), and a root
    /// that doesn't detach during an epoch (PC3b) becomes the next epoch ledger
    /// as a mask. Left a mask, an epoch ledger would keep its changes and the
    /// hashes on their paths, 3 to 4 KB per account, on the heap, scan the
    /// whole file for delegators, and keep the root above it a mask too: PC3b
    /// detaches the root only from a database. Detached it costs a second
    /// compacted hash matrix (about 9 MB on devnet) and a file, once.
    ///
    /// Returns the detached ledger's delegate index, derived from its parent's
    /// and its own accounts if the parent has one, so that the commit reads no
    /// file to make it.
    fn detach_epoch_ledger(
        &self,
        hash: &LedgerHash,
        mask: &Mask,
    ) -> Option<super::delegate_index::DelegateIndex> {
        let parent = mask
            .get_parent()
            .filter(|parent| parent.is_root() && parent.disk_state().is_some())?;
        if mask.is_root() {
            return None;
        }
        let parent_index = self
            .delegate_index
            .get(&merkle_root(&mut parent.clone()))
            .filter(|index| parent.disk_state() == Some(index.overlay))
            .cloned();
        let changed = parent_index.as_ref().map(|_| {
            mask.own_account_indexes()
                .into_iter()
                .filter_map(|index| {
                    let slot = u32::try_from(index.0).ok()?;
                    Some((slot, mask.get_at_index(index)?.delegate))
                })
                .collect::<Vec<_>>()
        });
        let started = redux::Instant::now();
        let moved = mask.detach_into_root();
        ::ledger::database::disk::kill_point("epoch-detach");
        mina_core::info!(
            mina_core::log::system_time();
            kind = "LedgerService::detach_epoch_ledger",
            summary = format!("epoch ledger {hash} detached from the ledger it was a mask over"),
            accounts = moved.unwrap_or(0),
            ms = started.elapsed().as_secs_f64() * 1000.0,
        );
        moved?;
        Some(parent_index?.derive(&changed?))
    }

    /// A ledger to start a sync from, made from `origin`, the ledger
    /// `origin_hash` (`committed` if it is among the committed ledgers).
    ///
    /// A copy, except for a complete sync ledger with nothing over it yet,
    /// when ledgers go to disk (PC3c): then a mask over it, so that a
    /// bootstrap holds one full ledger, the staking ledger, rather than three.
    /// The next epoch ledger syncs as a mask over the staking ledger and the
    /// root's over the next epoch ledger's, each holding only its
    /// differences, as a node that has run through epoch boundaries holds
    /// them. A complete sync ledger doesn't change: syncs write only their
    /// own target, and a retarget replaces the ledger rather than writing
    /// into it. With nothing else over it, the chain stays a line, so the
    /// commit's folds (`compact_snarked_ledgers`) can't change a ledger
    /// under another's mask. The commit then moves the staking ledger to disk
    /// and detaches the masks ([`Self::detach_epoch_ledger`], PC3b's root
    /// detach).
    ///
    /// Any other copy is a database of its own ([`Mask::copy_independent`]).
    /// A plain copy of a mask, such as a retarget's copy of an incomplete
    /// sync ledger, would read through the mask's parent unregistered, and a
    /// commit that drops that parent would fold the copy into it and panic
    /// (PC3c review). So a sync ledger is either a registered mask over a
    /// complete sync ledger or a database of its own.
    fn copy_for_sync(&self, origin: &Mask, origin_hash: &LedgerHash, committed: bool) -> Mask {
        let complete = || LedgerHash::from_fp(origin.clone().merkle_root()) == *origin_hash;
        if !committed && self.ledger_disk_dir().is_some() && !origin.has_children() && complete() {
            let child = origin.make_child();
            ::ledger::database::disk::kill_point("sync-copy");
            child
        } else {
            origin.copy_independent()
        }
    }

    /// The accounts of `mask`, the snarked ledger `ledger_hash`, that delegate
    /// to `delegate` by `delegates`, with their slots, in ledger order: from
    /// the delegate index for a frozen epoch ledger, else by a scan.
    fn delegating_accounts<F>(
        &self,
        ledger_hash: &LedgerHash,
        mask: &Mask,
        delegate: &CompressedPubKey,
        delegates: F,
    ) -> Vec<(ledger::AccountIndex, Account)>
    where
        F: Fn(&Account) -> bool,
    {
        let indexed = self
            .delegate_index
            .get(ledger_hash)
            .filter(|_| self.snarked_ledgers.contains_key(ledger_hash))
            .filter(|index| mask.disk_state() == Some(index.overlay));
        if let Some(index) = indexed {
            return index
                .candidates(mask, delegate)
                .into_iter()
                .filter(|(_, account)| delegates(account))
                .map(|(index, account)| (index, *account))
                .collect();
        }
        let mut accounts = Vec::new();
        mask.iter(|account| {
            if delegates(account) {
                accounts.push(account.clone());
            }
        });
        accounts
            .into_iter()
            .map(|account| (mask.index_of_account(account.id()).unwrap(), account))
            .collect()
    }

    pub fn new_with_additional_snarked_ledgers<P>(path: P) -> Self
    where
        P: AsRef<Path>,
    {
        use std::fs;

        let Ok(dir) = fs::read_dir(path) else {
            return Self::default();
        };

        let additional_snarked_ledgers = dir
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let hash = entry.file_name().to_str()?.parse().ok()?;
                let mut file =
                    std::io::BufReader::with_capacity(1 << 20, fs::File::open(entry.path()).ok()?);

                let _ = Option::<LedgerHash>::binprot_read(&mut file).ok()?;

                let accounts = Vec::<Account>::binprot_read(&mut file).ok()?;
                let mut mask = Mask::new_root(Database::create(35));
                for account in accounts {
                    let account_id = account.id();
                    mask.get_or_create_account(account_id, account).unwrap();
                }
                Some((hash, mask))
            })
            .collect();

        LedgerCtx {
            additional_snarked_ledgers,
            ..Default::default()
        }
    }

    pub fn set_archive_mode(&mut self) {
        self.archive_mode = true;
    }

    // TODO(tizoc): Only used for the current workaround to make staged ledger
    // reconstruction async, can be removed when the ledger services are made async
    pub fn set_event_sender(
        &mut self,
        event_sender: mina_core::channels::mpsc::UnboundedSender<crate::event_source::Event>,
    ) {
        self.event_sender = Some(event_sender);
    }

    pub(super) fn send_event(&self, event: LedgerEvent) {
        if let Some(tx) = self.event_sender.as_ref() {
            let _ = tx.send(event.into());
        }
    }

    pub(super) fn send_write_response(&self, resp: LedgerWriteResponse) {
        self.send_event(LedgerEvent::Write(resp))
    }

    pub(super) fn send_read_response(&self, id: LedgerReadId, resp: LedgerReadResponse) {
        self.send_event(LedgerEvent::Read(id, resp))
    }

    pub fn insert_genesis_ledger(&mut self, mut mask: Mask) {
        let merkle_root_hash = merkle_root(&mut mask);
        let staged_ledger =
            StagedLedger::create_exn(constraint_constants().clone(), mask.copy()).unwrap();
        self.snarked_ledgers.insert(merkle_root_hash.clone(), mask);
        // The genesis ledger is a specific case, some of its hashes are zero
        let staged_ledger_hash =
            MinaBaseStagedLedgerHashStableV1::zero(merkle_root_hash, empty_pending_coinbase_hash());
        self.staged_ledgers
            .insert(Arc::new(staged_ledger_hash), staged_ledger);
    }

    pub fn staged_ledger_reconstruct_result_store(&mut self, ledger: StagedLedger) {
        self.staged_ledgers.insert_by_recomputing_hash(ledger);
    }

    // TODO(adonagy): Uh-oh, clean this up
    pub fn get_accounts_for_rpc(
        &self,
        ledger_hash: LedgerHash,
        requested_public_key: Option<AccountPublicKey>,
    ) -> Vec<Account> {
        if let Some((mask, _)) = self.mask(&ledger_hash) {
            let mut accounts = Vec::new();
            let mut single_account = Vec::new();

            mask.iter(|account| {
                accounts.push(account.clone());
                if let Some(public_key) = requested_public_key.as_ref() {
                    if public_key == &AccountPublicKey::from(account.public_key.clone()) {
                        single_account.push(account.clone());
                    }
                }
            });

            if requested_public_key.is_some() {
                single_account
            } else {
                accounts
            }
        } else {
            vec![]
        }
    }

    // TODO(tizoc): explain when `is_synced` is `true` and when it is `false`. Also use something else than a boolean.
    /// Returns a tuple of `(mask, is_synced)` for a [Mask] with the specified `hash` if it exists or `None` otherwise.
    pub fn mask(&self, hash: &LedgerHash) -> Option<(Mask, bool)> {
        self.snarked_ledgers
            .get(hash)
            .cloned()
            .map(|mask| (mask, true))
            .or_else(|| Some((self.staged_ledgers.get_mask(hash)?, true)))
            .or_else(|| self.sync.mask(hash))
            .or_else(|| {
                self.additional_snarked_ledgers
                    .get(hash)
                    .map(|l| (l.clone(), true))
            })
    }

    /// A complete exact-hash snapshot for network serving. A working sync
    /// mask must not shadow a complete staged or additional ledger. Keep
    /// `mask`'s working-mask preference for local synchronization reads.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/sync_handler/sync_handler.ml#L53-L115>
    fn complete_mask(&self, hash: &LedgerHash) -> Option<Mask> {
        self.snarked_ledgers
            .get(hash)
            .cloned()
            .or_else(|| self.staged_ledgers.get_mask(hash))
            .or_else(|| self.sync.staged_ledgers.get_mask(hash))
            .or_else(|| self.additional_snarked_ledgers.get(hash).cloned())
    }

    /// A ledger being synced, and whether it is a staged one.
    #[cfg(test)]
    pub(crate) fn sync_ledger_for_tests(&self, hash: &LedgerHash) -> Option<(Mask, bool)> {
        self.sync.mask(hash)
    }

    pub fn contains_snarked_ledger(&self, hash: &LedgerHash) -> bool {
        self.snarked_ledgers.contains_key(hash)
    }

    /// Whether a snarked ledger is being synced, or has been and isn't
    /// committed yet.
    pub fn contains_sync_snarked_ledger(&self, hash: &LedgerHash) -> bool {
        self.sync.snarked_ledgers.contains_key(hash)
    }

    /// Returns the mask for a snarked ledger being synchronized or an error if it is not present
    pub fn pending_sync_snarked_ledger_mask(&self, hash: &LedgerHash) -> Result<Mask, String> {
        self.sync.pending_sync_snarked_ledger_mask(hash)
    }

    /// The ledger to start a sync from: the first of `candidates` we have,
    /// committed or among the sync ledgers, and whether it is committed.
    ///
    /// Candidates come closest to the target first. During a bootstrap the
    /// epoch ledgers and the previous root target are synced, or being synced,
    /// but not committed until the bootstrap ends, so they are only among the
    /// sync ledgers. Looking at committed ledgers alone, or before the sync
    /// ledgers, made a root ledger whose target changed start from the genesis
    /// ledger: absent with a genesis config without ledgers, so the ledger
    /// synced from empty (minutes instead of seconds), and with one, the
    /// fork's ledger, far from the target.
    fn sync_copy_origin<'a>(&self, candidates: &'a [LedgerHash]) -> Option<(&'a LedgerHash, bool)> {
        candidates.iter().find_map(|hash| {
            if self.contains_snarked_ledger(hash) {
                Some((hash, true))
            } else if self.contains_sync_snarked_ledger(hash) {
                Some((hash, false))
            } else {
                None
            }
        })
    }

    /// Starts the sync of a snarked ledger from a copy of the first of
    /// `origin_candidates` we have ([`Self::sync_copy_origin`]), or from an
    /// empty ledger if we have none of them. Returns `false` if the target is
    /// already among the sync ledgers and `overwrite` isn't set.
    pub fn copy_snarked_ledger_contents_for_sync(
        &mut self,
        origin_candidates: &[LedgerHash],
        target_snarked_ledger_hash: LedgerHash,
        overwrite: bool,
    ) -> Result<bool, String> {
        if !overwrite
            && self
                .sync
                .snarked_ledgers
                .contains_key(&target_snarked_ledger_hash)
        {
            return Ok(false);
        }

        // The target itself first, if it is committed: a ledger a frontier
        // store loaded (I5d1) or one the node kept. A copy of it syncs in a
        // query or two.
        let origin = match self.snarked_ledgers.get(&target_snarked_ledger_hash) {
            Some(target) => Some((
                target_snarked_ledger_hash.clone(),
                true,
                target.copy_independent(),
            )),
            None => self
                .sync_copy_origin(origin_candidates)
                .and_then(|(hash, committed)| {
                    let ledgers = if committed {
                        &self.snarked_ledgers
                    } else {
                        &self.sync.snarked_ledgers
                    };
                    let origin = ledgers.get(hash)?;
                    Some((
                        hash.clone(),
                        committed,
                        self.copy_for_sync(origin, hash, committed),
                    ))
                }),
        };
        let Some((origin_hash, origin_committed, target)) = origin else {
            // Nothing to start from, e.g. a genesis config without ledgers:
            // the sync starts from an empty ledger and fetches all of it.
            self.sync
                .snarked_ledger_mut(target_snarked_ledger_hash.clone())
                .map_err(|error| error.to_string())?;
            mina_core::warn!(
                mina_core::log::system_time();
                kind = "LedgerService",
                summary = "no ledger to copy for sync, starting from an empty one",
                origin = origin_candidates
                    .first()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                target = target_snarked_ledger_hash.to_string(),
            );
            return Ok(true);
        };

        mina_core::debug!(
            mina_core::log::system_time();
            kind = "LedgerService",
            summary = "copied a ledger to start the sync from",
            origin = origin_hash.to_string(),
            origin_committed = origin_committed,
            target = target_snarked_ledger_hash.to_string(),
        );
        self.sync
            .snarked_ledgers
            .insert(target_snarked_ledger_hash, target);

        Ok(true)
    }

    pub fn compute_snarked_ledger_hashes(
        &mut self,
        snarked_ledger_hash: &LedgerHash,
    ) -> Result<(), String> {
        let origin = self
            .snarked_ledgers
            .get_mut(snarked_ledger_hash)
            .or_else(|| self.sync.snarked_ledgers.get_mut(snarked_ledger_hash))
            .ok_or(format!(
                "Cannot hash non-existing snarked ledger: {}",
                snarked_ledger_hash
            ))?;

        // Our ledger is lazy when it comes to hashing, but retrieving the
        // merkle root hash forces all pending hashes to be computed.
        let _force_hashing = origin.merkle_root();

        Ok(())
    }

    /// Returns a mutable reference to the [StagedLedger] with the specified `hash` if it exists or `None` otherwise.
    fn staged_ledger_mut(
        &mut self,
        hash: &MinaBaseStagedLedgerHashStableV1,
    ) -> Option<&mut StagedLedger> {
        match self.staged_ledgers.get_mut(hash) {
            Some(v) => Some(v),
            None => self.sync.staged_ledger_mut(hash),
        }
    }

    fn recreate_snarked_ledger(
        &mut self,
        root_snarked_ledger_updates: &TransitionFrontierRootSnarkedLedgerUpdates,
        needed_protocol_states: &BTreeMap<StateHash, MinaStateProtocolStateValueStableV2>,
        snarked_ledger_hash: &LedgerHash,
    ) -> Result<(), String> {
        let Some(update) = root_snarked_ledger_updates.get(snarked_ledger_hash) else {
            return Ok(());
        };
        self.recreate_snarked_ledger(
            root_snarked_ledger_updates,
            needed_protocol_states,
            &update.parent,
        )?;

        self.push_snarked_ledger(
            needed_protocol_states,
            &update.parent,
            snarked_ledger_hash,
            &update.staged_ledger_hash,
        )
    }

    fn push_snarked_ledger(
        &mut self,
        protocol_states: &BTreeMap<StateHash, MinaStateProtocolStateValueStableV2>,
        old_root_snarked_ledger_hash: &LedgerHash,
        new_root_snarked_ledger_hash: &LedgerHash,
        new_root_staged_ledger_hash: &MinaBaseStagedLedgerHashStableV1,
    ) -> Result<(), String> {
        mina_core::debug!(mina_core::log::system_time();
            kind = "LedgerService::push_snarked_ledger",
            summary = format!("{old_root_snarked_ledger_hash} -> {new_root_snarked_ledger_hash}"));
        // Steps 4-7 from https://github.com/openmina/mina/blob/bc812dc9b90e05898c0c36ac76ba51ccf6cac137/src/lib/transition_frontier/full_frontier/full_frontier.ml#L354-L392
        // Step 4: create a new temporary mask `mt` with `s` as it's parent
        let root_snarked_ledger = self
            .snarked_ledgers
            .get(old_root_snarked_ledger_hash)
            .or_else(|| self.sync.snarked_ledgers.get(old_root_snarked_ledger_hash))
            .ok_or_else(|| {
                format!(
                    "push_snarked_ledger: could not find old root snarked ledger: {}",
                    old_root_snarked_ledger_hash,
                )
            })?;
        let mut mt = root_snarked_ledger.make_child();

        self.apply_emitted_proof(protocol_states, &mut mt, new_root_staged_ledger_hash)?;

        // Assert that the obtained ledger is the one we expect
        let expected_hash = new_root_snarked_ledger_hash;
        let obtained_hash = LedgerHash::from_fp(mt.merkle_root());

        if expected_hash != &obtained_hash {
            return Err(format!(
                "Expected to obtain snarked root ledger hash {} but got {}",
                expected_hash, obtained_hash
            ));
        }

        self.sync
            .snarked_ledgers
            .insert(new_root_snarked_ledger_hash.clone(), mt);

        Ok(())
    }

    /// Steps 5-6 of moving a snarked ledger on: applies to `mt` the
    /// transactions of the proof the scan state of `staged_ledger_hash` last
    /// emitted.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L704-L747>
    fn apply_emitted_proof(
        &mut self,
        protocol_states: &BTreeMap<StateHash, MinaStateProtocolStateValueStableV2>,
        mt: &mut Mask,
        staged_ledger_hash: &MinaBaseStagedLedgerHashStableV1,
    ) -> Result<(), String> {
        let constraint_constants = &constraint_constants();

        // Step 5: apply any transactions to `mt` that appear in the transition between `s` and `s'`
        let apply_first_pass = |global_slot: Slot,
                                txn_state_view: &ProtocolStateView,
                                ledger: &mut Mask,
                                transaction: &Transaction| {
            ledger::scan_state::transaction_logic::apply_transaction_first_pass(
                constraint_constants,
                global_slot,
                txn_state_view,
                ledger,
                transaction,
            )
        };

        let apply_second_pass = |ledger: &mut Mask, tx: TransactionPartiallyApplied<Mask>| {
            ledger::scan_state::transaction_logic::apply_transaction_second_pass(
                constraint_constants,
                ledger,
                tx,
            )
        };

        let apply_first_pass_sparse_ledger =
            |global_slot: Slot,
             txn_state_view: &ProtocolStateView,
             sparse_ledger: &mut SparseLedger,
             transaction: &Transaction| {
                ledger::scan_state::transaction_logic::apply_transaction_first_pass(
                    constraint_constants,
                    global_slot,
                    txn_state_view,
                    sparse_ledger,
                    transaction,
                )
            };

        let get_protocol_state = |state_hash: Fp| {
            let state_hash = StateHash::from_fp(state_hash);
            if let Some(s) = protocol_states.get(&state_hash) {
                Ok(s.clone())
            } else {
                Err(format!(
                    "Failed to find protocol state for state hash: {}",
                    state_hash
                ))
            }
        };

        let scan_state = self
            .staged_ledger_mut(staged_ledger_hash)
            .ok_or_else(|| {
                format!(
                    "Failed to find staged ledger with hash: {:#?}",
                    staged_ledger_hash
                )
            })?
            .scan_state();

        let Pass::FirstPassLedgerHash(_first_pass_ledger_target) = scan_state
            .get_snarked_ledger_sync(
                mt,
                get_protocol_state,
                apply_first_pass,
                apply_second_pass,
                apply_first_pass_sparse_ledger,
            )?;

        Ok(())
    }

    /// `mina ledger export`: the ledger `request` names, its accounts in
    /// ledger index order, in OCaml's runtime-config JSON.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L680-L760>
    pub fn export_ledger(
        &mut self,
        request: &LedgerExportRequest,
    ) -> Result<RpcLedgerExport, String> {
        let target = &request.ledger_hash;
        let mut temporary = None;
        let result = match self.mask(target) {
            Some((mask, true)) => Ok(mask),
            Some((_, false)) => Err(format!("ledger {target} is still syncing")),
            None => self.rebuild_snarked_ledger(request, &mut temporary),
        }
        .and_then(|mut mask| {
            let ledger_hash = LedgerHash::from_fp(mask.merkle_root());
            if &ledger_hash != target {
                return Err(format!(
                    "ledger {target} has Merkle root {ledger_hash} on this node"
                ));
            }
            let accounts = mask.to_list();
            let mut json = Vec::with_capacity(accounts.len().saturating_mul(1024));
            crate::daemon_json::write_accounts(&mut json, &accounts)
                .map_err(|e| format!("failed to write the accounts: {e}"))?;
            Ok(RpcLedgerExport {
                kind: request.kind,
                ledger_hash,
                best_tip: request.best_tip.clone(),
                num_accounts: accounts.len() as u64,
                accounts_json: String::from_utf8(json)
                    .map_err(|e| format!("accounts are not UTF-8: {e}"))?,
            })
        });
        // The rebuilt masks were only for this export.
        if let Some(first) = temporary {
            first.unregister_mask(ledger::UnregisterBehavior::Recursive);
        }
        result
    }

    /// The next hard fork's ledgers: the fork block's staged ledger and its
    /// staking and next epoch ledgers, found by hash or rebuilt from the best
    /// chain as [`Self::export_ledger`] does, each with its stable and
    /// migrated Merkle roots.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L2786-L2878>
    pub fn export_hard_fork(
        &mut self,
        request: &HardForkExportRequest,
    ) -> Result<RpcHardForkExport, String> {
        let inputs = &request.inputs;
        let block = &inputs.fork_block;
        let mut export = |kind: LedgerExportKind, ledger_hash: &LedgerHash| {
            let request = LedgerExportRequest {
                kind,
                ledger_hash: ledger_hash.clone(),
                best_tip: LedgerExportBestTip {
                    state_hash: block.state_hash.clone(),
                    blockchain_length: block.blockchain_length,
                    global_slot_since_genesis: block.global_slot_since_genesis,
                    epoch: block.epoch,
                },
                chain: request.chain.clone(),
                protocol_states: request.protocol_states.clone(),
            };
            self.export_hard_fork_ledger(&request)
                .map_err(|e| format!("{kind}: {e}"))
        };
        let ledger = export(LedgerExportKind::StagedLedger, &block.staged_ledger_hash)?;
        let staking = export(
            LedgerExportKind::StakingEpochLedger,
            &inputs.staking_epoch.ledger_hash,
        )?;
        let next = export(
            LedgerExportKind::NextEpochLedger,
            &inputs.next_epoch.ledger_hash,
        )?;
        Ok(RpcHardForkExport {
            inputs: inputs.clone(),
            chain_id: request.chain_id.clone(),
            ledger,
            staking,
            next,
        })
    }

    fn export_hard_fork_ledger(
        &mut self,
        request: &LedgerExportRequest,
    ) -> Result<HardForkLedger, String> {
        let target = &request.ledger_hash;
        let mut temporary = None;
        let result = match self.mask(target) {
            Some((mask, true)) => Ok(mask),
            Some((_, false)) => Err(format!("ledger {target} is still syncing")),
            None => self.rebuild_snarked_ledger(request, &mut temporary),
        }
        .and_then(|mut mask| {
            let hash = LedgerHash::from_fp(mask.merkle_root());
            if &hash != target {
                return Err(format!(
                    "ledger {target} has Merkle root {hash} on this node"
                ));
            }
            let accounts = mask.to_list();
            let leaves: Vec<Fp> = accounts.par_iter().map(|a| a.hash()).collect();
            // The same leaves under the stable empty account must give the
            // ledger's own root: the accounts are complete and in index order.
            let stable = merkle_root_of_leaves(&leaves, LEDGER_DEPTH, Account::empty().hash());
            if LedgerHash::from_fp(stable) != hash {
                return Err(format!(
                    "ledger {target}: its {} accounts in index order hash to {}",
                    accounts.len(),
                    LedgerHash::from_fp(stable)
                ));
            }
            let migrated =
                merkle_root_of_leaves(&leaves, LEDGER_DEPTH, migrated_empty_account().hash());
            let mut json = Vec::with_capacity(accounts.len().saturating_mul(1024));
            crate::daemon_json::write_accounts(&mut json, &accounts)
                .map_err(|e| format!("failed to write the accounts: {e}"))?;
            Ok(HardForkLedger {
                hash,
                migrated_hash: LedgerHash::from_fp(migrated),
                num_accounts: accounts.len() as u64,
                accounts_json: String::from_utf8(json)
                    .map_err(|e| format!("accounts are not UTF-8: {e}"))?,
            })
        });
        if let Some(first) = temporary {
            first.unregister_mask(ledger::UnregisterBehavior::Recursive);
        }
        result
    }

    /// Rebuilds a snarked ledger of the best chain that the service doesn't
    /// hold, from the nearest one before it that it holds, block by block, as
    /// OCaml's `get_snarked_ledger_full` does from the root. The first mask
    /// made goes into `temporary`, for the caller to unregister.
    fn rebuild_snarked_ledger(
        &mut self,
        request: &LedgerExportRequest,
        temporary: &mut Option<Mask>,
    ) -> Result<Mask, String> {
        let target = &request.ledger_hash;
        let chain = &request.chain;
        let end = chain
            .iter()
            .rposition(|(snarked, _)| snarked == target)
            .ok_or_else(|| {
                format!("ledger {target} is not held by this node, nor a snarked ledger of its best chain")
            })?;
        let start = (0..=end)
            .rev()
            .find(|i| matches!(self.mask(&chain[*i].0), Some((_, true))))
            .ok_or_else(|| format!("no snarked ledger to rebuild {target} from"))?;
        let (mut ledger, _) = self
            .mask(&chain[start].0)
            .ok_or_else(|| format!("no snarked ledger to rebuild {target} from"))?;
        let mut previous = &chain[start].0;
        for (snarked, staged) in &chain[start + 1..=end] {
            if snarked == previous {
                continue;
            }
            let mut mt = ledger.make_child();
            temporary.get_or_insert_with(|| mt.clone());
            self.apply_emitted_proof(&request.protocol_states, &mut mt, staged)?;
            let obtained = LedgerHash::from_fp(mt.merkle_root());
            if &obtained != snarked {
                return Err(format!(
                    "rebuilding snarked ledger {snarked} gave {obtained}"
                ));
            }
            ledger = mt;
            previous = snarked;
        }
        Ok(ledger)
    }

    pub fn get_account_delegators(
        &self,
        ledger_hash: &LedgerHash,
        account_id: &AccountId,
    ) -> Option<Vec<Account>> {
        let (mask, _) = self.mask(ledger_hash)?;
        let delegate = &account_id.public_key;
        let accounts = self
            .delegating_accounts(ledger_hash, &mask, delegate, |account| {
                account.delegate.as_ref() == Some(delegate)
            })
            .into_iter()
            .map(|(_, account)| account)
            .collect();
        Some(accounts)
    }

    /// The VRF evaluator's delegator table of a producer in a staking ledger:
    /// each delegating account's index, key and balance. The delegating
    /// accounts are recorded for `lastEpochDelegators` on the way, so that
    /// GraphQL needs no scan of its own.
    pub fn producer_delegator_table(
        &mut self,
        ledger_hash: &LedgerHash,
        producer: &AccountPublicKey,
    ) -> Option<DelegatorTable> {
        let producer = CompressedPubKey::try_from(producer.clone()).ok()?;
        let (mask, _) = self.mask(ledger_hash)?;
        let delegating = self.delegating_accounts(ledger_hash, &mask, &producer, |account| {
            staking_delegate(account) == Some(&producer)
        });
        let table = delegating
            .iter()
            .map(|(index, account)| {
                let pub_key = AccountPublicKey::from(account.public_key.clone());
                (*index, (pub_key, account.balance.as_u64()))
            })
            .collect();
        let delegators = delegating.into_iter().map(|(_, account)| account).collect();
        self.epoch_delegatees
            .record(&producer, ledger_hash.clone(), delegators);
        Some(table)
    }

    /// The delegators [`DelegatorsQuery`] asks for.
    pub fn delegators_for_rpc(
        &self,
        query: &DelegatorsQuery,
        account_id: &AccountId,
    ) -> Option<Vec<Account>> {
        match query {
            DelegatorsQuery::Ledger(ledger_hash) => {
                self.get_account_delegators(ledger_hash, account_id)
            }
            DelegatorsQuery::LastEpoch { limit } => self
                .epoch_delegatees
                .last_epoch_delegators(account_id, *limit),
        }
    }

    #[allow(clippy::type_complexity)]
    pub fn producers_with_delegates<F: FnMut(&CompressedPubKey) -> bool>(
        &self,
        ledger_hash: &LedgerHash,
        mut filter: F,
    ) -> Option<BTreeMap<AccountPublicKey, Vec<(ledger::AccountIndex, AccountPublicKey, u64)>>>
    {
        let (mask, _) = self.mask(ledger_hash)?;
        let mut accounts = Vec::new();

        mask.iter(|account| {
            if let Some(delegate) = staking_delegate(&account).filter(|delegate| filter(delegate)) {
                accounts.push((account.id(), delegate.clone(), account.balance.as_u64()));
            }
        });

        let producers = accounts.into_iter().fold(
            BTreeMap::<_, Vec<_>>::new(),
            |mut producers, (id, delegate, balance)| {
                let index = mask.index_of_account(id.clone()).unwrap();
                let pub_key = AccountPublicKey::from(id.public_key);
                let producer = AccountPublicKey::from(delegate);
                producers
                    .entry(producer)
                    .or_default()
                    .push((index, pub_key, balance));
                producers
            },
        );
        Some(producers)
    }

    pub fn child_hashes_get(
        &mut self,
        snarked_ledger_hash: LedgerHash,
        parent: &LedgerAddress,
    ) -> Result<(LedgerHash, LedgerHash), String> {
        let mut mask = self.pending_sync_snarked_ledger_mask(&snarked_ledger_hash)?;
        let left_hash = LedgerHash::from_fp(mask.get_inner_hash_at_addr(parent.child_left())?);
        let right_hash = LedgerHash::from_fp(mask.get_inner_hash_at_addr(parent.child_right())?);

        Ok((left_hash, right_hash))
    }

    /// Sets `accounts` under `parent` if they hash to `expected_hash`, and
    /// returns the hash they give the subtree. Accounts that don't match are
    /// left out of the ledger, so a bad answer can't change what a later one
    /// hashes to.
    pub fn accounts_set(
        &mut self,
        snarked_ledger_hash: LedgerHash,
        parent: &LedgerAddress,
        expected_hash: &LedgerHash,
        accounts: Vec<v2::MinaBaseAccountBinableArgStableV2>,
    ) -> Result<LedgerHash, String> {
        let mut mask = self.pending_sync_snarked_ledger_mask(&snarked_ledger_hash)?;
        let height = LEDGER_DEPTH
            .checked_sub(parent.length())
            .ok_or("Address deeper than the ledger")?;
        let mut accounts: Vec<Box<Account>> = accounts
            .into_iter()
            .map(|account| Ok(Box::new((&account).try_into()?)))
            .collect::<Result<Vec<_>, InvalidBigInt>>()
            .map_err(error_to_string)?;
        // Only as many as the subtree has leaves are set.
        if let Some(leaves) = 1usize.checked_shl(height as u32) {
            accounts.truncate(leaves);
        }

        // Hashing the accounts is most of the work of syncing a ledger, about
        // 1 ms each in the browser, so hash them in parallel and seed the
        // leaf hashes, leaving only the inner nodes to hash here.
        let hashes: Vec<_> = accounts.par_iter().map(|account| account.hash()).collect();
        let answer_hash = LedgerHash::from_fp(subtree_hash(hashes.clone(), height));
        if &answer_hash != expected_hash {
            return Ok(answer_hash);
        }
        mask.set_all_accounts_rooted_at(parent.clone(), &accounts)
            .map_err(|_| "Failed when setting accounts".to_owned())?;
        for (leaf, hash) in parent.iter_children(LEDGER_DEPTH).zip(hashes) {
            mask.set_cached_hash_unchecked(&leaf, hash);
        }

        let computed_hash = LedgerHash::from_fp(mask.get_inner_hash_at_addr(parent.clone())?);

        Ok(computed_hash)
    }

    /// Rebuilds the root's staged ledger from the synced root snarked ledger
    /// and the staged ledger parts, as a mask over that snarked ledger, as
    /// OCaml's root staged ledger is (R38), not over a full copy of it.
    ///
    /// It runs here, on the ledger thread. The staged ledger's mask reads the
    /// snarked ledger, which the ledger thread also serves, and a mask is only
    /// ever used by one thread at a time (`Mask::with` takes its lock with
    /// `try_lock().expect(..)`). It used to run on a thread of its own, over a
    /// copy no other thread touched.
    pub fn staged_ledger_reconstruct_sync(
        &mut self,
        snarked_ledger_hash: LedgerHash,
        parts: Option<Arc<StagedLedgerAuxAndPendingCoinbasesValid>>,
    ) -> Result<(v2::LedgerHash, Result<(), String>), InvalidBigInt> {
        let snarked_ledger = self
            .sync
            .snarked_ledger_mut(snarked_ledger_hash.clone())?
            .clone();
        let (staged_ledger_hash, result) =
            staged_ledger_reconstruct(snarked_ledger, snarked_ledger_hash, parts)?;
        let result = match result {
            Err(err) => Err(err),
            Ok(staged_ledger) => {
                self.staged_ledger_reconstruct_result_store(staged_ledger);
                Ok(())
            }
        };

        Ok((staged_ledger_hash, result))
    }

    pub fn block_apply(
        &mut self,
        block: ArcBlockWithHash,
        pred_block: AppliedBlock,
        skip_verification: Option<SkipVerification>,
    ) -> Result<BlockApplyResult, String> {
        self.block_apply_with(block, pred_block, skip_verification, false)
    }

    /// As [`Self::block_apply`]. With `restored` (a block a frontier store
    /// restore replays, I5d1), a staged ledger hash mismatch is an error
    /// rather than a panic: the node then drops the store and bootstraps
    /// (R20 D5), where a peer's block that mismatches stops the node.
    pub fn block_apply_with(
        &mut self,
        block: ArcBlockWithHash,
        pred_block: AppliedBlock,
        skip_verification: Option<SkipVerification>,
        restored: bool,
    ) -> Result<BlockApplyResult, String> {
        // Catch-up blocks bypass gossip prevalidation. Enforce the same stop
        // policy before applying any ledger changes, even with proof checks off.
        mina_core::block::prevalidate::validate_stop_slots(
            &block,
            mina_core::constants::stop_slots(),
        )
        .map_err(|err| format!("{err:?}"))?;
        mina_core::debug!(mina_core::log::system_time();
            kind = "LedgerService::block_apply",
            summary = format!("{}, {} <- {}", block.height(), block.hash(), block.pred_hash()),
            pred_staged_ledger_hash = pred_block.merkle_root_hash().to_string(),
            staged_ledger_hash = block.merkle_root_hash().to_string(),
        );
        // OCaml skips this only for blocks loaded from its own persistence;
        // here even catch-up blocks applied with `SkipVerification::All` and
        // our own blocks are checked, as it costs little next to applying.
        let started = redux::Instant::now();
        let body_result = crate::transition_frontier::body_reference::validate_body_reference(
            block.block.as_ref(),
        );
        self.send_event(LedgerEvent::Verification(
            crate::verification::VerificationUpdate::Body {
                block: crate::verification::BlockIdentity::new(&block),
                passed: body_result.is_ok(),
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            },
        ));
        body_result.map_err(|error| format!("{error:?}"))?;

        let mut staged_ledger = self
            .staged_ledger_mut(pred_block.staged_ledger_hashes())
            .ok_or_else(|| {
                format!(
                    "parent staged ledger missing: {:#?}",
                    pred_block.staged_ledger_hashes()
                )
            })?
            .clone();

        let global_slot = block.global_slot_since_genesis();
        let prev_protocol_state = &pred_block.header().protocol_state;
        let prev_state_view = protocol_state_view(prev_protocol_state).map_err(error_to_string)?;

        let consensus_state = &block.header().protocol_state.body.consensus_state;
        let coinbase_receiver: CompressedPubKey = (&consensus_state.coinbase_receiver)
            .try_into()
            .map_err(error_to_string)?;
        let supercharge_coinbase = consensus_state.supercharge_coinbase;

        let diff: Diff = (&block.body().staged_ledger_diff)
            .try_into()
            .map_err(error_to_string)?;

        let prev_protocol_state: ledger::proofs::block::ProtocolState =
            prev_protocol_state.try_into()?;

        let result = staged_ledger
            .apply(
                skip_verification,
                constraint_constants(),
                Slot::from_u32(global_slot),
                diff,
                (),
                &Verifier,
                &prev_state_view,
                prev_protocol_state.hashes(),
                coinbase_receiver.clone(),
                supercharge_coinbase,
            )
            .map_err(|err| format!("{err:?}"))?;
        let just_emitted_a_proof = result.ledger_proof.is_some();
        let ledger_hashes = MinaBaseStagedLedgerHashStableV1::from(&result.hash_after_applying);

        // TODO(binier): return error if not matching.
        let expected_ledger_hashes = block.staged_ledger_hashes();
        if &ledger_hashes != expected_ledger_hashes {
            let staged_ledger = self
                .staged_ledger_mut(pred_block.staged_ledger_hashes())
                .unwrap(); // We already know the ledger exists, see the same call a few lines above

            match dump_application_to_file(staged_ledger, block.clone(), pred_block) {
                Ok(filename) => mina_core::info!(
                    mina_core::log::system_time();
                    kind = "LedgerService::dump - Failed application",
                    summary = format!("StagedLedger and block saved to: {filename:?}")
                ),
                Err(e) => mina_core::error!(
                    mina_core::log::system_time();
                    kind = "LedgerService::dump - Failed application",
                    summary = format!("Failed to save block application to file: {e:?}")
                ),
            }

            if restored {
                return Err(format!(
                    "staged ledger hash mismatch replaying a restored block. found: {ledger_hashes:?}, expected: {expected_ledger_hashes:?}"
                ));
            }
            panic!("staged ledger hash mismatch. found: {ledger_hashes:#?}, expected: {expected_ledger_hashes:#?}");
        }

        let archive_data = if self.archive_mode {
            BlockApplyResultArchive::new(&block, &staged_ledger, &result.accounts_created)
                .inspect_err(|e| {
                    mina_core::error!(mina_core::log::system_time();
                        kind = "LedgerService::block_apply",
                        summary = format!("no archive data for {}: {e}", block.hash()),
                    )
                })
                .ok()
        } else {
            None
        };

        block.cache_proofs().map_err(|error| error.to_string())?;
        self.sync
            .staged_ledgers
            .insert(Arc::new(ledger_hashes), staged_ledger);

        // staged_ledger.ledger().get_at_index(index)

        Ok(BlockApplyResult {
            block,
            just_emitted_a_proof,
            archive_data,
        })
    }

    /// Snapshot before commit prunes/reparents the old frontier's masks.
    /// Protocol states are shared across the window and pruned to the union
    /// required by its scan states, including states older than the window.
    fn retain_root_history(
        &mut self,
        history: &crate::transition_frontier::root_history::RootHistory,
        protocol_states: &BTreeMap<StateHash, MinaStateProtocolStateValueStableV2>,
    ) {
        self.root_history
            .retain(|hash, _| history.get(hash).is_some());
        for block in history.iter() {
            if self.root_history.contains_key(block.hash()) {
                continue;
            }
            let Some(ledger) = self.staged_ledger_mut(block.staged_ledger_hashes()) else {
                // A restored or disconnected frontier can lack old auxiliary
                // data. Such requests receive None, not a partial response.
                continue;
            };
            ledger.pending_coinbase_collection_merkle_root();
            let root = HistoricalRoot {
                scan_state: ledger.scan_state().clone(),
                pending_coinbase: ledger.pending_coinbase_collection().into(),
                ledger_hash: block.staged_ledger_hashes().clone(),
            };
            self.root_history.insert(block.hash().clone(), root);
        }
        self.root_history_protocol_states
            .extend(protocol_states.clone());
        self.root_history_protocol_states.extend(
            history
                .iter()
                .map(|b| (b.hash().clone(), b.header().protocol_state.clone())),
        );
        let needed: BTreeSet<StateHash> = self
            .root_history
            .values()
            .flat_map(|root| root.scan_state.required_state_hashes())
            .map(|fp| DataHashLibStateHashStableV1(fp.into()).into())
            .collect();
        self.root_history_protocol_states
            .retain(|hash, _| needed.contains(hash));
    }

    pub fn commit(
        &mut self,
        root_history: crate::transition_frontier::root_history::RootHistory,
        mut ledgers_to_keep: LedgersToKeep,
        root_snarked_ledger_updates: TransitionFrontierRootSnarkedLedgerUpdates,
        needed_protocol_states: BTreeMap<StateHash, MinaStateProtocolStateValueStableV2>,
        new_root: &ArcBlockWithHash,
        new_best_tip: &ArcBlockWithHash,
    ) -> CommitResult {
        mina_core::debug!(mina_core::log::system_time();
            kind = "LedgerService::commit",
            summary = format!("commit {}, {}", new_best_tip.height(), new_best_tip.hash()),
            new_root = format!("{}, {}", new_root.height(), new_root.hash()),
            new_root_staking_epoch_ledger = new_root.staking_epoch_ledger_hash().to_string(),
            new_root_next_epoch_ledger = new_root.next_epoch_ledger_hash().to_string(),
            new_root_snarked_ledger = new_root.snarked_ledger_hash().to_string(),
        );
        // The crash tests' kill points through a commit (PC3b).
        use ::ledger::database::disk::kill_point;
        kill_point("commit-start");
        self.recreate_snarked_ledger(
            &root_snarked_ledger_updates,
            &needed_protocol_states,
            new_root.snarked_ledger_hash(),
        )
        .unwrap();

        self.retain_root_history(&root_history, &needed_protocol_states);

        // Ledgers the frontier store is writing whole stay as they are, with
        // their ancestors, until it is done (R20 D3).
        self.persist_keep_pinned(&mut ledgers_to_keep);
        self.drop_snarked_ledgers(&ledgers_to_keep);
        self.epoch_delegatees.commit(new_root, new_best_tip);
        self.snarked_ledgers.extend(
            std::mem::take(&mut self.sync.snarked_ledgers)
                .into_iter()
                .filter(|(hash, _)| {
                    let keep = ledgers_to_keep.contains(hash);
                    if !keep {
                        mina_core::debug!(mina_core::log::system_time();
                            kind = "LedgerService::commit - snarked_ledgers.drop",
                            summary = format!("drop snarked ledger {hash}"));
                    }
                    keep
                }),
        );

        self.staged_ledgers
            .retain(|hash| ledgers_to_keep.contains(hash));
        self.staged_ledgers.extend(
            self.sync
                .staged_ledgers
                .take()
                .into_iter()
                .filter(|(hash, _)| ledgers_to_keep.contains(&**hash)),
        );

        for ledger_hash in [
            new_best_tip.staking_epoch_ledger_hash(),
            new_root.snarked_ledger_hash(),
            new_root.merkle_root_hash(),
        ] {
            if let Some((mut mask, is_synced)) = self.mask(ledger_hash) {
                if !is_synced {
                    panic!("ledger mask expected to be synced: {ledger_hash}");
                }
                let calculated = merkle_root(&mut mask);
                assert_eq!(ledger_hash, &calculated, "ledger mask hash mismatch");
            } else {
                panic!("ledger mask is missing: {ledger_hash}");
            }
        }

        self.compact_snarked_ledgers(&ledgers_to_keep);
        kill_point("commit-compacted");
        self.freeze_epoch_ledgers(new_root, new_best_tip);
        self.freeze_root_snarked_ledger(new_root);
        kill_point("commit-frozen");

        // TODO(tizoc): should this fail silently?
        if !self.commit_root_staged_ledger(
            new_root.staged_ledger_hashes(),
            new_root.snarked_ledger_hash(),
        ) {
            return Default::default();
        }

        let needed_protocol_states = self
            .staged_ledger_mut(new_root.staged_ledger_hashes())
            .map(|l| {
                l.scan_state()
                    .required_state_hashes()
                    .into_iter()
                    .map(|fp| DataHashLibStateHashStableV1(fp.into()).into())
                    .collect()
            })
            .unwrap_or_default();

        let available_jobs = Arc::new(
            self.staged_ledger_mut(new_best_tip.staged_ledger_hashes())
                .map(|l| {
                    l.scan_state()
                        .all_job_pairs_iter()
                        .map(|job| job.map(|single| AvailableJobMessage::from(single)))
                        .collect()
                })
                .unwrap_or_default(),
        );

        // self.check_alive_masks();
        kill_point("commit-end");

        CommitResult {
            alive_masks: ::ledger::mask::alive_len(),
            available_jobs,
            needed_protocol_states,
        }
    }

    /// Makes the new root's staged ledger mask the root staged ledger's, as
    /// OCaml's `move_root` commits the new root's mask into the old root's,
    /// which is parented off the root snarked ledger. The masks from the new
    /// root's up to the root staged ledger's are committed and detached. The
    /// chain stops at the first mask whose parent is a snarked ledger, where
    /// the root staged ledger's mask hangs since it is rebuilt as a mask
    /// (R38), or at a database of its own, as the genesis staged ledger's copy
    /// is: the staged changes never go into a snarked ledger. False if the new
    /// root's staged ledger is missing.
    ///
    /// The root staged ledger's mask is then kept over the root snarked
    /// ledger `root_snarked` itself ([`Self::rebase_root_staged_ledger`]).
    /// After an epoch boundary the root snarked ledger is a new mask over the
    /// object the staged mask was built on, which has become the next epoch
    /// ledger, and PC3b's and PC3c's `detach_into_root` later make it a
    /// database of its own. In memory the old object is folded back into
    /// service when it is dropped two boundaries later
    /// ([`Self::compact_snarked_ledgers`]), so a staged mask left on it is
    /// over the root snarked ledger again. On disk nothing folds a detached
    /// ledger back, and the staged mask would be committed into the dropped
    /// ledger's database: an orphan root with its file and hash matrix, and
    /// an overlay no commit ever writes out (REL-REVIEW finding 1).
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_frontier/full_frontier/full_frontier.ml#L457-L479>
    fn commit_root_staged_ledger(
        &mut self,
        new_root: &MinaBaseStagedLedgerHashStableV1,
        root_snarked: &LedgerHash,
    ) -> bool {
        let snarked_ledgers = self
            .snarked_ledgers
            .values()
            .map(|mask| mask.get_uuid())
            .collect::<BTreeSet<_>>();
        let Some(new_root_ledger) = self.staged_ledgers.get_mut(new_root) else {
            return false;
        };
        new_root_ledger
            .commit_and_reparent_to_base(|parent| snarked_ledgers.contains(&parent.get_uuid()));
        let staged = new_root_ledger.ledger();
        let Some(onto) = self.snarked_ledgers.get(root_snarked) else {
            bug_condition!(
                "commit_root_staged_ledger: the root snarked ledger {root_snarked} is not held"
            );
            return true;
        };
        match staged.get_parent() {
            Some(parent) if parent.get_uuid() == onto.get_uuid() => {}
            Some(parent) => self.rebase_root_staged_ledger(&staged, &parent, onto, root_snarked),
            // A database of its own: the genesis staged ledger's copy, in
            // memory. One on disk is a dropped snarked ledger the staged
            // mask was committed into, which the rebase above prevents.
            None => {
                if staged.disk_state().is_some() {
                    bug_condition!(
                        "the root staged ledger is a database on disk: a dropped snarked ledger"
                    );
                }
            }
        }
        // Restore uses the same scan-state references in
        // `hold_referenced_accounts`. In particular, a changed-back account
        // can equal the snarked value while a pending proof still changes it.
        // Reclaim only over the current snarked ledger, never after a failed
        // rebase. All comparisons use resident leaf hashes, even on disk.
        if staged
            .get_parent()
            .is_some_and(|parent| parent.get_uuid() == onto.get_uuid())
        {
            let pending = self
                .staged_ledgers
                .get(new_root)
                .unwrap()
                .scan_state()
                .staged_accounts_referenced();
            staged.reclaim_unreferenced_accounts(&pending);
        }
        true
    }

    /// Moves the root staged ledger's mask `staged` from `from`, a snarked
    /// ledger it was built over, onto `onto`, the root snarked ledger, with
    /// its accounts, its children, its uuid and its cached hashes.
    ///
    /// The two parents differ only in accounts of transactions whose proofs
    /// the root's moves emitted since the mask was built over `from`. Every
    /// one of those was staged first, so the mask owns it: a mask never
    /// drops an own account when its parent sets it (PRUNE-1), and `from`
    /// has not changed since (it is an epoch ledger, or the ledger the
    /// root detached from). So the mask reads the same over either parent,
    /// and so do its cached hashes, which are of paths through its own
    /// accounts. That is checked before the move, exactly and without
    /// hashing: a Merkle walk of `onto` against `from` finds the accounts
    /// that differ, and each must be the mask's own. If one isn't, the mask
    /// stays where it is, the node runs on as before this fix, and the case
    /// is reported.
    ///
    /// This move keeps all own accounts, equal to `onto`'s or not. One that
    /// transactions still in the scan state changed and changed back equals
    /// the parent's, and must stay the mask's own until those transactions
    /// are emitted: a proof emitted later may end part way through them, and
    /// the snarked ledger then holds a value the staged ledger must not show.
    /// The commit then reclaims equal accounts absent from the scan state.
    /// No file is written, and the reads are of
    /// the kind every block makes.
    fn rebase_root_staged_ledger(
        &self,
        staged: &Mask,
        from: &Mask,
        onto: &Mask,
        onto_hash: &LedgerHash,
    ) {
        let started = redux::Instant::now();
        let own = staged
            .own_account_indexes()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let differing = match crate::persist::diff::merkle_diff_bounded(onto, from, own.len()) {
            Ok(Some(diff)) => diff
                .accounts
                .into_iter()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            Ok(None) => {
                bug_condition!(
                    "the root snarked ledger {onto_hash} differs from the ledger the root staged ledger hangs from in more than the {} accounts the staged ledger owns; left where it is",
                    own.len()
                );
                return;
            }
            Err(error) => {
                bug_condition!(
                    "the root snarked ledger {onto_hash} can't be compared with the ledger the root staged ledger hangs from: {error}; left where it is"
                );
                return;
            }
        };
        let not_own = differing
            .iter()
            .filter(|index| !own.contains(index))
            .count();
        if not_own > 0 {
            bug_condition!(
                "the root snarked ledger {onto_hash} differs from the ledger the root staged ledger hangs from in {not_own} accounts the staged ledger doesn't own; left where it is"
            );
            return;
        }
        if !staged.move_onto_parent(onto) {
            bug_condition!("the root staged ledger isn't an attached mask; left where it is");
            return;
        }
        mina_core::info!(
            mina_core::log::system_time();
            kind = "LedgerService::rebase_root_staged_ledger",
            summary = format!("root staged ledger moved onto the root snarked ledger {onto_hash}"),
            own_accounts = own.len(),
            differing_accounts = differing.len(),
            ms = started.elapsed().as_secs_f64() * 1000.0,
        );
    }

    /// A commit's first step: the snarked ledgers not kept are dropped.
    fn drop_snarked_ledgers(&mut self, ledgers_to_keep: &LedgersToKeep) {
        self.snarked_ledgers.retain(|hash, _| {
            let keep = ledgers_to_keep.contains(hash);
            if !keep {
                mina_core::debug!(mina_core::log::system_time();
                    kind = "LedgerService::commit - snarked_ledgers.drop",
                    summary = format!("drop snarked ledger {hash}"));
            }
            keep
        });
    }

    fn compact_snarked_ledgers(&mut self, ledgers_to_keep: &LedgersToKeep) {
        for (ledger_hash, snarked_ledger) in self.snarked_ledgers.iter_mut() {
            while let Some((parent_hash, parent)) = snarked_ledger
                .get_parent()
                .map(|mut parent| (merkle_root(&mut parent), parent))
                .filter(|(parent_hash, _)| !ledgers_to_keep.contains(parent_hash))
            {
                mina_core::debug!(mina_core::log::system_time();
                    kind = "LedgerService::commit - mask.commit_and_reparent",
                    summary = format!("{ledger_hash} -> {parent_hash}"));
                snarked_ledger.commit();
                // A retained staged/snarked ledger may still be our child.
                // Commit made our parent equivalent; preserve descendants by
                // reparenting instead of requiring this mask to be a leaf.
                snarked_ledger.remove_and_reparent();
                *snarked_ledger = parent;
            }
        }
    }

    #[allow(dead_code)]
    fn check_alive_masks(&mut self) {
        let mut alive: BTreeSet<_> = ::ledger::mask::alive_collect();
        let staged_ledgers = self
            .staged_ledgers
            .staged_ledgers
            .iter()
            .map(|(hash, ledger)| (&hash.non_snark.ledger_hash, ledger.ledger_ref()));

        let alive_ledgers = self
            .snarked_ledgers
            .iter()
            .chain(staged_ledgers)
            .map(|(hash, mask)| {
                let uuid = mask.get_uuid();
                if !alive.remove(&uuid) {
                    bug_condition!("mask not found among alive masks! uuid: {uuid}, hash: {hash}");
                }
                (uuid, hash)
            })
            .collect::<Vec<_>>();
        mina_core::debug!(redux::Timestamp::global_now(); "alive_ledgers_after_commit: {alive_ledgers:#?}");

        if !alive.is_empty() {
            bug_condition!(
                "masks alive which are no longer part of the ledger service: {alive:#?}"
            );
        }
    }

    pub fn get_num_accounts(
        &mut self,
        ledger_hash: v2::LedgerHash,
    ) -> Option<(u64, v2::LedgerHash)> {
        let mask = self.complete_mask(&ledger_hash)?;
        // fix(binier): incorrect ledger hash, must be a hash of a populated subtree.

        let num_accounts = mask.num_accounts() as u64;
        let first_node_addr = ledger::Address::first(
            LEDGER_DEPTH.saturating_sub(super::tree_height_for_num_accounts(num_accounts)),
        );
        let hash = LedgerHash::from_fp(mask.get_hash(first_node_addr)?);
        Some((num_accounts, hash))
    }

    /// The hashes `depth` levels below `addr`, left to right.
    pub fn get_child_hashes(
        &mut self,
        ledger_hash: v2::LedgerHash,
        addr: LedgerAddress,
        depth: usize,
    ) -> Option<Vec<v2::LedgerHash>> {
        let (mask, is_synced) = self.mask(&ledger_hash)?;
        Self::child_hashes_from_mask(mask, is_synced, addr, depth)
    }

    pub fn get_child_hashes_for_serving(
        &mut self,
        ledger_hash: v2::LedgerHash,
        addr: LedgerAddress,
        depth: usize,
    ) -> Option<Vec<v2::LedgerHash>> {
        let mask = self.complete_mask(&ledger_hash)?;
        Self::child_hashes_from_mask(mask, true, addr, depth)
    }

    fn child_hashes_from_mask(
        mask: Mask,
        is_synced: bool,
        addr: LedgerAddress,
        depth: usize,
    ) -> Option<Vec<v2::LedgerHash>> {
        if depth == 0
            || depth > super::read::MAX_SERVED_CHILD_HASHES_DEPTH
            || addr
                .length()
                .checked_add(depth)
                .is_none_or(|length| length > LEDGER_DEPTH)
        {
            return None;
        }
        let get_hash = |addr: LedgerAddress| {
            let depth = addr.length();
            mask.get_hash(addr)
                .map(|fp| MinaBaseLedgerHash0StableV1(fp.into()).into())
                .or_else(|| {
                    if is_synced {
                        Some(ledger_empty_hash_at_depth(depth))
                    } else {
                        None
                    }
                })
        };
        let descendants_depth = addr.length().checked_add(depth)?;
        if depth == 0 || descendants_depth > LEDGER_DEPTH {
            return None;
        }
        addr.iter_children(descendants_depth)
            .map(get_hash)
            .collect()
    }

    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/syncable_ledger/syncable_ledger.ml#L321-L377>
    pub fn get_child_accounts(
        &mut self,
        ledger_hash: v2::LedgerHash,
        addr: LedgerAddress,
    ) -> Result<Vec<v2::MinaBaseAccountBinableArgStableV2>, String> {
        use crate::p2p::channels::rpc::{
            CONTENTS_EMPTY, CONTENTS_TOO_LARGE, MAX_SERVED_ACCOUNT_SUBTREE_HEIGHT,
        };
        use mina_p2p_messages::binprot::BinProtWrite;
        let height = LEDGER_DEPTH
            .checked_sub(addr.length())
            .ok_or("Invalid ledger address")?;
        if height > MAX_SERVED_ACCOUNT_SUBTREE_HEIGHT {
            return Err(CONTENTS_TOO_LARGE.into());
        }
        let mask = self
            .complete_mask(&ledger_hash)
            .ok_or("Ledger unavailable")?;
        // At most 64 addresses, visited in ledger order. Check the encoded
        // size before collecting/encoding the full answer, leaving room for
        // result, length, constructor and list prefixes.
        let mut size = LedgerAnswerSize(32);
        let mut accounts = Vec::new();
        for address in addr.iter_children(LEDGER_DEPTH) {
            if let Some(account) = mask.get(address) {
                let account: v2::MinaBaseAccountBinableArgStableV2 = (&*account).into();
                account
                    .binprot_write(&mut size)
                    .map_err(|error| error.to_string())?;
                accounts.push(account);
            }
        }
        if accounts.is_empty() {
            return Err(CONTENTS_EMPTY.into());
        }
        Ok(accounts)
    }

    pub fn get_accounts(
        &mut self,
        ledger_hash: v2::LedgerHash,
        ids: Vec<AccountId>,
    ) -> Vec<Account> {
        let Some((mask, _)) = self.mask(&ledger_hash) else {
            mina_core::warn!(
                mina_core::log::system_time();
                kind = "LedgerService::get_accounts",
                summary = format!("Ledger not found: {ledger_hash:?}")
            );
            return Vec::new();
        };
        let addrs = mask
            .location_of_account_batch(&ids)
            .into_iter()
            .filter_map(|(_id, addr)| addr)
            .collect::<Vec<_>>();

        mask.get_batch(&addrs)
            .into_iter()
            .filter_map(|(_, account)| account.map(|account| *account))
            .collect::<Vec<_>>()
    }

    /// LN2: each account with its index, leaf hash and Merkle path in the
    /// complete ledger held under exactly `request.ledger_hash`, all from one
    /// snapshot of it, as OCaml's
    /// GraphQL `account { index leafHash merklePath }` and
    /// `encodedSnarkedLedgerAccountMembership` read them. Never a working
    /// sync mask. The path is folded back to the root before it is served,
    /// so a ledger whose contents don't match the hash it is held under is
    /// refused here rather than handed to a client.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1703-L1727>
    /// and <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L2422-L2462>
    pub fn light_account(&self, request: &LightAccountRead) -> RpcLightAccountResponse {
        let mut mask = self.complete_mask(&request.ledger_hash).ok_or_else(|| {
            RpcLightError::LedgerUnavailable {
                kind: request.ledger,
                hash: request.ledger_hash.clone(),
            }
        })?;
        let expected = request
            .ledger_hash
            .to_field::<Fp>()
            .map_err(|error| RpcLightError::Internal(error.to_string()))?;
        // Every account from this one mask: one snapshot of one ledger.
        let entries = request
            .account_ids
            .iter()
            .map(|account_id| Self::light_account_in(&mut mask, request, &expected, account_id))
            .collect();
        Ok(entries)
    }

    fn light_account_in(
        mask: &mut Mask,
        request: &LightAccountRead,
        expected: &Fp,
        account_id: &AccountId,
    ) -> RpcLightAccountEntry {
        let address = mask
            .location_of_account(account_id)
            .ok_or(RpcLightError::AccountNotFound(request.ledger))?;
        let account = mask
            .get(address.clone())
            .ok_or(RpcLightError::AccountNotFound(request.ledger))?;
        let path = mask.merkle_path(address.clone());
        let leaf_hash = account.hash();
        let merge = <ledger::V2 as ledger::TreeVersion>::hash_node;
        let root =
            path.iter()
                .enumerate()
                .fold(leaf_hash, |node, (height, sibling)| match sibling {
                    ledger::MerklePath::Left(right) => merge(height, node, *right),
                    ledger::MerklePath::Right(left) => merge(height, *left, node),
                });
        if path.len() != LEDGER_DEPTH || &root != expected {
            return Err(RpcLightError::Internal(format!(
                "the path from account {} does not reach ledger {}",
                address.to_index().as_u64(),
                request.ledger_hash
            )));
        }
        Ok(Box::new(RpcLightAccount {
            ledger: request.ledger,
            ledger_hash: request.ledger_hash.clone(),
            block: request.block.clone(),
            index: address.to_index().as_u64(),
            leaf_hash: (&leaf_hash).into(),
            account: (&*account).into(),
            merkle_path: path.iter().map(LightMerklePathElement::from).collect(),
        }))
    }

    /// The account that owns `token_id` in a ledger, as OCaml's
    /// `Ledger.token_owner`: every account in a ledger owns the token its
    /// id derives, and the default token has no owner. Positive index hits
    /// answer directly. A miss builds a complete index lazily (one Poseidon
    /// hash per account), cached for the requested ledger hash. This
    /// includes tokens with no holders and caches misses too. Public GraphQL
    /// excludes this field so visitors cannot trigger the initial scan.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/merkle_mask/masking_merkle_tree.ml#L536-L542>
    pub fn token_owner_for_rpc(
        &mut self,
        ledger_hash: v2::LedgerHash,
        token_id: TokenId,
    ) -> Vec<Account> {
        use rayon::prelude::*;

        if token_id.is_default() {
            return Vec::new();
        }
        let Some((mask, _)) = self.mask(&ledger_hash) else {
            return Vec::new();
        };
        // The database's optional index omits default-token accounts, so a
        // miss is not authoritative even when has_token_owners() is true.
        let owner = if let Some(owner) = mask.token_owner(token_id.clone()) {
            Some(owner)
        } else {
            if !self
                .token_owners
                .as_ref()
                .is_some_and(|(hash, _)| hash == &ledger_hash)
            {
                let mut ids = Vec::new();
                mask.iter(|account| ids.push(account.id()));
                let owners = ids
                    .into_par_iter()
                    .map(|id| (id.derive_token_id(), id))
                    .collect();
                self.token_owners = Some((ledger_hash.clone(), owners));
            }
            self.token_owners
                .as_ref()
                .and_then(|(_, owners)| owners.get(&token_id))
                .cloned()
        };
        owner
            .map(|owner| self.get_accounts(ledger_hash, vec![owner]))
            .unwrap_or_default()
    }

    pub fn staged_ledger_aux_and_pending_coinbase(
        &mut self,
        block_hash: &StateHash,
        ledger_hash: &MinaBaseStagedLedgerHashStableV1,
        protocol_states: BTreeMap<StateHash, MinaStateProtocolStateValueStableV2>,
    ) -> Option<Arc<StagedLedgerAuxAndPendingCoinbases>> {
        if let Some(root) = self.root_history.get(block_hash) {
            if &root.ledger_hash != ledger_hash {
                return None;
            }
            let needed_blocks = ordered_required_protocol_states(
                root.scan_state.required_state_hashes(),
                &self.root_history_protocol_states,
            )?;
            return Some(Arc::new(StagedLedgerAuxAndPendingCoinbases {
                scan_state: (&root.scan_state).into(),
                staged_ledger_hash: root.ledger_hash.non_snark.ledger_hash.clone(),
                pending_coinbase: root.pending_coinbase.clone(),
                needed_blocks,
            }));
        }
        let ledger = self.staged_ledger_mut(ledger_hash)?;
        let needed_blocks = ordered_required_protocol_states(
            ledger.scan_state().required_state_hashes(),
            &protocol_states,
        )?;
        // Required so that we can perform the conversion bellow, which
        // will not work if the hash is not available already.
        ledger.pending_coinbase_collection_merkle_root();
        Some(
            StagedLedgerAuxAndPendingCoinbases {
                scan_state: (ledger.scan_state()).into(),
                staged_ledger_hash: ledger_hash.non_snark.ledger_hash.clone(),
                pending_coinbase: (ledger.pending_coinbase_collection()).into(),
                needed_blocks,
            }
            .into(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn staged_ledger_diff_create(
        &mut self,
        pred_block: AppliedBlock,
        global_slot_since_genesis: v2::MinaNumbersGlobalSlotSinceGenesisMStableV1,
        is_new_epoch: bool,
        producer: NonZeroCurvePoint,
        delegator: NonZeroCurvePoint,
        coinbase_receiver: NonZeroCurvePoint,
        completed_snarks: BTreeMap<SnarkJobId, Snark>,
        supercharge_coinbase: bool,
        transactions_by_fee: Vec<valid::UserCommand>,
    ) -> Result<StagedLedgerDiffCreateOutput, String> {
        let mut staged_ledger = self
            .staged_ledger_mut(pred_block.staged_ledger_hashes())
            .ok_or_else(|| {
                format!(
                    "parent staged ledger missing: {}",
                    pred_block.merkle_root_hash()
                )
            })?
            .clone();

        // calculate merkle root hash, otherwise `MinaBasePendingCoinbaseStableV2::from` fails.
        staged_ledger.hash();
        let pending_coinbase_witness =
            MinaBasePendingCoinbaseStableV2::from(staged_ledger.pending_coinbase_collection());

        let protocol_state_view =
            protocol_state_view(&pred_block.header().protocol_state).map_err(error_to_string)?;

        // TODO(binier): include `invalid_txns` in output.
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L177-L237>
        let transactions_allowed = stop_slot_policy(
            mina_core::constants::stop_slots(),
            global_slot_since_genesis.as_u32(),
            pred_block.global_slot_diff(),
        )?;
        let (pre_diff, _invalid_txns) = if !transactions_allowed {
            (
                ledger::staged_ledger::diff::with_valid_signatures_and_proofs::Diff::empty(),
                Vec::new(),
            )
        } else {
            staged_ledger
                .create_diff(
                    constraint_constants(),
                    (&global_slot_since_genesis).into(),
                    Some(true),
                    (&coinbase_receiver).try_into().map_err(error_to_string)?,
                    (),
                    &protocol_state_view,
                    transactions_by_fee,
                    |stmt| completed_work_for_statement(&completed_snarks, stmt),
                    supercharge_coinbase,
                )
                .map_err(|err| format!("{err:?}"))?
        };

        // TODO(binier): maybe here, check if block reward is above threshold.
        // <https://github.com/minaprotocol/mina/blob/b3d418a8c0ae4370738886c2b26f0ec7bdb49303/src/lib/block_producer/block_producer.ml#L222>

        let pred_body_hash = pred_block
            .header()
            .protocol_state
            .body
            .try_hash()
            .map_err(error_to_string)?;
        let diff = (&pre_diff).into();

        let res = staged_ledger
            .apply_diff_unchecked(
                constraint_constants(),
                (&global_slot_since_genesis).into(),
                pre_diff,
                (),
                &protocol_state_view,
                (
                    pred_block.hash().0.to_field().map_err(error_to_string)?,
                    pred_body_hash.0.to_field().map_err(error_to_string)?,
                ),
                (&coinbase_receiver).try_into().map_err(error_to_string)?,
                supercharge_coinbase,
            )
            .map_err(|err| format!("{err:?}"))?;

        let diff_hash = block_body_hash(&diff).map_err(|err| format!("{err:?}"))?;
        let staking_ledger_hash = if is_new_epoch {
            pred_block.next_epoch_ledger_hash()
        } else {
            pred_block.staking_epoch_ledger_hash()
        };

        Ok(StagedLedgerDiffCreateOutput {
            diff,
            diff_hash,
            staged_ledger_hash: (&res.hash_after_applying).into(),
            emitted_ledger_proof: res
                .ledger_proof
                .map(|(proof, ..)| (&proof).into())
                .map(Arc::new),
            pending_coinbase_update: (&res.pending_coinbase_update.1).into(),
            pending_coinbase_witness: MinaBasePendingCoinbaseWitnessStableV2 {
                pending_coinbases: pending_coinbase_witness,
                is_new_stack: res.pending_coinbase_update.0,
            },
            stake_proof_sparse_ledger: self.stake_proof_sparse_ledger(
                staking_ledger_hash,
                &producer,
                &delegator,
            )?,
        })
    }

    /// The staking ledger's paths to the producer's and the delegator's
    /// accounts. A staking ledger this thread doesn't hold fails this block,
    /// as any diff error does, rather than the ledger thread.
    pub fn stake_proof_sparse_ledger(
        &mut self,
        staking_ledger: &LedgerHash,
        producer: &NonZeroCurvePoint,
        delegator: &NonZeroCurvePoint,
    ) -> Result<v2::MinaBaseSparseLedgerBaseStableV2, String> {
        let mask = self
            .snarked_ledgers
            .get(staking_ledger)
            .ok_or_else(|| format!("staking ledger {staking_ledger} missing"))?;
        let producer_id = ledger::AccountId::new(
            producer.try_into().map_err(error_to_string)?,
            ledger::TokenId::default(),
        );
        let delegator_id = ledger::AccountId::new(
            delegator.try_into().map_err(error_to_string)?,
            ledger::TokenId::default(),
        );
        let sparse_ledger = ledger::sparse_ledger::SparseLedger::of_ledger_subset_exn(
            mask.clone(),
            &[producer_id, delegator_id],
        );
        Ok((&sparse_ledger).into())
    }

    pub fn snark_pool_prover_account(
        &self,
        ledger_hash: &v2::LedgerHash,
        prover: &v2::NonZeroCurvePoint,
    ) -> Result<crate::snark_pool::ProverAccount, String> {
        // Never substitute an epoch/snarked/sync ledger for the best-tip staged
        // ledger. Missing ledger data is not proof of a missing account.
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool.ml#L370-L393>
        let mask = self
            .staged_ledgers
            .get_mask(ledger_hash)
            .ok_or("best-tip staged ledger unavailable")?;
        let public_key = prover.try_into().map_err(error_to_string)?;
        let id = AccountId::new(public_key, ledger::TokenId::default());
        let account = mask
            .location_of_account(&id)
            .and_then(|location| mask.get(location));
        Ok(crate::snark_pool::ProverAccount::from_account(
            account.as_deref(),
        ))
    }

    /// Read-only producer rehearsal on the ledger worker, using create_diff's
    /// temporary masks and the same ordered work lookup as actual production.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L177-L237>
    pub fn work_coverage(
        &self,
        request: crate::snark_pool::WorkCoverageRequest,
    ) -> Result<crate::snark_pool::WorkCoverage, String> {
        use crate::snark_pool::{work_coverage::coverage_counts, WorkCoverage};
        let ledger = self
            .staged_ledgers
            .get(request.parent.staged_ledger_hashes())
            .ok_or("parent staged ledger unavailable")?;
        let jobs = ledger.scan_state().work_statements_for_new_diff();
        let lookup = |stmt: &ledger::scan_state::scan_state::transaction_snark::work::Statement| {
            completed_work_for_statement(&request.completed_snarks, stmt)
        };
        let (jobs_available, available_prefix) =
            coverage_counts(&jobs, |stmt| lookup(stmt).is_some());
        let transactions_allowed = stop_slot_policy(
            mina_core::constants::stop_slots(),
            request.slot,
            request.parent.global_slot_diff(),
        )?;
        // Mesa's multiplier is 1, so supercharge eligibility cannot change the
        // diff. Do not give a misleading estimate on a different network.
        if constraint_constants().supercharged_coinbase_factor != 1 {
            return Err("coverage rehearsal requires Mesa coinbase multiplier 1".into());
        }
        let considered = request.transactions.len();
        let (diff, _) = if transactions_allowed {
            ledger
                .create_diff(
                    constraint_constants(),
                    Slot::from_u32(request.slot),
                    Some(false),
                    (&request.coinbase_receiver)
                        .try_into()
                        .map_err(error_to_string)?,
                    (),
                    &protocol_state_view(&request.parent.header().protocol_state)
                        .map_err(error_to_string)?,
                    request.transactions,
                    lookup,
                    false,
                )
                .map_err(|error| format!("{error:?}"))?
        } else {
            (
                ledger::staged_ledger::diff::with_valid_signatures_and_proofs::Diff::empty(),
                Vec::new(),
            )
        };
        let coinbase =
            ledger::staged_ledger::diff::coinbase(&diff.diff, constraint_constants(), false)
                .ok_or("coinbase amount overflow")?
                .as_u64();
        Ok(WorkCoverage {
            sampled_at: request.time,
            parent_hash: request.parent.hash().clone(),
            parent_height: request.parent.height(),
            slot_since_genesis: request.slot,
            coinbase_receiver: request.coinbase_receiver,
            receiver_source: request.receiver_source,
            jobs_needed: jobs.len(),
            jobs_available,
            available_prefix,
            first_missing_job: jobs.get(available_prefix).map(SnarkJobId::from),
            transactions_considered: considered,
            expected_transactions: diff.diff.0.commands.len()
                + diff.diff.1.as_ref().map_or(0, |d| d.commands.len()),
            expected_works: diff.diff.0.completed_works.len()
                + diff.diff.1.as_ref().map_or(0, |d| d.completed_works.len()),
            expected_coinbase_nanomina: coinbase,
            coinbase_can_be_included: coinbase > 0,
        })
    }

    /// PC1: the proofs this thread holds, for [`crate::proof_memory`]. The
    /// staged ledgers' scan states first, then what retired roots add.
    pub fn proof_memory(&self) -> crate::proof_memory::LedgerProofMemory {
        use crate::proof_memory::{
            base_job_zkapp_proofs, LedgerProofMemory, ProofTally, ProofsSeen,
        };
        use ledger::scan_state::scan_state::ScanState;
        use std::collections::HashSet;

        #[derive(Default)]
        struct Walk {
            seen: ProofsSeen,
            merge_jobs: HashSet<usize>,
            base_jobs: HashSet<usize>,
            merge_job_refs: u64,
            base_job_refs: u64,
        }

        impl Walk {
            fn scan_state(
                &mut self,
                scan_state: &ScanState,
                proofs: &mut ProofTally,
                zkapp_proofs: &mut ProofTally,
            ) {
                let (mut bases, mut merges) = (Vec::new(), Vec::new());
                scan_state
                    .scan_state
                    .for_each_retained_job(|b| bases.push(b.clone()), |m| merges.push(m.clone()));
                bases.extend(
                    scan_state
                        .previous_incomplete_zkapp_updates
                        .0
                        .iter()
                        .cloned(),
                );
                self.merge_job_refs += merges.len() as u64;
                self.base_job_refs += bases.len() as u64;
                for merge in merges {
                    if self.merge_jobs.insert(Arc::as_ptr(&merge) as usize) {
                        self.seen.tally_ref(proofs, &merge.0.proof);
                    }
                }
                for base in bases {
                    if self.base_jobs.insert(Arc::as_ptr(&base) as usize) {
                        base_job_zkapp_proofs(&mut self.seen, zkapp_proofs, &base);
                    }
                }
            }
        }

        let mut report = LedgerProofMemory::default();
        let mut walk = Walk::default();
        let staged = self
            .staged_ledgers
            .staged_ledgers
            .values()
            .chain(self.sync.staged_ledgers.staged_ledgers.values());
        for staged_ledger in staged {
            report.staged_ledgers += 1;
            walk.scan_state(
                staged_ledger.scan_state(),
                &mut report.scan_state_proofs,
                &mut report.base_job_zkapp_proofs,
            );
        }
        for root in self.root_history.values() {
            report.historical_roots += 1;
            walk.scan_state(
                &root.scan_state,
                &mut report.historical_root_proofs,
                &mut report.base_job_zkapp_proofs,
            );
        }
        report.merge_jobs = walk.merge_jobs.len() as u64;
        report.base_jobs = walk.base_jobs.len() as u64;
        report.merge_job_refs = walk.merge_job_refs;
        report.base_job_refs = walk.base_job_refs;
        report.addresses = walk.seen.into_addresses();
        report.snarked_ledgers = self
            .snarked_ledgers
            .iter()
            .chain(&self.additional_snarked_ledgers)
            .chain(&self.sync.snarked_ledgers)
            .map(|(hash, mask)| (hash.to_string(), mask.num_accounts() as u64))
            .collect();
        report
    }

    pub fn scan_state_summary(
        &self,
        staged_ledger_hash: &MinaBaseStagedLedgerHashStableV1,
    ) -> Result<Vec<Vec<RpcScanStateSummaryScanStateJob>>, String> {
        use ledger::scan_state::scan_state::JobValue;

        let ledger = self.staged_ledgers.get(staged_ledger_hash);
        let Some(ledger) = ledger else {
            return Ok(Vec::new());
        };
        ledger
            .scan_state()
            .view()
            .map(|jobs| {
                let jobs = jobs.collect::<Vec<JobValueWithIndex<'_>>>();
                let iter = jobs.iter().peekable();
                let mut res = Vec::with_capacity(jobs.len());

                for job in iter {
                    let (stmt, seq_no, job_kind, is_done) = match &job.job {
                        JobValue::Leaf(JobValueBase::Empty)
                        | JobValue::Node(JobValueMerge::Empty)
                        | JobValue::Node(JobValueMerge::Part(_)) => {
                            res.push(RpcScanStateSummaryScanStateJob::Empty);
                            continue;
                        }
                        JobValue::Leaf(JobValueBase::Full(job)) => {
                            let stmt = &job.job.statement;
                            let tx = &job.job.transaction_with_status;
                            let status = (&tx.status).into();
                            let tx = MinaTransactionTransactionStableV2::from(&tx.data);
                            let kind = RpcScanStateSummaryScanStateJobKind::Base(
                                RpcScanStateSummaryBlockTransaction {
                                    hash: tx.hash().ok(),
                                    kind: (&tx).into(),
                                    status,
                                },
                            );
                            let seq_no = job.seq_no.as_u64();
                            (stmt.clone(), seq_no, kind, job.state.is_done())
                        }
                        JobValue::Node(JobValueMerge::Full(job)) => {
                            let stmt = job
                                .left
                                .statement()
                                .merge(&job.right.statement())
                                .unwrap();
                            let kind = RpcScanStateSummaryScanStateJobKind::Merge;
                            let seq_no = job.seq_no.as_u64();
                            (stmt, seq_no, kind, job.state.is_done())
                        }
                    };
                    let stmt: MinaStateBlockchainStateValueStableV2LedgerProofStatement =
                        (&stmt).into();
                    let job_id: SnarkJobId = (&stmt.source, &stmt.target).into();

                    let bundle =
                        job.bundle_sibling()
                            .and_then(|(sibling_index, is_sibling_left)| {
                                let sibling_job = jobs.get(sibling_index)?;
                                let sibling_stmt: MinaStateBlockchainStateValueStableV2LedgerProofStatement = match &sibling_job.job {
                                    JobValue::Leaf(JobValueBase::Full(job)) => {
                                        (&job.job.statement).into()
                                    }
                                    JobValue::Node(JobValueMerge::Full(job)) => (&job
                                        .left
                                        .statement()
                                        .merge(&job.right.statement())
                                        .unwrap()).into(),
                                    _ => return None,
                                };
                                let bundle_job_id: SnarkJobId = match is_sibling_left {
                                    false => (&stmt.source, &sibling_stmt.target).into(),
                                    true => (&sibling_stmt.source, &stmt.target).into(),
                                };
                                Some((bundle_job_id, is_sibling_left))
                            });

                    let bundle_job_id = bundle
                        .as_ref()
                        .map_or_else(|| job_id.clone(), |(id, _)| id.clone());

                    if is_done {
                        res.push(RpcScanStateSummaryScanStateJob::Done {
                            job_id,
                            bundle_job_id,
                            job: Box::new(job_kind),
                            seq_no,
                            snark: None,
                        });
                    } else {
                        res.push(RpcScanStateSummaryScanStateJob::Todo {
                            job_id,
                            bundle_job_id,
                            job: job_kind,
                            seq_no,
                        });
                    }
                }
                Ok(res)
            })
            .collect()
    }
}

impl LedgerSyncState {
    fn mask(&self, hash: &LedgerHash) -> Option<(Mask, bool)> {
        self.snarked_ledgers
            .get(hash)
            .cloned()
            .map(|mask| (mask, false))
            .or_else(|| Some((self.staged_ledgers.get_mask(hash)?, true)))
    }

    fn pending_sync_snarked_ledger_mask(&self, hash: &LedgerHash) -> Result<Mask, String> {
        self.snarked_ledgers
            .get(hash)
            .cloned()
            .ok_or_else(|| format!("Missing sync snarked ledger {}", hash))
    }

    /// Returns a [Mask] instance for the snarked ledger with `hash`. If it doesn't
    /// exist a new instance is created.
    fn snarked_ledger_mut(&mut self, hash: LedgerHash) -> Result<&mut Mask, InvalidBigInt> {
        let hash_fp = hash.to_field()?;
        Ok(self.snarked_ledgers.entry(hash.clone()).or_insert_with(|| {
            let mut ledger = Mask::create(LEDGER_DEPTH);
            ledger.set_cached_hash_unchecked(&LedgerAddress::root(), hash_fp);
            ledger
        }))
    }

    fn staged_ledger_mut(
        &mut self,
        hash: &MinaBaseStagedLedgerHashStableV1,
    ) -> Option<&mut StagedLedger> {
        self.staged_ledgers.get_mut(hash)
    }
}

fn staged_ledger_reconstruct(
    snarked_ledger: Mask,
    snarked_ledger_hash: LedgerHash,
    parts: Option<Arc<StagedLedgerAuxAndPendingCoinbasesValid>>,
) -> Result<(v2::LedgerHash, Result<StagedLedger, String>), InvalidBigInt> {
    let staged_ledger_hash = parts
        .as_ref()
        .map(|p| p.staged_ledger_hash.clone())
        .unwrap_or_else(|| snarked_ledger_hash.clone());

    let ledger = snarked_ledger.make_child();

    let mut result = if let Some(parts) = &parts {
        let states = parts
            .needed_blocks
            .iter()
            .map(|state| Ok((state.try_hash()?.to_field()?, state.clone())))
            .collect::<Result<BTreeMap<Fp, _>, _>>()?;

        // Converted when the parts were checked: a shallow copy, the jobs
        // shared (R38).
        StagedLedger::of_scan_state_pending_coinbases_and_snarked_ledger(
            (),
            constraint_constants(),
            Verifier,
            parts.scan_state.clone(),
            ledger,
            LocalState::empty(),
            parts.staged_ledger_hash.to_field()?,
            parts.pending_coinbase.clone(),
            |key| states.get(&key).cloned().unwrap(),
        )
    } else {
        StagedLedger::create_exn(constraint_constants().clone(), ledger)
    };

    match result.as_mut() {
        Ok(_) => {
            // For profiling: the `reconstruct_staged_ledger` test in mina-tree
            // replays the dump. The snarked ledger is as it was: the staged
            // changes are in the mask over it.
            if std::env::var_os("MINA_DUMP_RECONSTRUCT").is_some() {
                if let Err(e) = dump_reconstruct_to_file(&snarked_ledger, &parts) {
                    mina_core::error!(
                        mina_core::log::system_time();
                        kind = "LedgerService::dump - Reconstruct",
                        summary = format!("Failed to save reconstruction to file: {e:?}")
                    );
                }
            }
        }
        Err(_) => {
            if let Err(e) = dump_reconstruct_to_file(&snarked_ledger, &parts) {
                mina_core::error!(
                    mina_core::log::system_time();
                    kind = "LedgerService::dump - Failed reconstruct",
                    summary = format!("Failed to save reconstruction to file: {e:?}")
                );
            }
        }
    }

    Ok((staged_ledger_hash, result))
}

pub trait LedgerService: redux::Service {
    fn ledger_manager(&self) -> &LedgerManager;
    fn force_sync_calls(&self) -> bool {
        false
    }

    fn write_init(&mut self, request: LedgerWriteRequest) {
        let request = LedgerRequest::Write(request);
        if self.force_sync_calls() {
            let _ = self.ledger_manager().call_sync(request);
        } else {
            self.ledger_manager().call(request);
        }
    }

    fn read_init(&mut self, id: LedgerReadId, request: LedgerReadRequest) {
        let request = LedgerRequest::Read(id, request);
        if self.force_sync_calls() {
            let _ = self.ledger_manager().call_sync(request);
        } else {
            self.ledger_manager().call(request);
        }
    }
}

/// Save reconstruction to file, when it fails.
/// So we can easily reproduce the application both in Rust and OCaml, to compare them.
fn dump_reconstruct_to_file(
    snarked_ledger: &Mask,
    parts: &Option<Arc<StagedLedgerAuxAndPendingCoinbasesValid>>,
) -> std::io::Result<()> {
    use mina_p2p_messages::binprot::{
        self,
        macros::{BinProtRead, BinProtWrite},
    };

    #[derive(BinProtRead, BinProtWrite)]
    struct ReconstructContext {
        accounts: Vec<v2::MinaBaseAccountBinableArgStableV2>,
        scan_state: v2::TransactionSnarkScanStateStableV2,
        pending_coinbase: v2::MinaBasePendingCoinbaseStableV2,
        staged_ledger_hash: LedgerHash,
        states: List<v2::MinaStateProtocolStateValueStableV2>,
    }

    let Some(parts) = parts else {
        return Err(std::io::ErrorKind::Other.into());
    };

    let StagedLedgerAuxAndPendingCoinbases {
        scan_state,
        staged_ledger_hash,
        pending_coinbase,
        needed_blocks,
    } = parts.to_wire();

    let reconstruct_context = ReconstructContext {
        accounts: snarked_ledger
            .to_list()
            .iter()
            .map(v2::MinaBaseAccountBinableArgStableV2::from)
            .collect(),
        scan_state,
        pending_coinbase,
        staged_ledger_hash,
        states: needed_blocks,
    };

    let debug_dir = mina_core::get_debug_dir();
    let filename = debug_dir
        .join("failed_reconstruct_ctx.binprot")
        .to_string_lossy()
        .to_string();
    std::fs::create_dir_all(&debug_dir)?;

    use mina_p2p_messages::binprot::BinProtWrite;
    let mut file = std::fs::File::create(&filename)?;
    reconstruct_context.binprot_write(&mut file)?;
    file.sync_all()?;

    mina_core::info!(
        mina_core::log::system_time();
        kind = "LedgerService::dump - Failed reconstruct",
        summary = format!("Reconstruction saved to: {filename:?}")
    );

    Ok(())
}

/// Save staged ledger and block to file, when the application fail.
/// So we can easily reproduce the application both in Rust and OCaml, to compare them.
/// - <https://github.com/o1-labs/mina-rust/blob/8e68037aafddd43842a54c8439baeafee4c6e1eb/ledger/src/staged_ledger/staged_ledger.rs#L5959>
/// - TODO: Find OCaml link, I remember having the same test in OCaml but I can't find where
fn dump_application_to_file(
    staged_ledger: &StagedLedger,
    block: ArcBlockWithHash,
    pred_block: AppliedBlock,
) -> std::io::Result<String> {
    use mina_p2p_messages::binprot::{
        self,
        macros::{BinProtRead, BinProtWrite},
    };

    #[derive(BinProtRead, BinProtWrite)]
    struct ApplyContext {
        accounts: Vec<v2::MinaBaseAccountBinableArgStableV2>,
        scan_state: v2::TransactionSnarkScanStateStableV2,
        pending_coinbase: v2::MinaBasePendingCoinbaseStableV2,
        pred_block: v2::MinaBlockBlockStableV2,
        blocks: Vec<v2::MinaBlockBlockStableV2>,
    }

    let cs = &block.block.header.protocol_state.body.consensus_state;
    let block_height = cs.blockchain_length.as_u32();

    let apply_context = ApplyContext {
        accounts: staged_ledger
            .ledger()
            .to_list()
            .iter()
            .map(v2::MinaBaseAccountBinableArgStableV2::from)
            .collect::<Vec<_>>(),
        scan_state: staged_ledger.scan_state().into(),
        pending_coinbase: staged_ledger.pending_coinbase_collection().into(),
        pred_block: (**pred_block.block()).clone(),
        blocks: vec![(*block.block).clone()],
    };

    let debug_dir = mina_core::get_debug_dir();
    let filename = debug_dir
        .join(format!("failed_application_ctx_{}.binprot", block_height))
        .to_string_lossy()
        .to_string();
    std::fs::create_dir_all(&debug_dir)?;

    let mut file = std::fs::File::create(&filename)?;

    use mina_p2p_messages::binprot::BinProtWrite;
    apply_context.binprot_write(&mut file)?;
    file.sync_all()?;

    Ok(filename)
}

/// Whether a block produced at this slot may include transactions, or an
/// error if no block may be produced. With no stop slots configured, the slot
/// offset is never computed, so that path behaves as it did before stop slots.
fn stop_slot_policy(
    stops: mina_core::constants::StopSlots,
    global_slot_since_genesis: u32,
    global_slot_diff: u32,
) -> Result<bool, String> {
    if !stops.any_stop() {
        return Ok(true);
    }
    let slot_since_hard_fork = global_slot_since_genesis
        .checked_sub(global_slot_diff)
        .ok_or("slot precedes hard fork")?;
    if !stops.blocks_allowed(slot_since_hard_fork) {
        return Err("reached slot_chain_end".into());
    }
    Ok(stops.transactions_allowed(slot_since_hard_fork))
}

/// `Account.Hardfork.empty` of OCaml 4.0.2: the empty account, with the
/// anticipated next release's default permissions, whose verification key
/// permission carries the next transaction version.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/account.ml#L1124-L1142>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/permissions.ml#L679-L698>
pub fn migrated_empty_account() -> Account {
    let mut account = Account::empty();
    let mut permissions = ledger::Permissions::user_default();
    permissions.set_verification_key.txn_version =
        ledger::scan_state::currency::TxnVersion::from_u32(
            ledger::TXN_VERSION_CURRENT.as_u32().saturating_add(1),
        );
    account.permissions = permissions;
    account
}

/// The root of a Merkle tree of `depth` whose leaves 0, 1, ... hash to
/// `leaves` and whose other leaves hash to `empty_leaf`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_ledger/ledger.ml#L96-L110>
pub fn merkle_root_of_leaves(leaves: &[Fp], depth: usize, empty_leaf: Fp) -> Fp {
    use ledger::TreeVersion;
    let mut empty = empty_leaf;
    let mut level = leaves.to_vec();
    for height in 0..depth {
        level = level
            .chunks(2)
            .map(|pair| {
                let right = pair.get(1).copied().unwrap_or(empty);
                ledger::V2::hash_node(height, pair[0], right)
            })
            .collect();
        empty = ledger::V2::hash_node(height, empty, empty);
    }
    level.first().copied().unwrap_or(empty)
}

/// How many accounts the root snarked ledger, a mask over a frozen next epoch
/// ledger, holds itself before it becomes a database of its own (PC3b): a
/// sixty-fourth of the ledger, at least 1,024. By then its accounts and the
/// hashes on their paths, about 3 to 4 KB each in a mask, cost about what a
/// second compacted hash matrix does.
fn root_detach_threshold(num_accounts: usize) -> usize {
    (num_accounts / 64).max(1024)
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L46-L62>
fn staking_delegate(account: &ledger::Account) -> Option<&CompressedPubKey> {
    if account.token_id.is_default() {
        account.delegate.as_ref()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use ledger::{AccountIndex, TokenId};
    use mina_core::constants::StopSlots;
    use mina_p2p_messages::v2::MinaBaseLedgerHash0StableV1;

    use crate::ledger::hash_node_at_depth;

    use super::*;

    /// OCaml 4.0.2 masking_merkle_tree.ml:536-542 registers the derived
    /// token for EVERY account on insertion, even with no token holders.
    /// Expected owner below is that inserted account, not a lookup result.
    #[test]
    fn token_owner_includes_unheld_tokens_and_tracks_ledger_snapshots() {
        let key = CompressedPubKey::from_address(
            "B62qnzbXmRNo9q32n4SNu2mpB8e7FYYLH8NmaX6oFCBYjjQ8SbD7uzV",
        )
        .unwrap();
        let owner = Account::create_with(
            AccountId::new(key.clone(), TokenId::default()),
            ledger::scan_state::currency::Balance::from_u64(10101),
        );
        let derived = owner.id().derive_token_id();
        let other = Account::create_with(
            AccountId::new(key, TokenId::from(2u64)),
            ledger::scan_state::currency::Balance::from_u64(1),
        );
        let other_token = other.id().derive_token_id();
        for indexed in [false, true] {
            let mut mask = Mask::new_root(Database::create(LEDGER_DEPTH as u8));
            if indexed {
                mask.set_token_owners();
            } else {
                mask.unset_token_owners();
            }
            assert_eq!(mask.has_token_owners(), indexed);
            mask.get_or_create_account(owner.id(), owner.clone())
                .unwrap();
            // The only account holds MINA, not the derived token.
            assert_ne!(owner.token_id, derived);
            let first = LedgerHash::from_fp(mask.merkle_root());
            let mut ctx = LedgerCtx::default();
            ctx.snarked_ledgers.insert(first.clone(), mask.clone());
            assert_eq!(
                ctx.token_owner_for_rpc(first.clone(), derived.clone()),
                vec![owner.clone()],
                "indexed={indexed}: an owner exists even before any holder"
            );
            for _ in 0..2 {
                assert!(ctx
                    .token_owner_for_rpc(first.clone(), other_token.clone())
                    .is_empty());
                assert!(ctx
                    .token_owner_for_rpc(first.clone(), TokenId::default())
                    .is_empty());
                assert_eq!(
                    ctx.token_owner_for_rpc(first.clone(), derived.clone()),
                    vec![owner.clone()]
                );
            }
            if !indexed {
                let (hash, owners) = ctx.token_owners.as_ref().unwrap();
                assert_eq!(hash, &first);
                assert_eq!(owners.len(), 1, "one complete index also answers misses");
                assert_eq!(owners.get(&derived), Some(&owner.id()));
            }
            let mut next = mask.make_child();
            next.get_or_create_account(other.id(), other.clone())
                .unwrap();
            let second = LedgerHash::from_fp(next.merkle_root());
            ctx.snarked_ledgers.insert(second.clone(), next);
            assert_eq!(
                ctx.token_owner_for_rpc(second, other_token.clone()),
                vec![other.clone()]
            );
            // A cached owner from another ledger must not leak into a fork.
            assert!(ctx
                .token_owner_for_rpc(first.clone(), other_token.clone())
                .is_empty());
            assert_eq!(
                ctx.token_owner_for_rpc(first, derived.clone()),
                vec![owner.clone()]
            );
        }
    }

    /// A sync answer of child accounts that doesn't hash to the expected
    /// subtree hash changes nothing, so the correct, shorter answer after it
    /// still matches.
    #[test]
    fn mismatched_sync_accounts_leave_the_ledger_unchanged() {
        let key = CompressedPubKey::from_address(
            "B62qnzbXmRNo9q32n4SNu2mpB8e7FYYLH8NmaX6oFCBYjjQ8SbD7uzV",
        )
        .unwrap();
        let accounts: Vec<Account> = (0..5u64)
            .map(|i| {
                Account::create_with(
                    AccountId::new(key.clone(), TokenId::from(i + 1)),
                    ledger::scan_state::currency::Balance::from_u64(i + 1),
                )
            })
            .collect();
        let mut synced = Mask::new_root(Database::create(LEDGER_DEPTH as u8));
        for account in &accounts[..3] {
            synced
                .get_or_create_account(account.id(), account.clone())
                .unwrap();
        }
        let parent = LedgerAddress::first(LEDGER_DEPTH - 6);
        let expected = LedgerHash::from_fp(synced.get_inner_hash_at_addr(parent.clone()).unwrap());
        let root = LedgerHash::from_fp(synced.merkle_root());
        let wire = |accounts: &[Account]| accounts.iter().map(Into::into).collect::<Vec<_>>();

        let mut ctx = LedgerCtx::default();
        ctx.sync.snarked_ledger_mut(root.clone()).unwrap();
        let answer = ctx
            .accounts_set(root.clone(), &parent, &expected, wire(&accounts))
            .unwrap();
        assert_ne!(answer, expected);
        let mask = ctx.pending_sync_snarked_ledger_mask(&root).unwrap();
        assert!(mask.get_all_accounts_rooted_at(parent.clone()).is_none());

        let answer = ctx
            .accounts_set(root.clone(), &parent, &expected, wire(&accounts[..3]))
            .unwrap();
        assert_eq!(answer, expected);
        let mask = ctx.pending_sync_snarked_ledger_mask(&root).unwrap();
        assert_eq!(mask.get_all_accounts_rooted_at(parent).unwrap().len(), 3);
    }

    #[test]
    fn historical_aux_survives_mask_pruning_and_eviction_is_terminal() {
        use crate::transition_frontier::root_history::RootHistory;
        use mina_p2p_messages::rpc_kernel::PayloadBinprotReader;
        let bytes = include_bytes!("../../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin");
        let captured =
            mina_p2p_messages::rpc::GetBestTipV2::response_payload(&mut bytes.as_slice())
                .unwrap()
                .unwrap()
                .data;
        let root = mina_core::block::BlockWithHash::try_new(Arc::new(captured)).unwrap();
        let mut ctx = LedgerCtx::default();
        // OCaml's empty scan state has no required protocol states. This tests
        // retirement/pruning without claiming this mask matches the captured block.
        let mut ledger =
            StagedLedger::create_exn(constraint_constants().clone(), Mask::create(35)).unwrap();
        ledger.pending_coinbase_collection_merkle_root();
        let scan: v2::TransactionSnarkScanStateStableV2 = ledger.scan_state().into();
        let pending: MinaBasePendingCoinbaseStableV2 = ledger.pending_coinbase_collection().into();
        ctx.staged_ledgers
            .staged_ledgers
            .insert(Arc::new(root.staged_ledger_hashes().clone()), ledger);
        let history = RootHistory(vec![root.clone()]);
        ctx.retain_root_history(&history, &BTreeMap::new());
        ctx.staged_ledgers.staged_ledgers.clear();
        assert!(ctx.staged_ledger_mut(root.staged_ledger_hashes()).is_none());
        let answer = ctx
            .staged_ledger_aux_and_pending_coinbase(
                root.hash(),
                root.staged_ledger_hashes(),
                BTreeMap::new(),
            )
            .unwrap();
        assert_eq!(answer.scan_state, scan);
        assert_eq!(answer.pending_coinbase, pending);
        assert!(answer.needed_blocks.is_empty());
        assert!(ctx
            .staged_ledger_aux_and_pending_coinbase(
                root.pred_hash(),
                root.staged_ledger_hashes(),
                BTreeMap::new()
            )
            .is_none());
        ctx.retain_root_history(&RootHistory::default(), &BTreeMap::new());
        assert!(ctx.root_history.is_empty());
        assert!(ctx.root_history_protocol_states.is_empty());
        assert!(ctx
            .staged_ledger_aux_and_pending_coinbase(
                root.hash(),
                root.staged_ledger_hashes(),
                BTreeMap::new()
            )
            .is_none());
    }

    /// Use an OCaml-sourced reconstruction dump to compare all four response
    /// components, exercise missing states, and measure shared snapshot overhead.
    #[test]
    #[ignore = "requires MINA_ROOT_HISTORY_DUMP from an OCaml bootstrap"]
    fn historical_aux_matches_ocaml_dump_and_measures_window() {
        use crate::transition_frontier::root_history::RootHistory;
        use mina_p2p_messages::{
            binprot::{self, macros::BinProtRead, BinProtWrite},
            rpc_kernel::PayloadBinprotReader,
        };
        #[derive(BinProtRead)]
        struct Dump {
            _accounts: Vec<v2::MinaBaseAccountBinableArgStableV2>,
            scan_state: v2::TransactionSnarkScanStateStableV2,
            pending_coinbase: MinaBasePendingCoinbaseStableV2,
            staged_ledger_hash: LedgerHash,
            states: List<MinaStateProtocolStateValueStableV2>,
        }
        fn rss() -> u64 {
            std::fs::read_to_string("/proc/self/status")
                .unwrap()
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<u64>()
                .unwrap()
                * 1024
        }
        let path = std::env::var("MINA_ROOT_HISTORY_DUMP").unwrap();
        let dump = Dump::binprot_read(&mut std::io::BufReader::new(
            std::fs::File::open(path).unwrap(),
        ))
        .unwrap();
        let bytes = include_bytes!("../../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin");
        let captured =
            mina_p2p_messages::rpc::GetBestTipV2::response_payload(&mut bytes.as_slice())
                .unwrap()
                .unwrap()
                .data;
        let mut root_block = captured.clone();
        root_block
            .header
            .protocol_state
            .body
            .blockchain_state
            .staged_ledger_hash
            .non_snark
            .ledger_hash = dump.staged_ledger_hash.clone();
        let root = mina_core::block::BlockWithHash::try_new(Arc::new(root_block)).unwrap();
        let states: BTreeMap<_, _> = dump
            .states
            .iter()
            .map(|s| (s.try_hash().unwrap(), s.clone()))
            .collect();
        let mut ctx = LedgerCtx::default();
        // The account mask is deliberately empty: auxiliary serving must never
        // read it. The three payload components and states come from OCaml.
        let ledger = StagedLedger::from_persisted_unchecked(
            constraint_constants(),
            (&dump.scan_state).try_into().unwrap(),
            Mask::create(35),
            (&dump.pending_coinbase).try_into().unwrap(),
        );
        ctx.staged_ledgers
            .staged_ledgers
            .insert(Arc::new(root.staged_ledger_hashes().clone()), ledger);
        let mut history = RootHistory(vec![root.clone()]);
        ctx.retain_root_history(&history, &states);
        let before = rss();
        // Same-payload snapshots isolate per-root structure cost; this is not
        // an upper bound for 300 distinct roots' unique transaction witnesses.
        for height in 1..300 {
            let mut block = root.block.as_ref().clone();
            block
                .header
                .protocol_state
                .body
                .consensus_state
                .blockchain_length = height.into();
            history
                .0
                .push(mina_core::block::BlockWithHash::try_new(Arc::new(block)).unwrap());
        }
        ctx.retain_root_history(&history, &states);
        let after = rss();
        println!(
            "root history: entries={}, additional_299_snapshot_rss_bytes={}, required_states={}",
            ctx.root_history.len(),
            after.saturating_sub(before),
            ctx.root_history_protocol_states.len()
        );
        ctx.staged_ledgers.staged_ledgers.clear();
        let answer = ctx
            .staged_ledger_aux_and_pending_coinbase(
                root.hash(),
                root.staged_ledger_hashes(),
                BTreeMap::new(),
            )
            .unwrap();
        let mut expected = Vec::new();
        dump.scan_state.binprot_write(&mut expected).unwrap();
        let mut actual = Vec::new();
        answer.scan_state.binprot_write(&mut actual).unwrap();
        assert_eq!(actual, expected);
        // The pending-coinbase indexes encode a map. Native HashMap iteration
        // can reorder that association list without changing its meaning/hash.
        let mut pending = answer.pending_coinbase.clone();
        let mut expected_pending = dump.pending_coinbase.clone();
        let mut indexes = pending.tree.indexes.into_iter().collect::<Vec<_>>();
        indexes.sort_by_key(|(id, _)| id.0.as_u64());
        pending.tree.indexes = indexes.into_iter().collect();
        let mut indexes = expected_pending
            .tree
            .indexes
            .into_iter()
            .collect::<Vec<_>>();
        indexes.sort_by_key(|(id, _)| id.0.as_u64());
        expected_pending.tree.indexes = indexes.into_iter().collect();
        assert_eq!(pending, expected_pending);
        assert_eq!(answer.staged_ledger_hash, dump.staged_ledger_hash);
        let expected_states: List<_> = states.values().rev().cloned().collect();
        assert_eq!(answer.needed_blocks, expected_states);
        assert!(!ctx.root_history_protocol_states.is_empty());
        let missing = ctx
            .root_history_protocol_states
            .first_key_value()
            .unwrap()
            .0
            .clone();
        ctx.root_history_protocol_states.remove(&missing);
        assert!(ctx
            .staged_ledger_aux_and_pending_coinbase(
                root.hash(),
                root.staged_ledger_hashes(),
                BTreeMap::new()
            )
            .is_none());
        ctx.retain_root_history(&RootHistory::default(), &BTreeMap::new());
        assert!(ctx.root_history.is_empty());
        assert!(ctx.root_history_protocol_states.is_empty());
        assert!(ctx
            .staged_ledger_aux_and_pending_coinbase(
                root.hash(),
                root.staged_ledger_hashes(),
                BTreeMap::new()
            )
            .is_none());
    }

    #[test]
    fn snark_pool_prover_lookup_uses_only_best_tip_default_token_account() {
        use crate::snark_pool::ProverAccount;
        let pk = CompressedPubKey::from_address(
            "B62qnzbXmRNo9q32n4SNu2mpB8e7FYYLH8NmaX6oFCBYjjQ8SbD7uzV",
        )
        .unwrap();
        let wire: v2::NonZeroCurvePoint = (&pk).into();
        for (token, can_receive, expected) in [
            (TokenId(Fp::from(2u64)), false, ProverAccount::Missing),
            (
                TokenId::default(),
                true,
                ProverAccount::Present { can_receive: true },
            ),
            (
                TokenId::default(),
                false,
                ProverAccount::Present { can_receive: false },
            ),
        ] {
            let mut account = Account::create_with(
                AccountId::new(pk.clone(), token),
                ledger::scan_state::currency::Balance::from_u64(0),
            );
            if !can_receive {
                account.permissions.access = ledger::AuthRequired::Impossible;
            }
            let mut mask = Mask::create(35);
            mask.get_or_create_account(account.id(), account).unwrap();
            let root = merkle_root(&mut mask);
            let mut ctx = LedgerCtx::default();
            ctx.snarked_ledgers.insert(root.clone(), mask.clone());
            // The same account in a snarked/epoch ledger cannot authorize work.
            assert!(ctx.snark_pool_prover_account(&root, &wire).is_err());
            let staged = StagedLedger::create_exn(constraint_constants().clone(), mask).unwrap();
            ctx.staged_ledgers.insert_by_recomputing_hash(staged);
            assert_eq!(
                ctx.snark_pool_prover_account(&root, &wire).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn coverage_rehearsal_on_empty_scan_state_keeps_the_ledger_unchanged() {
        use mina_p2p_messages::rpc_kernel::PayloadBinprotReader;
        let bytes = include_bytes!("../../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin");
        let block = mina_p2p_messages::rpc::GetBestTipV2::response_payload(&mut bytes.as_slice())
            .unwrap()
            .unwrap()
            .data;
        let parent = mina_core::block::BlockWithHash::try_new(Arc::new(block)).unwrap();
        let receiver = parent
            .header()
            .protocol_state
            .body
            .consensus_state
            .coinbase_receiver
            .clone();
        // Synthetic empty scan state: OCaml create_diff needs no completed
        // work for an empty tree and includes the ordinary coinbase. The
        // protocol-state context and public receiver come from OCaml's block.
        let mut ledger =
            StagedLedger::create_exn(constraint_constants().clone(), Mask::create(35)).unwrap();
        let before = ledger.hash();
        let key = parent.staged_ledger_hashes().clone();
        let mut ctx = LedgerCtx::default();
        ctx.staged_ledgers
            .staged_ledgers
            .insert(Arc::new(key.clone()), ledger);
        let request = crate::snark_pool::WorkCoverageRequest {
            time: redux::Timestamp::ZERO,
            slot: parent.global_slot_since_genesis() + 1,
            parent,
            coinbase_receiver: receiver,
            receiver_source: "test_receiver".into(),
            completed_snarks: BTreeMap::new(),
            transactions: Vec::new(),
        };
        for _ in 0..2 {
            let result = ctx.work_coverage(request.clone()).unwrap();
            assert_eq!(
                (
                    result.jobs_needed,
                    result.jobs_available,
                    result.available_prefix
                ),
                (0, 0, 0)
            );
            assert_eq!(
                (result.expected_transactions, result.expected_works),
                (0, 0)
            );
            assert!(result.coinbase_can_be_included);
            // Mesa mainnet and devnet OCaml profile: 360 MINA, multiplier 1.
            assert_eq!(result.expected_coinbase_nanomina, 360_000_000_000);
            assert_eq!(ctx.staged_ledgers.get_mut(&key).unwrap().hash(), before);
            assert_eq!(
                ctx.staged_ledgers
                    .get(&key)
                    .unwrap()
                    .ledger()
                    .num_accounts(),
                0
            );
        }
    }

    #[test]
    fn unset_stop_slots_skip_the_fork_offset() {
        // A slot below the offset must not error when no stop slots are set.
        assert_eq!(stop_slot_policy(StopSlots::default(), 10, 20), Ok(true));
        assert_eq!(stop_slot_policy(StopSlots::default(), 30, 20), Ok(true));
    }

    #[test]
    fn set_stop_slots_use_slots_since_the_fork() {
        let stops = StopSlots {
            slot_tx_end: Some(5),
            slot_chain_end: Some(8),
            ..Default::default()
        };
        assert!(stop_slot_policy(stops, 10, 20).is_err());
        assert_eq!(stop_slot_policy(stops, 24, 20), Ok(true));
        assert_eq!(stop_slot_policy(stops, 25, 20), Ok(false));
        assert_eq!(
            stop_slot_policy(stops, 28, 20),
            Err("reached slot_chain_end".to_string())
        );
    }

    #[test]
    fn replacing_staged_ledger_does_not_grow_root_index() {
        let ledger =
            StagedLedger::create_exn(constraint_constants().clone(), Mask::create(35)).unwrap();
        let mut storage = StagedLedgersStorage::default();
        for _ in 0..100 {
            storage.insert_by_recomputing_hash(ledger.clone());
        }
        assert_eq!(storage.staged_ledgers.len(), 1);
        assert_eq!(storage.by_merkle_root_hash.len(), 1);
        assert_eq!(
            storage.by_merkle_root_hash.values().next().unwrap().len(),
            1
        );
        storage.retain(|_| false);
        assert!(storage.staged_ledgers.is_empty());
        assert!(storage.by_merkle_root_hash.is_empty());
    }

    #[test]
    fn compact_snarked_ledger_with_retained_child() {
        let root = Mask::create(35);
        let parent = root.make_child();
        let mut child = parent.make_child();
        let mut account = Account::empty();
        account.balance = ledger::scan_state::currency::Balance::from_u64(42);
        let addr = ledger::Address::first(35);
        child.set(addr.clone(), Box::new(account.clone()));
        let mut ctx = LedgerCtx::default();
        ctx.snarked_ledgers.insert(LedgerHash::zero(), parent);

        // The older parent is not retained; the descendant must stay usable.
        ctx.compact_snarked_ledgers(&LedgersToKeep::default());
        assert_eq!(child.get_parent().unwrap().get_uuid(), root.get_uuid());
        assert_eq!(*child.get(addr).unwrap(), account);
        assert_eq!(child.num_accounts(), 1);
        assert_eq!(
            ctx.snarked_ledgers[&LedgerHash::zero()].get_uuid(),
            root.get_uuid()
        );
    }

    /// A commit moving the root (R38): the new snarked ledger goes into the
    /// old one, then the new root's staged ledger mask into the old root's.
    /// The root staged ledger stays a mask over the root snarked ledger, which
    /// takes none of the staged changes and has the new snarked ledger's hash.
    #[test]
    fn commit_keeps_the_root_staged_ledger_a_mask_over_the_snarked_ledger() {
        fn set_balance(mask: &mut Mask, index: u64, balance: u64) {
            let address = ledger::Address::from_index(AccountIndex(index), LEDGER_DEPTH);
            let mut account = mask.get(address.clone()).unwrap();
            account.balance = ledger::scan_state::currency::Balance::from_u64(balance);
            mask.set(address, account);
        }
        fn staged_ledger(mask: &Mask) -> (Arc<MinaBaseStagedLedgerHashStableV1>, StagedLedger) {
            let mut staged_ledger =
                StagedLedger::create_exn(constraint_constants().clone(), mask.clone()).unwrap();
            let hash = MinaBaseStagedLedgerHashStableV1::from(&staged_ledger.hash());
            (Arc::new(hash), staged_ledger)
        }

        let mut snarked = ledger_with(8);
        let old_snarked_hash = merkle_root(&mut snarked);
        // The old root's staged ledger, the new root's and the best tip's.
        let mut old_root = snarked.make_child();
        set_balance(&mut old_root, 0, 100);
        set_balance(&mut old_root, 1, 101);
        let mut new_root = old_root.make_child();
        set_balance(&mut new_root, 1, 201);
        set_balance(&mut new_root, 2, 202);
        let mut best_tip = new_root.make_child();
        set_balance(&mut best_tip, 3, 303);
        // The new root's snarked ledger: the old one with the old root's
        // transactions, as `push_snarked_ledger` makes it.
        let mut new_snarked = snarked.make_child();
        set_balance(&mut new_snarked, 0, 100);
        set_balance(&mut new_snarked, 1, 101);
        let new_snarked_hash = merkle_root(&mut new_snarked);
        let new_root_hash = merkle_root(&mut new_root);
        let best_tip_hash = merkle_root(&mut best_tip);

        let mut ctx = LedgerCtx::default();
        ctx.snarked_ledgers
            .insert(old_snarked_hash, snarked.clone());
        ctx.snarked_ledgers
            .insert(new_snarked_hash.clone(), new_snarked.clone());
        let mut keep = LedgersToKeep::new();
        keep.add_snarked(new_snarked_hash.clone());
        let mut new_root_staged = None;
        for (mask, kept) in [(&old_root, false), (&new_root, true), (&best_tip, true)] {
            let (hash, staged_ledger) = staged_ledger(mask);
            if kept {
                keep.add_staged(hash.clone());
            }
            if mask.get_uuid() == new_root.get_uuid() {
                new_root_staged = Some(hash.clone());
            }
            ctx.staged_ledgers.insert(hash, staged_ledger);
        }
        let new_root_staged = new_root_staged.unwrap();

        // `commit`'s steps on the ledgers.
        ctx.drop_snarked_ledgers(&keep);
        ctx.staged_ledgers.retain(|hash| keep.contains(hash));
        ctx.compact_snarked_ledgers(&keep);
        assert!(ctx.commit_root_staged_ledger(&new_root_staged, &new_snarked_hash));

        let mut root_snarked = ctx.snarked_ledgers[&new_snarked_hash].clone();
        assert_eq!(root_snarked.get_uuid(), snarked.get_uuid());
        assert_eq!(merkle_root(&mut root_snarked), new_snarked_hash);
        assert!(!new_snarked.is_attached());

        let mut root_staged = ctx.staged_ledgers.get(&new_root_staged).unwrap().ledger();
        assert_eq!(root_staged.get_uuid(), old_root.get_uuid());
        assert_eq!(
            root_staged.get_parent().unwrap().get_uuid(),
            snarked.get_uuid()
        );
        assert_eq!(merkle_root(&mut root_staged), new_root_hash);
        assert!(!new_root.is_attached());

        assert_eq!(
            best_tip.get_parent().unwrap().get_uuid(),
            old_root.get_uuid()
        );
        assert_eq!(merkle_root(&mut best_tip), best_tip_hash);
    }

    #[test]
    fn compact_snarked_ledger_preserves_inherited_and_child_accounts() {
        let mut root = Mask::create(35);
        let mut parent = root.make_child();
        let mut child = parent.make_child();
        let sibling = parent.make_child();
        let mut accounts = Vec::new();
        for (index, mask) in [&mut root, &mut parent, &mut child].into_iter().enumerate() {
            let mut account = Account::empty();
            account.token_id = TokenId::from(index as u64 + 1);
            let address = ledger::Address::from_index(AccountIndex(index as u64), 35);
            mask.set(address.clone(), Box::new(account.clone()));
            accounts.push((address, account));
        }
        let parent_id = parent.get_uuid();
        let parent_hash = merkle_root(&mut parent);
        let mut ctx = LedgerCtx::default();
        ctx.snarked_ledgers.insert(parent_hash.clone(), parent);

        ctx.compact_snarked_ledgers(&LedgersToKeep::default());
        assert!(!ledger::mask::is_alive(&parent_id));
        assert_eq!(child.get_parent().unwrap().get_uuid(), root.get_uuid());
        assert_eq!(sibling.get_parent().unwrap().get_uuid(), root.get_uuid());
        // OCaml Attached.last_filled takes the maximum of local and parent
        // locations. Compaction must preserve both inherited and local data.
        assert_eq!(root.num_accounts(), 2);
        assert_eq!(sibling.num_accounts(), 2);
        assert_eq!(child.num_accounts(), 3);
        for (address, account) in accounts {
            assert_eq!(*child.get(address).unwrap(), account);
        }
        assert_eq!(
            ctx.snarked_ledgers[&parent_hash].get_uuid(),
            root.get_uuid()
        );
    }

    #[test]
    fn only_explicit_default_token_delegations_count_as_stake() {
        // OCaml compute_delegatee_table requires both predicates, even if a
        // non-default token account happens to name the producer as delegate.
        let mut account = ledger::Account::create_with(
            ledger::AccountId::new_with_default_token(
                crate::account::AccountSecretKey::genesis_producer().public_key_compressed(),
            ),
            ledger::scan_state::currency::Balance::from_u64(1_000_000_000),
        );
        assert_eq!(staking_delegate(&account), Some(&account.public_key));
        account.delegate = None;
        assert!(staking_delegate(&account).is_none());
        account.delegate = Some(account.public_key.clone());
        account.token_id = ledger::TokenId::from(2);
        assert!(staking_delegate(&account).is_none());
    }

    #[test]
    fn test_ledger_hash() {
        IntoIterator::into_iter([(
            LedgerAddress::root(),
            "jx5YAT36bv62M8mPcREYYfZWXaKqqMzDCP8wmc21uf4CfDKAHCr",
            "jxo5pSyt16XGwA9UeuAdiFDzrwFH3smbNTJF7fxq98w1y9Jem2m",
            "jwq3nCDr8XejL8HKDxR5qVhFJbKoUTGZgtLBZCp3MrqLTnqmjdP",
        )])
        .map(|(addr, expected_hash, left, right)| {
            let left: LedgerHash = left.parse().unwrap();
            let right: LedgerHash = right.parse().unwrap();
            (addr, expected_hash, left, right)
        })
        .for_each(|(address, expected_hash, left, right)| {
            let hash = hash_node_at_depth(
                address.length(),
                left.0.to_field().unwrap(),
                right.0.to_field().unwrap(),
            );
            let hash: LedgerHash = MinaBaseLedgerHash0StableV1(hash.into()).into();
            assert_eq!(hash.to_string(), expected_hash);
        });
    }

    /// Like OCaml's `num_accounts_answer`, a ledger of one account answers
    /// `Num_accounts` with the account's leaf hash: height `Int.ceil_log2 1`,
    /// which is 0. And like `handle_num_accounts`, completing that hash with
    /// empty subtrees gives the root.
    #[test]
    fn test_num_accounts_one_account() {
        // mina-tree's test-only `Account::create()`.
        let public_key = CompressedPubKey::from_address(
            "B62qnzbXmRNo9q32n4SNu2mpB8e7FYYLH8NmaX6oFCBYjjQ8SbD7uzV",
        )
        .unwrap();
        let account = Account::create_with(
            AccountId::new(public_key, TokenId::default()),
            ledger::scan_state::currency::Balance::from_u64(10101),
        );
        let leaf = LedgerHash::from_fp(account.hash());
        let mut mask = Mask::new_root(Database::create(LEDGER_DEPTH as u8));
        mask.get_or_create_account(account.id(), account).unwrap();
        let root = LedgerHash::from_fp(mask.merkle_root());
        // OCaml 4.0.2's `mina ledger hash` of this ledger, mainnet profile (I7).
        assert_eq!(
            root.to_string(),
            "jxwqr9HpSsaM8zoMoszLFooftiQpejdKWqc3vVPzgGgpfyPZCBT"
        );

        let mut ctx = LedgerCtx::default();
        ctx.snarked_ledgers.insert(root.clone(), mask);
        assert_eq!(ctx.get_num_accounts(root.clone()), Some((1, leaf.clone())));
        assert_eq!(
            crate::ledger::complete_num_accounts_tree_with_empties(&leaf, 1).unwrap(),
            root
        );
    }

    #[test]
    fn network_queries_use_complete_additional_ledger_despite_partial_sync_mask() {
        let key = CompressedPubKey::from_address(
            "B62qnzbXmRNo9q32n4SNu2mpB8e7FYYLH8NmaX6oFCBYjjQ8SbD7uzV",
        )
        .unwrap();
        let account = Account::create_with(
            AccountId::new(key.clone(), TokenId::default()),
            ledger::scan_state::currency::Balance::from_u64(10101),
        );
        let mut complete = Mask::create(LEDGER_DEPTH);
        complete
            .get_or_create_account(account.id(), account)
            .unwrap();
        let hash: LedgerHash = complete.merkle_root().into();
        let mut ctx = LedgerCtx::default();
        ctx.sync.snarked_ledger_mut(hash.clone()).unwrap();
        ctx.additional_snarked_ledgers
            .insert(hash.clone(), complete);
        // Local synchronization still needs the working mask, not the snapshot.
        let (local, synced) = ctx.mask(&hash).unwrap();
        assert!(!synced);
        assert_eq!(local.num_accounts(), 0);
        for request in [
            LedgerReadRequest::GetNumAccounts(hash.clone()),
            LedgerReadRequest::GetChildHashesAtAddr(hash.clone(), LedgerAddress::root(), 1),
            LedgerReadRequest::GetChildAccountsAtAddr(hash, LedgerAddress::first(29)),
        ] {
            match LedgerRequest::read_for_tests(&mut ctx, request) {
                LedgerReadResponse::GetNumAccounts(Some((count, _))) => assert_eq!(count, 1),
                LedgerReadResponse::GetChildHashesAtAddr(Some(hashes)) => {
                    assert_eq!(hashes.len(), 2)
                }
                LedgerReadResponse::GetChildAccountsAtAddr(Ok(accounts)) => {
                    assert_eq!(accounts.len(), 1);
                    assert_eq!(accounts[0].public_key, (&key).into());
                }
                response => panic!("complete additional ledger was refused: {response:?}"),
            }
        }
    }

    /// An epoch ledger of `n` accounts, a third of them delegating to
    /// `producer`, some of those with another token (not stake).
    fn epoch_ledger(n: usize, producer: &CompressedPubKey) -> Mask {
        let mut mask = Mask::create(LEDGER_DEPTH);
        for i in 0..n {
            let mut account = Account::rand();
            if i % 3 == 0 {
                account.delegate = Some(producer.clone());
                if i % 6 == 0 {
                    account.token_id = TokenId::default();
                }
            }
            mask.get_or_create_account(account.id(), account).unwrap();
        }
        mask
    }

    /// PC3a: what reads the epoch ledgers gets the same answers once they are
    /// on disk: serving syncing peers (account count, child hashes, account
    /// ranges), the VRF evaluator's delegator table, the delegator RPCs, RPC
    /// account reads and the frontier store's full file, byte for byte.
    #[test]
    fn epoch_ledgers_on_disk_answer_as_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let producer = Account::rand().public_key;
        let staking = epoch_ledger(700, &producer);
        let mut memory_copy = staking.copy();
        let hash = LedgerHash::from_fp(memory_copy.merkle_root());

        let mut memory = LedgerCtx::default();
        memory.snarked_ledgers.insert(hash.clone(), memory_copy);
        let mut disk = LedgerCtx::default();
        disk.set_ledger_disk_dir(dir.path().to_owned());
        disk.snarked_ledgers.insert(hash.clone(), staking);
        disk.freeze_snarked_ledgers(BTreeSet::from([&hash, &ledger_hash(MISSING)]));
        assert_eq!(disk.snarked_ledgers[&hash].disk_state(), Some(0));
        assert!(memory.snarked_ledgers[&hash].disk_state().is_none());
        // Delegator lookups use the index on disk, a scan in memory.
        assert!(disk.delegate_index.contains_key(&hash));
        assert!(memory.delegate_index.is_empty());

        let ids = {
            let mut ids = vec![Account::rand().id()];
            memory.snarked_ledgers[&hash].iter(|account| ids.push(account.id()));
            ids
        };
        fn both<T: PartialEq + std::fmt::Debug>(
            a: &mut LedgerCtx,
            b: &mut LedgerCtx,
            f: impl Fn(&mut LedgerCtx) -> T,
        ) {
            assert_eq!(f(a), f(b));
        }
        both(&mut memory, &mut disk, |ctx| {
            ctx.get_num_accounts(hash.clone())
        });
        for (addr, depth) in [
            (LedgerAddress::root(), 1),
            (LedgerAddress::first(20), 6),
            (LedgerAddress::first(29), 6),
            (LedgerAddress::first(33), 2),
        ] {
            both(&mut memory, &mut disk, |ctx| {
                ctx.get_child_hashes_for_serving(hash.clone(), addr.clone(), depth)
            });
        }
        for index in [0u64, 1, 10] {
            let addr = LedgerAddress::from_index(AccountIndex(index), 29);
            both(&mut memory, &mut disk, |ctx| {
                ctx.get_child_accounts(hash.clone(), addr.clone())
            });
        }
        both(&mut memory, &mut disk, |ctx| {
            ctx.get_accounts(hash.clone(), ids.clone())
        });
        let producer_key = AccountPublicKey::from(producer.clone());
        both(&mut memory, &mut disk, |ctx| {
            ctx.producer_delegator_table(&hash, &producer_key)
        });
        let table = disk.producer_delegator_table(&hash, &producer_key).unwrap();
        assert_eq!(table.len(), 117);
        both(&mut memory, &mut disk, |ctx| {
            ctx.producers_with_delegates(&hash, |key| key == &producer)
        });
        let producer_id = AccountId::new(producer.clone(), TokenId::default());
        both(&mut memory, &mut disk, |ctx| {
            ctx.get_account_delegators(&hash, &producer_id)
        });
        let mut memory_file = vec![];
        crate::persist::ledger_codec::write_full(&mut memory_file, &memory.snarked_ledgers[&hash])
            .unwrap();
        let mut disk_file = vec![];
        crate::persist::ledger_codec::write_full(&mut disk_file, &disk.snarked_ledgers[&hash])
            .unwrap();
        assert!(memory_file == disk_file, "frontier store files differ");
    }

    /// PC3a at an epoch boundary: the next epoch ledger is a mask over the
    /// frozen staking ledger it replaces. The commit folds it in, and the
    /// staking ledger is written again with the changes. A sync whose target
    /// is the committed ledger copies it, sharing the file (W5).
    #[test]
    fn epoch_boundary_folds_into_a_frozen_staking_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let producer = Account::rand().public_key;
        let mut old_staking = epoch_ledger(200, &producer);
        let old_hash = LedgerHash::from_fp(old_staking.merkle_root());
        let mut ctx = LedgerCtx::default();
        ctx.set_ledger_disk_dir(dir.path().to_owned());
        // Write a frozen ledger again at 4 changes (PC3c: by default a
        // sixteenth of the accounts, at least 4,096).
        ctx.root_disk_limits = Some((4, 16));
        ctx.snarked_ledgers
            .insert(old_hash.clone(), old_staking.clone());
        ctx.freeze_snarked_ledgers(BTreeSet::from([&old_hash]));

        let mut next = old_staking.make_child();
        let mut expected = old_staking.copy();
        for index in [0u64, 5, 199] {
            let mut account = next.get_at_index(AccountIndex(index)).unwrap();
            account.balance = ledger::scan_state::currency::Balance::from_u64(index + 7);
            next.set_at_index(AccountIndex(index), account.clone())
                .unwrap();
            expected.set_at_index(AccountIndex(index), account).unwrap();
        }
        let added = Account::rand();
        next.get_or_create_account(added.id(), added.clone())
            .unwrap();
        expected.get_or_create_account(added.id(), added).unwrap();
        let next_hash = LedgerHash::from_fp(next.merkle_root());
        assert_eq!(next_hash, LedgerHash::from_fp(expected.merkle_root()));
        ctx.snarked_ledgers.insert(next_hash.clone(), next);

        let mut keep = LedgersToKeep::default();
        keep.add_snarked(next_hash.clone());
        ctx.drop_snarked_ledgers(&keep);
        ctx.compact_snarked_ledgers(&keep);
        let mut staking = ctx.snarked_ledgers[&next_hash].clone();
        assert!(staking.is_root());
        assert_eq!(staking.disk_state(), Some(4));
        assert!(ctx.delegate_index.contains_key(&old_hash));
        ctx.freeze_snarked_ledgers(BTreeSet::from([&next_hash]));
        assert_eq!(staking.disk_state(), Some(0));
        // The old ledger's delegate index went with it; the new one has its own.
        assert!(!ctx.delegate_index.contains_key(&old_hash));
        assert!(ctx.delegate_index.contains_key(&next_hash));
        assert_eq!(LedgerHash::from_fp(staking.merkle_root()), next_hash);
        assert_eq!(staking.to_list(), expected.to_list());
        // The old file went with the old staking ledger's last clones.
        drop((old_staking, expected));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

        assert!(ctx
            .copy_snarked_ledger_contents_for_sync(&[], next_hash.clone(), false)
            .unwrap());
        let (mut copy, _) = ctx.sync.mask(&next_hash).unwrap();
        assert_eq!(copy.disk_state(), Some(0));
        assert_eq!(LedgerHash::from_fp(copy.merkle_root()), next_hash);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    fn set_balances(mask: &mut Mask, changes: impl IntoIterator<Item = (u64, u64)>) {
        for (index, balance) in changes {
            let mut account = mask.get_at_index(AccountIndex(index)).unwrap();
            account.balance = ledger::scan_state::currency::Balance::from_u64(balance);
            mask.set_at_index(AccountIndex(index), account).unwrap();
        }
    }

    /// A bootstrap's three ledgers, as `copy_snarked_ledger_contents_for_sync`
    /// starts them: the staking ledger synced in `ctx`'s sync ledgers, then
    /// the next epoch and root ledgers each started from the one before and
    /// synced (their `changes` set), with in-memory twins of what each must
    /// hold. Returns the hashes and the twins.
    fn bootstrap(
        ctx: &mut LedgerCtx,
        producer: &CompressedPubKey,
        changes: [Vec<(u64, u64)>; 2],
    ) -> ([LedgerHash; 3], [Mask; 3]) {
        let staking = epoch_ledger(300, producer);
        let staking_hash = LedgerHash::from_fp(staking.clone().merkle_root());
        let mut next = staking.copy();
        set_balances(&mut next, changes[0].clone());
        let next_hash = merkle_root(&mut next);
        let mut root = next.copy();
        set_balances(&mut root, changes[1].clone());
        let root_hash = merkle_root(&mut root);
        let twins = [staking.copy(), next, root];
        ctx.sync
            .snarked_ledgers
            .insert(staking_hash.clone(), staking);
        let mut origin = staking_hash.clone();
        for (target, changes) in [&next_hash, &root_hash].into_iter().zip(changes) {
            let candidates = [origin.clone(), staking_hash.clone()];
            assert!(ctx
                .copy_snarked_ledger_contents_for_sync(&candidates, target.clone(), false)
                .unwrap());
            let mut synced = ctx.sync.snarked_ledgers[target].clone();
            set_balances(&mut synced, changes);
            assert_eq!(&merkle_root(&mut synced), target);
            origin = target.clone();
        }
        ([staking_hash, next_hash, root_hash], twins)
    }

    /// The bootstrap's commit, as far as the ledgers go: the sync ledgers
    /// become the committed ones and go to disk.
    fn bootstrap_commit(ctx: &mut LedgerCtx, [staking, next, root]: &[LedgerHash; 3]) {
        let synced = std::mem::take(&mut ctx.sync.snarked_ledgers);
        ctx.snarked_ledgers.extend(synced);
        ctx.freeze_snarked_ledgers(BTreeSet::from([staking, next]));
        ctx.freeze_root_snarked_ledger_hash(root);
    }

    fn assert_holds(ctx: &LedgerCtx, hashes: &[LedgerHash; 3], twins: &[Mask; 3]) {
        for (hash, twin) in hashes.iter().zip(twins) {
            let mut mask = ctx.snarked_ledgers[hash].clone();
            assert_eq!(&merkle_root(&mut mask), hash);
            assert_eq!(mask.to_list(), twin.to_list(), "{hash}");
            let mut files = [vec![], vec![]];
            for (file, mask) in files.iter_mut().zip([&mask, twin]) {
                crate::persist::ledger_codec::write_full(file, mask).unwrap();
            }
            assert!(files[0] == files[1], "{hash}: frontier store files differ");
        }
    }

    /// Store diffs can share their verified full base's frozen accounts,
    /// as the bootstrap does, while keeping independent roots and indexes.
    #[test]
    fn ledger_0201_restored_diffs_share_one_accounts_file() {
        use crate::persist::{
            ledger_codec::{write_diff, write_full},
            load_control::LoadControl,
            merkle_diff,
            store::{decode_ledgers_with_disk, LedgerFiles},
            VerifyMode,
        };
        let bootstrap_dir = tempfile::tempdir().unwrap();
        let mut bootstrapped = LedgerCtx::default();
        bootstrapped.set_ledger_disk_dir(bootstrap_dir.path().to_owned());
        let producer = Account::rand().public_key;
        let (hashes, twins) = bootstrap(
            &mut bootstrapped,
            &producer,
            [vec![(1, 10), (5, 20)], vec![(3, 30), (7, 40)]],
        );
        let files = || {
            let mut bytes = vec![Vec::new(), Vec::new(), Vec::new()];
            write_full(&mut bytes[0], &twins[0]).unwrap();
            for i in 1..3 {
                let diff = merkle_diff(&twins[i], &twins[i - 1]).unwrap();
                write_diff(&mut bytes[i], &diff).unwrap();
            }
            LedgerFiles::from_bytes(bytes)
        };
        let dir = tempfile::tempdir().unwrap();
        let (loaded, _, _) = decode_ledgers_with_disk(
            files(),
            VerifyMode::Full,
            &LoadControl::default(),
            Some(dir.path()),
        )
        .unwrap();
        let mut restored = LedgerCtx::default();
        restored.set_ledger_disk_dir(dir.path().to_owned());
        restored.snarked_ledgers = loaded;
        let [staking, next, root] = &hashes;
        restored.freeze_snarked_ledgers(BTreeSet::from([staking, next]));
        restored.freeze_root_snarked_ledger_hash(root);
        assert_holds(&restored, &hashes, &twins);
        for hash in &hashes {
            assert!(restored.snarked_ledgers[hash].is_root());
            restored.snarked_ledgers[hash]
                .validate_account_indexes()
                .unwrap();
        }
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "the restored ledgers should share their full base file"
        );
        // A separate bootstrap reaches the same contents and one file.
        bootstrap_commit(&mut bootstrapped, &hashes);
        assert_holds(&bootstrapped, &hashes, &twins);
        assert_eq!(std::fs::read_dir(bootstrap_dir.path()).unwrap().count(), 1);
        // Refreezing one restored root leaves both epochs and the bootstrap
        // unchanged. The shared file survives until its last owner leaves.
        let mut moved = restored.snarked_ledgers[root].clone();
        set_balances(&mut moved, [(9, 50)]);
        assert!(moved.freeze_to_disk(dir.path()).unwrap().unwrap().refreeze);
        for i in 0..2 {
            assert_eq!(
                restored.snarked_ledgers[&hashes[i]].to_list(),
                twins[i].to_list()
            );
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        drop((moved, restored));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        // A write failure is an optimization failure only; keep valid ledgers
        // in memory and allow the later commit to retry after its backoff.
        let (loaded, _, _) = decode_ledgers_with_disk(
            files(),
            VerifyMode::Full,
            &LoadControl::default(),
            Some(&dir.path().join("missing")),
        )
        .unwrap();
        for (hash, twin) in hashes.iter().zip(&twins) {
            assert_eq!(loaded[hash].disk_state(), None);
            assert_eq!(loaded[hash].to_list(), twin.to_list());
            loaded[hash].validate_account_indexes().unwrap();
        }
    }

    /// PC3c: a bootstrap holds one full ledger. The next epoch ledger syncs as
    /// a mask over the staking ledger and the root's over it, each holding
    /// only its differences. The commit moves the staking ledger to disk and
    /// makes the masks databases of their own, sharing its file until their
    /// first write. Each ledger holds what a full copy would, and is written
    /// to the frontier store byte for byte the same.
    #[test]
    fn a_bootstrap_syncs_its_later_ledgers_as_masks() {
        let dir = tempfile::tempdir().unwrap();
        let producer = Account::rand().public_key;
        let mut ctx = LedgerCtx::default();
        ctx.set_ledger_disk_dir(dir.path().to_owned());
        // Refreeze the root at 8 changed accounts, detach it at 16.
        ctx.root_disk_limits = Some((8, 16));
        let next_changes = (0..10).map(|i| (i * 7, i + 1)).collect::<Vec<_>>();
        let root_changes = (0..20).map(|i| (i * 11 + 1, i + 100)).collect::<Vec<_>>();
        let (hashes, twins) = bootstrap(&mut ctx, &producer, [next_changes, root_changes]);
        let [staking, next, root] = &hashes;

        // Masks, a line: staking <- next <- root, each with its own changes.
        let ledgers = &ctx.sync.snarked_ledgers;
        assert!(ledgers[staking].is_root());
        for (hash, parent, own) in [(next, staking, 10), (root, next, 20)] {
            let mask = &ledgers[hash];
            assert!(!mask.is_root(), "{hash}");
            assert_eq!(
                mask.get_parent().unwrap().get_uuid(),
                ledgers[parent].get_uuid()
            );
            assert_eq!(mask.num_own_accounts(), own);
        }
        // A ledger with something over it already is copied, so that a fold
        // into it can't change a ledger under another mask.
        let other = ledger_hash(MISSING);
        assert!(ctx
            .copy_snarked_ledger_contents_for_sync(&[staking.clone()], other.clone(), false)
            .unwrap());
        assert!(ctx.sync.snarked_ledgers[&other].is_root());
        ctx.sync.snarked_ledgers.remove(&other);
        assert_holds(
            &LedgerCtx {
                snarked_ledgers: ctx.sync.snarked_ledgers.clone(),
                ..Default::default()
            },
            &hashes,
            &twins,
        );

        bootstrap_commit(&mut ctx, &hashes);
        for hash in &hashes {
            let mask = &ctx.snarked_ledgers[hash];
            assert!(mask.is_root(), "{hash} is a database of its own");
            assert_eq!(mask.disk_state(), Some(0), "{hash} is on disk");
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
        // The root's parent link is gone with the detach; the next epoch
        // ledger's index answers delegators as a scan would.
        assert!(ctx.snarked_ledgers[root].get_parent().is_none());
        assert!(ctx.delegate_index.contains_key(next));
        let producer_key = AccountPublicKey::from(producer.clone());
        let scanned = {
            let mut memory = LedgerCtx::default();
            memory.snarked_ledgers.insert(next.clone(), twins[1].copy());
            memory.producer_delegator_table(next, &producer_key)
        };
        assert_eq!(ctx.producer_delegator_table(next, &producer_key), scanned);
        assert_holds(&ctx, &hashes, &twins);
    }

    /// PC3c: a detached epoch ledger with fewer changes than a refreeze needs
    /// writes no file: the bootstrap's commit writes the staking ledger's file
    /// only, and the next epoch and root ledgers read it, with their changes
    /// in memory. The next epoch ledger's delegate index, derived from the
    /// staking ledger's, answers as a scan of a full copy does.
    #[test]
    fn a_detached_epoch_ledger_writes_no_file_for_a_few_changes() {
        let dir = tempfile::tempdir().unwrap();
        let producer = Account::rand().public_key;
        let mut ctx = LedgerCtx::default();
        ctx.set_ledger_disk_dir(dir.path().to_owned());
        // Refreeze at 100 changes; detach the root at 16.
        ctx.root_disk_limits = Some((100, 16));
        let next_changes = (0..10).map(|i| (i * 7, i + 1)).collect::<Vec<_>>();
        let root_changes = (0..20).map(|i| (i * 11 + 1, i + 100)).collect::<Vec<_>>();
        let (hashes, twins) = bootstrap(&mut ctx, &producer, [next_changes, root_changes]);
        let [staking, next, root] = &hashes;
        bootstrap_commit(&mut ctx, &hashes);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(ctx.snarked_ledgers[staking].disk_state(), Some(0));
        assert_eq!(ctx.snarked_ledgers[next].disk_state(), Some(10));
        // The root's overlay starts as a copy of the next epoch ledger's (10)
        // and takes its own 20, one of them the same account.
        assert_eq!(ctx.snarked_ledgers[root].disk_state(), Some(29));
        assert_eq!(ctx.delegate_index[next].overlay, 10);
        let producer_key = AccountPublicKey::from(producer.clone());
        let mut memory = LedgerCtx::default();
        memory.snarked_ledgers.insert(next.clone(), twins[1].copy());
        for _ in 0..2 {
            assert_eq!(
                ctx.producer_delegator_table(next, &producer_key),
                memory.producer_delegator_table(next, &producer_key)
            );
            let id = AccountId::new(producer.clone(), TokenId::default());
            assert_eq!(
                ctx.get_account_delegators(next, &id),
                memory.get_account_delegators(next, &id)
            );
            // Another commit changes nothing and writes nothing.
            ctx.freeze_snarked_ledgers(BTreeSet::from([staking, next]));
            ctx.freeze_root_snarked_ledger_hash(root);
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        }
        assert!(ctx.delegate_index.contains_key(next));
        assert_holds(&ctx, &hashes, &twins);
    }

    /// PC3c review: a retarget copies an incomplete sync ledger that is a mask
    /// over the staking ledger. The commit keeps only the new target, so the
    /// fold of the new target into the dropped staking ledger must work: the
    /// copy is a database of its own, and the fold has nothing to do. It
    /// panicked (`Parent doesn't have this mask as child`) when the copy was a
    /// mask reading through the staking ledger unregistered.
    #[test]
    fn a_retargeted_sync_ledger_survives_a_commit_that_drops_its_origin() {
        let dir = tempfile::tempdir().unwrap();
        let producer = Account::rand().public_key;
        let staking = epoch_ledger(300, &producer);
        let staking_hash = LedgerHash::from_fp(staking.clone().merkle_root());
        let mut ctx = LedgerCtx::default();
        ctx.set_ledger_disk_dir(dir.path().to_owned());
        ctx.sync
            .snarked_ledgers
            .insert(staking_hash.clone(), staking);

        let old_target = ledger_hash(MISSING);
        assert!(ctx
            .copy_snarked_ledger_contents_for_sync(&[staking_hash], old_target.clone(), false)
            .unwrap());
        let mut incomplete = ctx.sync.snarked_ledgers[&old_target].clone();
        assert!(!incomplete.is_root());
        set_balances(&mut incomplete, [(0, 111)]);

        let mut expected = incomplete.copy();
        set_balances(&mut expected, [(1, 222)]);
        let new_target = merkle_root(&mut expected);
        assert!(ctx
            .copy_snarked_ledger_contents_for_sync(&[old_target], new_target.clone(), false)
            .unwrap());
        let mut retargeted = ctx.sync.snarked_ledgers[&new_target].clone();
        set_balances(&mut retargeted, [(1, 222)]);
        assert_eq!(merkle_root(&mut retargeted), new_target);

        let completed = ctx.sync.snarked_ledgers.remove(&new_target).unwrap();
        ctx.snarked_ledgers.insert(new_target.clone(), completed);
        let mut keep = LedgersToKeep::new();
        keep.add_snarked(new_target.clone());
        ctx.sync.snarked_ledgers.clear();
        ctx.compact_snarked_ledgers(&keep);
        let mut committed = ctx.snarked_ledgers[&new_target].clone();
        assert!(
            committed.is_root(),
            "an incomplete mask's copy is independent"
        );
        assert_eq!(merkle_root(&mut committed), new_target);
        assert_eq!(committed.to_list(), expected.to_list());
    }

    /// PC3c: with files that can't be written (a full disk, say), a bootstrap
    /// still completes: its ledgers stay as they are, in memory, and hold the
    /// same. Commits during the retry delay also complete in memory.
    #[test]
    fn a_bootstrap_whose_ledgers_cant_be_written_completes_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_dir = dir.path().join("not-a-directory");
        std::fs::write(&not_a_dir, b"").unwrap();
        let producer = Account::rand().public_key;
        let mut ctx = LedgerCtx::default();
        ctx.set_ledger_disk_dir(not_a_dir);
        ctx.root_disk_limits = Some((8, 16));
        let next_changes = (0..10).map(|i| (i * 7, i + 1)).collect::<Vec<_>>();
        let root_changes = (0..20).map(|i| (i * 11 + 1, i + 100)).collect::<Vec<_>>();
        let (hashes, twins) = bootstrap(&mut ctx, &producer, [next_changes, root_changes]);
        let [staking, next, root] = &hashes;

        bootstrap_commit(&mut ctx, &hashes);
        assert!(ctx.snarked_ledgers[staking].disk_state().is_none());
        assert!(
            !ctx.snarked_ledgers[next].is_root(),
            "nothing to detach from"
        );
        assert!(!ctx.snarked_ledgers[root].is_root());
        assert_holds(&ctx, &hashes, &twins);
        let producer_key = AccountPublicKey::from(producer.clone());
        let mut memory = LedgerCtx::default();
        memory.snarked_ledgers.insert(next.clone(), twins[1].copy());
        let scanned = memory.producer_delegator_table(next, &producer_key);
        assert_eq!(ctx.producer_delegator_table(next, &producer_key), scanned);

        // The disk is back at the next commit, but the retry deadline has
        // not elapsed. No files should be written yet. The database tests
        // advance the clock through the deadline and check eventual recovery.
        ctx.set_ledger_disk_dir(dir.path().to_owned());
        ctx.freeze_snarked_ledgers(BTreeSet::from([staking, next]));
        ctx.freeze_root_snarked_ledger_hash(root);
        for hash in &hashes {
            assert_eq!(ctx.snarked_ledgers[hash].disk_state(), None, "{hash}");
        }
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "only the original not-a-directory fixture remains"
        );
        assert_holds(&ctx, &hashes, &twins);
    }

    /// Every reader of an epoch ledger on disk whose file can't be read back,
    /// here cut to nothing, stops the node with status 1 and one line saying
    /// why; none answers from accounts it couldn't read, and none panics. By
    /// role: the producer's delegator table and stake proof; block
    /// application and diff creation on every node, which read the ledger
    /// under their masks; serving syncing peers; RPC and GraphQL accounts and
    /// delegators; the frontier store's full file; the epoch ledgers' and
    /// the root snarked ledger's refreeze, which copies the unchanged records
    /// (an error there would leave the node on a file it can't read). Each in
    /// a child process of this test binary: a panic there would poison the
    /// masks it unwinds through, and dropping one of them then aborts the
    /// process.
    #[test]
    fn every_reader_of_an_unreadable_ledger_file_stops_the_node_cleanly() {
        const CHILD: &str = "MINA_NODE_TEST_UNREADABLE_READER";
        const CHILD_DIR: &str = "MINA_NODE_TEST_UNREADABLE_READER_DIR";
        const READERS: [&str; 10] = [
            "VRF delegator table",
            "stake proof",
            "block application and diff creation, through a mask",
            "serving syncing peers",
            "RPC accounts by id",
            "RPC and GraphQL all accounts",
            "GraphQL delegators",
            "frontier store full file",
            "epoch ledger refreeze",
            "root snarked ledger refreeze",
        ];
        if let (Some(reader), Some(dir)) = (std::env::var(CHILD).ok(), std::env::var_os(CHILD_DIR))
        {
            ::ledger::database::disk::set_exit_on_unreadable(true);
            let producer = Account::rand().public_key;
            let staking = epoch_ledger(60, &producer);
            let ids = staking
                .to_list()
                .iter()
                .map(Account::id)
                .collect::<Vec<_>>();
            let delegator = staking
                .to_list()
                .into_iter()
                .find(|account| staking_delegate(account) == Some(&producer))
                .unwrap()
                .public_key;
            let hash = LedgerHash::from_fp(staking.clone().merkle_root());
            let mut ctx = LedgerCtx::default();
            ctx.set_ledger_disk_dir(dir.clone().into());
            ctx.snarked_ledgers.insert(hash.clone(), staking);
            ctx.freeze_snarked_ledgers(BTreeSet::from([&hash]));
            assert!(ctx.delegate_index.contains_key(&hash));
            if reader.ends_with("refreeze") {
                // A change, read and made while the file is whole, that the
                // next freeze writes again at once.
                let mut mask = ctx.snarked_ledgers[&hash].clone();
                let mut account = mask.get_at_index(AccountIndex(3)).unwrap();
                account.balance = ledger::scan_state::currency::Balance::from_u64(3);
                mask.set_at_index(AccountIndex(3), account).unwrap();
                ctx.root_disk_limits = Some((1, usize::MAX));
            }
            for entry in std::fs::read_dir(&dir).unwrap() {
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(entry.unwrap().path())
                    .unwrap();
                file.set_len(0).unwrap();
            }
            let producer_id = AccountId::new(producer.clone(), TokenId::default());
            match READERS.iter().position(|name| *name == reader) {
                Some(0) => {
                    ctx.producer_delegator_table(&hash, &AccountPublicKey::from(producer));
                }
                Some(1) => {
                    let _ = ctx.stake_proof_sparse_ledger(
                        &hash,
                        &v2::NonZeroCurvePoint::from(&producer),
                        &v2::NonZeroCurvePoint::from(&delegator),
                    );
                }
                Some(2) => {
                    let child = ctx.snarked_ledgers[&hash].make_child();
                    child.location_of_account(&ids[1]);
                }
                Some(3) => {
                    let addr = LedgerAddress::from_index(AccountIndex(0), 29);
                    let _ = ctx.get_child_accounts(hash.clone(), addr);
                }
                Some(4) => {
                    ctx.get_accounts(hash.clone(), ids.clone());
                }
                Some(5) => {
                    ctx.get_accounts_for_rpc(hash.clone(), None);
                }
                Some(6) => {
                    ctx.get_account_delegators(&hash, &producer_id);
                }
                Some(7) => {
                    let _ = crate::persist::ledger_codec::write_full(
                        &mut vec![],
                        &ctx.snarked_ledgers[&hash],
                    );
                }
                Some(8) => {
                    ctx.freeze_snarked_ledgers(BTreeSet::from([&hash]));
                }
                Some(9) => {
                    ctx.freeze_root_snarked_ledger_hash(&hash);
                }
                _ => panic!("no reader {reader}"),
            }
            unreachable!("{reader} answered from an unreadable file");
        }
        let name = format!(
            "{}::every_reader_of_an_unreadable_ledger_file_stops_the_node_cleanly",
            module_path!().split_once("::").unwrap().1
        );
        let dir = tempfile::tempdir().unwrap();
        for (i, reader) in READERS.iter().enumerate() {
            let child_dir = dir.path().join(i.to_string());
            std::fs::create_dir(&child_dir).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &name, "--nocapture", "--test-threads", "1"])
                .env(CHILD, reader)
                .env(CHILD_DIR, &child_dir)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(1), "{reader}: {stderr}");
            assert!(
                stderr.contains("Fatal: ledger accounts file"),
                "{reader}: {stderr}"
            );
            assert!(!stderr.contains("panicked"), "{reader}: {stderr}");
        }
    }

    /// The VRF evaluator's delegator table from a staking ledger whose file
    /// reads back other bytes than were written: the producer's node logs it
    /// and exits with status 1, for its supervisor to restart it, rather than
    /// unwind a panic through its ledger and stop answering. In a child
    /// process of this test binary.
    #[test]
    fn a_producer_whose_staking_ledger_file_is_unreadable_exits_cleanly() {
        const CHILD: &str = "MINA_NODE_TEST_UNREADABLE_DIR";
        if let Some(dir) = std::env::var_os(CHILD) {
            ::ledger::database::disk::set_exit_on_unreadable(true);
            let producer = Account::rand().public_key;
            let mut staking = epoch_ledger(60, &producer);
            let hash = LedgerHash::from_fp(staking.merkle_root());
            let mut ctx = LedgerCtx::default();
            ctx.set_ledger_disk_dir(dir.clone().into());
            ctx.snarked_ledgers.insert(hash.clone(), staking);
            ctx.freeze_snarked_ledgers(BTreeSet::from([&hash]));
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let bytes = std::fs::read(&path)
                    .unwrap()
                    .iter()
                    .map(|b| !b)
                    .collect::<Vec<_>>();
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .and_then(|mut file| std::io::Write::write_all(&mut file, &bytes))
                    .unwrap();
            }
            ctx.producer_delegator_table(&hash, &AccountPublicKey::from(producer));
            unreachable!("the table was made");
        }
        let dir = tempfile::tempdir().unwrap();
        let name = format!(
            "{}::a_producer_whose_staking_ledger_file_is_unreadable_exits_cleanly",
            module_path!().split_once("::").unwrap().1
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture", "--test-threads", "1"])
            .env(CHILD, dir.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains("Fatal: ledger accounts file"), "{stderr}");
        assert!(stderr.contains("checksum mismatch"), "{stderr}");
        assert!(!stderr.contains("panicked"), "{stderr}");
    }

    /// A stake proof for a staking ledger the ledger thread doesn't hold is an
    /// error, which fails that block, not a panic of the ledger thread.
    #[test]
    fn a_stake_proof_without_its_staking_ledger_is_an_error() {
        let mut ctx = LedgerCtx::default();
        let key = v2::NonZeroCurvePoint::from(&Account::rand().public_key);
        let error = ctx
            .stake_proof_sparse_ledger(&ledger_hash(MISSING), &key, &key)
            .unwrap_err();
        assert!(error.contains("missing"), "{error}");
    }

    /// The epoch boundary's fold, after another process removed the staking
    /// ledger's file, as a second node started with the same work directory
    /// does (it empties `ledger-accounts`): the fold scans the staking ledger
    /// for its delegate index and copies it to a new file, both through the
    /// descriptor the file was written with, so every node's commit, and a
    /// producer's next delegator table, go on.
    #[test]
    fn an_epoch_fold_reads_a_staking_ledger_file_another_process_removed() {
        let dir = tempfile::tempdir().unwrap();
        let producer = Account::rand().public_key;
        let mut old_staking = epoch_ledger(200, &producer);
        let old_hash = LedgerHash::from_fp(old_staking.merkle_root());
        let mut ctx = LedgerCtx::default();
        ctx.set_ledger_disk_dir(dir.path().to_owned());
        ctx.snarked_ledgers
            .insert(old_hash.clone(), old_staking.clone());
        ctx.freeze_snarked_ledgers(BTreeSet::from([&old_hash]));

        let mut next = old_staking.make_child();
        let mut expected = old_staking.copy();
        for index in [0u64, 5, 199] {
            let mut account = next.get_at_index(AccountIndex(index)).unwrap();
            account.balance = ledger::scan_state::currency::Balance::from_u64(index + 7);
            next.set_at_index(AccountIndex(index), account.clone())
                .unwrap();
            expected.set_at_index(AccountIndex(index), account).unwrap();
        }
        let next_hash = LedgerHash::from_fp(next.merkle_root());
        ctx.snarked_ledgers.insert(next_hash.clone(), next);
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }

        let mut keep = LedgersToKeep::default();
        keep.add_snarked(next_hash.clone());
        ctx.drop_snarked_ledgers(&keep);
        ctx.compact_snarked_ledgers(&keep);
        ctx.freeze_snarked_ledgers(BTreeSet::from([&next_hash]));
        let mut staking = ctx.snarked_ledgers[&next_hash].clone();
        assert_eq!(staking.disk_state(), Some(3));
        assert!(ctx.delegate_index.contains_key(&next_hash));
        assert_eq!(LedgerHash::from_fp(staking.merkle_root()), next_hash);
        let accounts = expected.to_list();
        assert_eq!(staking.to_list(), accounts);
        let delegators = accounts
            .iter()
            .filter(|account| staking_delegate(account) == Some(&producer))
            .count();
        let table = ctx
            .producer_delegator_table(&next_hash, &AccountPublicKey::from(producer))
            .unwrap();
        assert_eq!(table.len(), delegators);
    }

    /// PC3b: the root snarked ledger on disk through root moves and an epoch
    /// boundary, against a twin in memory doing the same. Root moves commit
    /// into the overlay and refreeze it at the threshold. After the boundary
    /// the root is a mask over the frozen next epoch ledger, until it holds
    /// enough to become a database of its own. At every step the root, the
    /// staged masks above it and what is served to syncing peers and written
    /// to the frontier store are the same as in memory, and the next epoch
    /// ledger doesn't change.
    #[test]
    fn root_snarked_ledger_on_disk_follows_root_moves_as_in_memory() {
        fn set_balances(mask: &mut Mask, changes: &[(u64, u64)]) {
            for (index, balance) in changes {
                let mut account = mask.get_at_index(AccountIndex(*index)).unwrap();
                account.balance = ledger::scan_state::currency::Balance::from_u64(*balance);
                mask.set_at_index(AccountIndex(*index), account).unwrap();
            }
        }
        /// A root move, as `push_snarked_ledger` and `commit` make it.
        fn root_move(
            ctx: &mut LedgerCtx,
            old: &LedgerHash,
            changes: &[(u64, u64)],
            epoch: &[LedgerHash],
        ) -> LedgerHash {
            let mut mt = ctx.snarked_ledgers[old].make_child();
            set_balances(&mut mt, changes);
            let new = merkle_root(&mut mt);
            ctx.snarked_ledgers.insert(new.clone(), mt);
            let mut keep = LedgersToKeep::new();
            keep.add_snarked(new.clone());
            for hash in epoch {
                keep.add_snarked(hash.clone());
            }
            ctx.drop_snarked_ledgers(&keep);
            ctx.compact_snarked_ledgers(&keep);
            ctx.freeze_snarked_ledgers(epoch.iter().filter(|hash| **hash != new).collect());
            ctx.freeze_root_snarked_ledger_hash(&new);
            new
        }
        fn same(
            memory: &mut LedgerCtx,
            disk: &mut LedgerCtx,
            hash: &LedgerHash,
            ids: &[AccountId],
        ) {
            assert_eq!(
                memory.get_num_accounts(hash.clone()),
                disk.get_num_accounts(hash.clone())
            );
            for (addr, depth) in [(LedgerAddress::root(), 1), (LedgerAddress::first(29), 6)] {
                assert_eq!(
                    memory.get_child_hashes_for_serving(hash.clone(), addr.clone(), depth),
                    disk.get_child_hashes_for_serving(hash.clone(), addr, depth)
                );
            }
            for index in [0u64, 2, 4] {
                let addr = LedgerAddress::from_index(AccountIndex(index), 29);
                assert_eq!(
                    memory.get_child_accounts(hash.clone(), addr.clone()),
                    disk.get_child_accounts(hash.clone(), addr)
                );
            }
            assert_eq!(
                memory.get_accounts(hash.clone(), ids.to_vec()),
                disk.get_accounts(hash.clone(), ids.to_vec())
            );
            let mut files = [vec![], vec![]];
            for (file, ctx) in files.iter_mut().zip([&*memory, &*disk]) {
                crate::persist::ledger_codec::write_full(file, &ctx.snarked_ledgers[hash]).unwrap();
            }
            assert!(files[0] == files[1], "frontier store files differ");
        }

        let dir = tempfile::tempdir().unwrap();
        let files = || std::fs::read_dir(dir.path()).unwrap().count();
        let disk_root = ledger_with(300);
        let mut memory = LedgerCtx::default();
        let mut disk = LedgerCtx::default();
        disk.set_ledger_disk_dir(dir.path().to_owned());
        // Refreeze at 8 changed accounts, detach a mask of 16.
        disk.root_disk_limits = Some((8, 16));
        let mut hash = LedgerHash::from_fp(disk_root.clone().merkle_root());
        memory
            .snarked_ledgers
            .insert(hash.clone(), disk_root.copy());
        disk.snarked_ledgers.insert(hash.clone(), disk_root);
        let mut ids = vec![Account::rand().id()];
        memory.snarked_ledgers[&hash].iter(|account| ids.push(account.id()));
        // The root staged ledger's mask and a block's above it.
        let staged = [&memory, &disk].map(|ctx| {
            let mut staged = ctx.snarked_ledgers[&hash].make_child();
            set_balances(&mut staged, &[(1, 11), (2, 12)]);
            let mut tip = staged.make_child();
            set_balances(&mut tip, &[(2, 22), (3, 23)]);
            (staged, tip)
        });
        disk.freeze_root_snarked_ledger_hash(&hash);
        assert_eq!(disk.snarked_ledgers[&hash].disk_state(), Some(0));
        assert!(memory.snarked_ledgers[&hash].disk_state().is_none());
        same(&mut memory, &mut disk, &hash, &ids);

        let mut overlay = vec![];
        let mut next_epoch = vec![];
        for round in 0u64..14 {
            // Round 6 is the epoch boundary: the root snarked ledger becomes
            // the next epoch ledger, and is kept as it is.
            if round == 6 {
                next_epoch = vec![hash.clone()];
            }
            let changes = [
                (round * 3 % 300, round + 100),
                ((round * 3 + 1) % 300, round + 200),
                ((round * 7 + 50) % 300, round + 300),
            ];
            let before = hash.clone();
            hash = root_move(&mut memory, &before, &changes, &next_epoch);
            assert_eq!(root_move(&mut disk, &before, &changes, &next_epoch), hash);
            let root = &disk.snarked_ledgers[&hash];
            overlay.push((root.is_root(), root.disk_state(), files()));
            same(&mut memory, &mut disk, &hash, &ids);
            let [(memory_root, memory_tip), (disk_root, disk_tip)] = staged.clone();
            for (mut memory_staged, mut disk_staged) in
                [(memory_root, disk_root), (memory_tip, disk_tip)]
            {
                assert_eq!(memory_staged.merkle_root(), disk_staged.merkle_root());
                assert_eq!(memory_staged.to_list(), disk_staged.to_list());
            }
            if let Some(epoch) = next_epoch.first() {
                let mut ledger = disk.snarked_ledgers[epoch].clone();
                assert_eq!(&LedgerHash::from_fp(ledger.merkle_root()), epoch);
                assert_eq!(ledger.disk_state(), Some(0));
            }
        }
        assert_eq!(
            overlay,
            vec![
                // Root moves into the overlay, refrozen at 8 and more.
                (true, Some(3), 1),
                (true, Some(6), 1),
                (true, Some(0), 1),
                (true, Some(3), 1),
                (true, Some(6), 1),
                (true, Some(0), 1),
                // After the boundary: a mask over the next epoch ledger...
                (false, None, 1),
                (false, None, 1),
                (false, None, 1),
                (false, None, 1),
                (false, None, 1),
                // ... until it holds 16 accounts: a database of its own,
                // written to its own file at once (16 changes).
                (true, Some(0), 2),
                (true, Some(3), 2),
                (true, Some(6), 2),
            ]
        );
        // The staged masks still hang from the root snarked ledger object.
        assert_eq!(
            staged[1].0.get_parent().unwrap().get_uuid(),
            disk.snarked_ledgers[&next_epoch[0]].get_uuid()
        );
    }

    /// The balances and delegates a root move or a block changes.
    #[derive(Clone)]
    struct RelReviewChanges {
        balances: Vec<(u64, u64)>,
        delegates: Vec<(u64, Option<CompressedPubKey>)>,
    }

    impl RelReviewChanges {
        /// Three balances and one delegate per round; the delegate flips on
        /// the default-token accounts `epoch_ledger` made delegate.
        fn round(round: u64, producer: &CompressedPubKey) -> Self {
            Self {
                balances: vec![
                    (round * 3 % 300, round + 100),
                    ((round * 3 + 1) % 300, round + 200),
                    ((round * 7 + 50) % 300, round + 300),
                ],
                delegates: vec![(
                    (round * 6) % 300,
                    (round % 2 == 0).then(|| producer.clone()),
                )],
            }
        }

        fn apply(&self, mask: &mut Mask) {
            set_balances(mask, self.balances.iter().copied());
            for (index, delegate) in &self.delegates {
                let mut account = mask.get_at_index(AccountIndex(*index)).unwrap();
                account.delegate = delegate.clone();
                mask.set_at_index(AccountIndex(*index), account).unwrap();
            }
        }
    }

    /// A commit, as far as the snarked ledgers go: the ones not kept are
    /// dropped and the masks over them folded in, then the epoch ledgers and
    /// the root snarked ledger go to disk (`LedgerCtx::commit`'s order).
    fn rel_review_commit(ctx: &mut LedgerCtx, root: &LedgerHash, epoch: &[LedgerHash]) {
        let mut keep = LedgersToKeep::new();
        keep.add_snarked(root.clone());
        for hash in epoch {
            keep.add_snarked(hash.clone());
        }
        ctx.drop_snarked_ledgers(&keep);
        ctx.compact_snarked_ledgers(&keep);
        ctx.freeze_snarked_ledgers(epoch.iter().filter(|hash| *hash != root).collect());
        ctx.freeze_root_snarked_ledger_hash(root);
    }

    /// A root move: the new root snarked ledger is a mask over the old one
    /// with `changes`, as `push_snarked_ledger` makes it, then the commit.
    fn rel_review_root_move(
        ctx: &mut LedgerCtx,
        old: &LedgerHash,
        changes: &RelReviewChanges,
        epoch: &[LedgerHash],
    ) -> LedgerHash {
        let mut mt = ctx.snarked_ledgers[old].make_child();
        changes.apply(&mut mt);
        let new = merkle_root(&mut mt);
        ctx.snarked_ledgers.insert(new.clone(), mt);
        rel_review_commit(ctx, &new, epoch);
        new
    }

    /// What a node answers from the ledger `hash`, from memory and from disk:
    /// serving (account count, child hashes, account ranges), RPC account
    /// reads, the VRF evaluator's delegator table and the delegator RPC, and
    /// the frontier store's full file, byte for byte.
    fn rel_review_same(
        memory: &mut LedgerCtx,
        disk: &mut LedgerCtx,
        hash: &LedgerHash,
        ids: &[AccountId],
        producer: &CompressedPubKey,
        step: &str,
    ) {
        let producer_key = AccountPublicKey::from(producer.clone());
        let producer_id = AccountId::new(producer.clone(), TokenId::default());
        assert_eq!(
            memory.get_num_accounts(hash.clone()),
            disk.get_num_accounts(hash.clone()),
            "{step}: {hash}"
        );
        for (addr, depth) in [(LedgerAddress::root(), 1), (LedgerAddress::first(29), 6)] {
            assert_eq!(
                memory.get_child_hashes_for_serving(hash.clone(), addr.clone(), depth),
                disk.get_child_hashes_for_serving(hash.clone(), addr, depth),
                "{step}: {hash}"
            );
        }
        for index in [0u64, 2, 4] {
            let addr = LedgerAddress::from_index(AccountIndex(index), 29);
            assert_eq!(
                memory.get_child_accounts(hash.clone(), addr.clone()),
                disk.get_child_accounts(hash.clone(), addr),
                "{step}: {hash}"
            );
        }
        assert_eq!(
            memory.get_accounts(hash.clone(), ids.to_vec()),
            disk.get_accounts(hash.clone(), ids.to_vec()),
            "{step}: {hash}"
        );
        assert_eq!(
            memory.producer_delegator_table(hash, &producer_key),
            disk.producer_delegator_table(hash, &producer_key),
            "{step}: {hash}: delegator table"
        );
        assert_eq!(
            memory.get_account_delegators(hash, &producer_id),
            disk.get_account_delegators(hash, &producer_id),
            "{step}: {hash}: delegators"
        );
        let mut files = [vec![], vec![]];
        for (file, ctx) in files.iter_mut().zip([&*memory, &*disk]) {
            crate::persist::ledger_codec::write_full(file, &ctx.snarked_ledgers[hash]).unwrap();
        }
        assert!(
            files[0] == files[1],
            "{step}: {hash}: frontier store files differ"
        );
    }

    /// A restart: the ledgers `hashes` come back from their frontier store
    /// files, as the loader reads them, into a ledger thread holding nothing,
    /// in a new process whose accounts directory `dir` starts empty.
    fn rel_review_restart(
        disk: &LedgerCtx,
        hashes: &[&LedgerHash],
        dir: &Path,
        limits: (usize, usize),
    ) -> LedgerCtx {
        use crate::persist::ledger_codec::{read_full, read_header, write_full, VerifyMode};
        let mut restored = LedgerCtx::default();
        restored.set_ledger_disk_dir(dir.to_owned());
        restored.root_disk_limits = Some(limits);
        for hash in hashes {
            if restored.snarked_ledgers.contains_key(*hash) {
                continue;
            }
            let mut bytes = vec![];
            write_full(&mut bytes, &disk.snarked_ledgers[*hash]).unwrap();
            let mut body = bytes.as_slice();
            let header = read_header(&mut body).unwrap();
            assert_eq!(&header.ledger_hash, *hash);
            let mask = read_full(&mut body, &header, VerifyMode::Full).unwrap();
            assert!(mask.is_root() && mask.disk_state().is_none());
            restored.snarked_ledgers.insert((*hash).clone(), mask);
        }
        restored
    }

    /// REL-REVIEW (0.20.0): PC3a, PC3b and PC3c together across two epoch
    /// boundaries, with root moves between them, a restart in the middle of
    /// an epoch and one right at a boundary, where the root snarked ledger
    /// and the next epoch ledger are one ledger. Every answer a node gives
    /// from these ledgers is compared with a node that keeps them in memory
    /// ([`rel_review_same`]), with the staged masks above the root, after
    /// every step; and no file outlives the ledgers it held.
    #[test]
    fn rel_review_two_boundaries_and_restarts_on_disk_answer_as_in_memory() {
        let producer = Account::rand().public_key;
        // Refreeze at 8 changed accounts, detach a mask of 16 (PC3b's rules,
        // scaled down to a ledger of 300 accounts).
        const LIMITS: (usize, usize) = (8, 16);
        let dirs = [(); 3].map(|()| tempfile::tempdir().unwrap());
        let files = |dir: &tempfile::TempDir| std::fs::read_dir(dir.path()).unwrap().count();

        let mut disk = LedgerCtx::default();
        disk.set_ledger_disk_dir(dirs[0].path().to_owned());
        disk.root_disk_limits = Some(LIMITS);
        let next_changes = (0..10).map(|i| (i * 7, i + 1)).collect::<Vec<_>>();
        let root_changes = (0..20).map(|i| (i * 11 + 1, i + 100)).collect::<Vec<_>>();
        let (hashes, twins) = bootstrap(&mut disk, &producer, [next_changes, root_changes]);
        let mut memory = LedgerCtx::default();
        for (hash, twin) in hashes.iter().zip(twins) {
            memory.snarked_ledgers.insert(hash.clone(), twin);
        }
        bootstrap_commit(&mut disk, &hashes);
        let [mut staking, mut next, mut root] = hashes;
        let mut ids = vec![Account::rand().id()];
        memory.snarked_ledgers[&staking].iter(|account| ids.push(account.id()));

        // The root staged ledger's mask and a block's above the root snarked
        // ledger, in both.
        let staged_over = |ctx: &LedgerCtx, root: &LedgerHash| {
            let mut staged = ctx.snarked_ledgers[root].make_child();
            set_balances(&mut staged, [(1, 11), (2, 12)]);
            let mut tip = staged.make_child();
            set_balances(&mut tip, [(2, 22), (3, 23)]);
            (staged, tip)
        };
        let mut staged = [&memory, &disk].map(|ctx| staged_over(ctx, &root));

        let mut round = 0u64;
        let check = |memory: &mut LedgerCtx,
                     disk: &mut LedgerCtx,
                     staged: &[(Mask, Mask); 2],
                     epoch: &[LedgerHash; 3],
                     step: &str| {
            for hash in epoch {
                rel_review_same(memory, disk, hash, &ids, &producer, step);
            }
            let [(memory_root, memory_tip), (disk_root, disk_tip)] = staged.clone();
            for (mut a, mut b) in [(memory_root, disk_root), (memory_tip, disk_tip)] {
                assert_eq!(a.merkle_root(), b.merkle_root(), "{step}: staged");
                assert_eq!(a.to_list(), b.to_list(), "{step}: staged");
            }
            assert!(
                disk.snarked_ledgers
                    .values()
                    .all(|mask| mask.disk_state().is_some()
                        || mask
                            .get_parent()
                            .is_some_and(|parent| parent.disk_state().is_some())),
                "{step}: every ledger reads a file"
            );
        };
        let mut moves = |memory: &mut LedgerCtx,
                         disk: &mut LedgerCtx,
                         staged: &[(Mask, Mask); 2],
                         root: &mut LedgerHash,
                         epoch: &[LedgerHash],
                         n: usize,
                         step: &str| {
            for _ in 0..n {
                let changes = RelReviewChanges::round(round, &producer);
                round += 1;
                let new = rel_review_root_move(memory, root, &changes, epoch);
                assert_eq!(rel_review_root_move(disk, root, &changes, epoch), new);
                *root = new;
                check(
                    memory,
                    disk,
                    staged,
                    &[epoch[0].clone(), epoch[1].clone(), root.clone()],
                    &format!("{step}, round {round}"),
                );
            }
        };

        check(
            &mut memory,
            &mut disk,
            &staged,
            &[staking.clone(), next.clone(), root.clone()],
            "bootstrap",
        );
        assert_eq!(files(&dirs[0]), 3);

        // Epoch k: six root moves.
        moves(
            &mut memory,
            &mut disk,
            &staged,
            &mut root,
            &[staking.clone(), next.clone()],
            6,
            "epoch k",
        );

        // Boundary 1, with a root move: the next epoch ledger becomes the
        // staking ledger, the root snarked ledger the next epoch ledger.
        (staking, next) = (next, root.clone());
        moves(
            &mut memory,
            &mut disk,
            &staged,
            &mut root,
            &[staking.clone(), next.clone()],
            7,
            "epoch k+1",
        );
        assert!(
            disk.delegate_index.contains_key(&staking),
            "the staking ledger is indexed"
        );
        assert!(
            disk.delegate_index.contains_key(&next),
            "the next epoch ledger is indexed"
        );
        assert!(disk.snarked_ledgers[&root].is_root(), "the root detached");

        // A restart in the middle of the epoch: three ledgers from the store.
        let restarted =
            rel_review_restart(&disk, &[&staking, &next, &root], dirs[1].path(), LIMITS);
        staged[1] = (Mask::create(LEDGER_DEPTH), Mask::create(LEDGER_DEPTH));
        drop(std::mem::replace(&mut disk, restarted));
        assert_eq!(
            files(&dirs[0]),
            0,
            "the old process's files are gone with its ledgers"
        );
        rel_review_commit(&mut disk, &root, &[staking.clone(), next.clone()]);
        // Both sides' staged masks are rebuilt over the current root, as a
        // restore rebuilds the root staged ledger over the loaded root.
        staged = [&memory, &disk].map(|ctx| staged_over(ctx, &root));
        check(
            &mut memory,
            &mut disk,
            &staged,
            &[staking.clone(), next.clone(), root.clone()],
            "restart 1",
        );
        assert_eq!(files(&dirs[1]), 3);
        moves(
            &mut memory,
            &mut disk,
            &staged,
            &mut root,
            &[staking.clone(), next.clone()],
            3,
            "epoch k+1 after restart",
        );

        // Boundary 2 without a root move: the root's snarked ledger is the
        // new next epoch ledger too, as right after a crossing.
        (staking, next) = (next, root.clone());
        rel_review_commit(&mut memory, &root, &[staking.clone(), next.clone()]);
        rel_review_commit(&mut disk, &root, &[staking.clone(), next.clone()]);
        check(
            &mut memory,
            &mut disk,
            &staged,
            &[staking.clone(), next.clone(), root.clone()],
            "boundary 2",
        );
        assert!(disk.delegate_index.contains_key(&staking));

        // A restart right there: two ledgers from the store, one of them the
        // root snarked and the next epoch ledger at once.
        let restarted =
            rel_review_restart(&disk, &[&staking, &next, &root], dirs[2].path(), LIMITS);
        staged[1] = (Mask::create(LEDGER_DEPTH), Mask::create(LEDGER_DEPTH));
        drop(std::mem::replace(&mut disk, restarted));
        assert_eq!(files(&dirs[1]), 0);
        assert_eq!(disk.snarked_ledgers.len(), 2);
        rel_review_commit(&mut disk, &root, &[staking.clone(), next.clone()]);
        staged = [&memory, &disk].map(|ctx| staged_over(ctx, &root));
        check(
            &mut memory,
            &mut disk,
            &staged,
            &[staking.clone(), next.clone(), root.clone()],
            "restart 2",
        );
        assert_eq!(files(&dirs[2]), 2);

        // Epoch k+2: root moves over the next epoch ledger, which is indexed
        // once it is no longer the root's.
        moves(
            &mut memory,
            &mut disk,
            &staged,
            &mut root,
            &[staking.clone(), next.clone()],
            7,
            "epoch k+2",
        );
        assert!(disk.delegate_index.contains_key(&next));
        assert!(disk.snarked_ledgers[&root].is_root());

        // Boundary 3, with a root move, and a few more.
        (staking, next) = (next, root.clone());
        moves(
            &mut memory,
            &mut disk,
            &staged,
            &mut root,
            &[staking.clone(), next.clone()],
            4,
            "epoch k+3",
        );
        assert!(
            files(&dirs[2]) <= 4,
            "at most the three ledgers' files and one being replaced"
        );

        drop(staged);
        drop(disk);
        for dir in &dirs {
            assert_eq!(files(dir), 0, "no file outlives its ledger");
        }
    }

    /// REL-REVIEW (0.20.0) finding 1: the root staged ledger's mask stays
    /// over the root snarked ledger the frontier holds, through three epoch
    /// boundaries with root moves, in memory and on disk side by side.
    ///
    /// On disk the mask used to be left on the object the root snarked
    /// ledger had detached from (PC3b's root detach, PC3c's next epoch
    /// detach), and when that object was dropped two boundaries later
    /// `commit_root_staged_ledger` committed the mask into it: an orphan root
    /// on disk, with its file kept and an overlay that grew at every root
    /// move. Now, after every commit, the mask's parent is the root snarked
    /// ledger on both sides, it is no root of its own, the directory holds
    /// the kept ledgers' files only, the mask owns the staged-but-not-snarked
    /// accounts and no more than in memory, every answer equals the
    /// in-memory twin's, and a restart from frontier store files at every
    /// step answers the same.
    #[test]
    fn rel_review_root_staged_ledger_after_three_boundaries() {
        fn staged_ledger(mask: &Mask) -> (Arc<MinaBaseStagedLedgerHashStableV1>, StagedLedger) {
            let mut staged_ledger =
                StagedLedger::create_exn(constraint_constants().clone(), mask.clone()).unwrap();
            let hash = MinaBaseStagedLedgerHashStableV1::from(&staged_ledger.hash());
            (Arc::new(hash), staged_ledger)
        }
        /// A root move with its staged ledger: the new root's staged ledger
        /// is a mask over the old root's with the round's changes, committed
        /// into it after the snarked ledgers' commit, as `commit` does.
        /// Returns the new root snarked hash and staged ledger hash.
        fn root_move(
            ctx: &mut LedgerCtx,
            old: &LedgerHash,
            old_staged: &Arc<MinaBaseStagedLedgerHashStableV1>,
            changes: &RelReviewChanges,
            staged_changes: &[(u64, u64)],
            epoch: &[LedgerHash],
        ) -> (LedgerHash, Arc<MinaBaseStagedLedgerHashStableV1>) {
            let mut mt = ctx.snarked_ledgers[old].make_child();
            changes.apply(&mut mt);
            let new = merkle_root(&mut mt);
            ctx.snarked_ledgers.insert(new.clone(), mt);
            // A block's changes: the ones the emitted proof moves into the
            // snarked ledger in this round were staged first, as in a node,
            // and some that stay staged.
            let mut staged = ctx
                .staged_ledgers
                .get(old_staged)
                .unwrap()
                .ledger()
                .make_child();
            changes.apply(&mut staged);
            set_balances(&mut staged, staged_changes.iter().copied());
            let (new_staged, ledger) = staged_ledger(&staged);
            ctx.staged_ledgers.insert(new_staged.clone(), ledger);
            let mut keep = LedgersToKeep::new();
            keep.add_snarked(new.clone());
            keep.add_staged(new_staged.clone());
            for hash in epoch {
                keep.add_snarked(hash.clone());
            }
            ctx.drop_snarked_ledgers(&keep);
            ctx.staged_ledgers.retain(|hash| keep.contains(hash));
            ctx.compact_snarked_ledgers(&keep);
            ctx.freeze_snarked_ledgers(epoch.iter().filter(|hash| **hash != new).collect());
            ctx.freeze_root_snarked_ledger_hash(&new);
            assert!(ctx.commit_root_staged_ledger(&new_staged, &new));
            (new, new_staged)
        }
        /// The root staged ledger's mask: whether its parent is the root
        /// snarked ledger, whether it is a root of its own, its disk state
        /// and the accounts it owns.
        fn root_staged(
            ctx: &LedgerCtx,
            root: &LedgerHash,
            staged: &Arc<MinaBaseStagedLedgerHashStableV1>,
        ) -> (bool, bool, Option<usize>, usize) {
            let mask = ctx.staged_ledgers.get(staged).unwrap().ledger();
            let over_root = mask
                .get_parent()
                .is_some_and(|parent| parent.get_uuid() == ctx.snarked_ledgers[root].get_uuid());
            (
                over_root,
                mask.is_root(),
                mask.disk_state(),
                mask.num_own_accounts(),
            )
        }
        /// A restart at this point: the three ledgers from frontier store
        /// files into a ledger thread holding nothing, the restore's commit,
        /// and the root staged ledger rebuilt over the restored root with the
        /// staged mask's own accounts, as the restore applies its diff.
        fn restarted(
            disk: &LedgerCtx,
            [staking, next, root]: &[LedgerHash; 3],
            staged: &Arc<MinaBaseStagedLedgerHashStableV1>,
            dir: &Path,
        ) -> (LedgerCtx, Mask) {
            let mut restored = rel_review_restart(disk, &[staking, next, root], dir, (8, 16));
            rel_review_commit(&mut restored, root, &[staking.clone(), next.clone()]);
            let source = disk.staged_ledgers.get(staged).unwrap().ledger();
            let mut mask = restored.snarked_ledgers[root].make_child();
            for index in source.own_account_indexes() {
                mask.set_at_index(index, source.get_at_index(index).unwrap())
                    .unwrap();
            }
            (restored, mask)
        }

        let producer = Account::rand().public_key;
        let dir = tempfile::tempdir().unwrap();
        let files = || std::fs::read_dir(dir.path()).unwrap().count();
        let mut disk = LedgerCtx::default();
        disk.set_ledger_disk_dir(dir.path().to_owned());
        disk.root_disk_limits = Some((8, 16));
        let next_changes = (0..10).map(|i| (i * 7, i + 1)).collect::<Vec<_>>();
        let root_changes = (0..20).map(|i| (i * 11 + 1, i + 100)).collect::<Vec<_>>();
        let (hashes, twins) = bootstrap(&mut disk, &producer, [next_changes, root_changes]);
        let mut memory = LedgerCtx::default();
        for (hash, twin) in hashes.iter().zip(twins) {
            memory.snarked_ledgers.insert(hash.clone(), twin);
        }
        bootstrap_commit(&mut disk, &hashes);
        let [mut staking, mut next, mut root] = hashes;
        let mut ids = vec![Account::rand().id()];
        memory.snarked_ledgers[&staking].iter(|account| ids.push(account.id()));
        // The root staged ledger, a mask over the root snarked ledger, as the
        // sync rebuilds it (R38) and as a restore loads it (I5d5).
        let mut staged = [&mut memory, &mut disk].map(|ctx| {
            let mut mask = ctx.snarked_ledgers[&root].make_child();
            set_balances(&mut mask, [(1, 11), (2, 12)]);
            let (hash, ledger) = staged_ledger(&mask);
            ctx.staged_ledgers.insert(hash.clone(), ledger);
            hash
        });

        type Step = (
            u64,
            (bool, bool, Option<usize>, usize),
            (bool, bool, Option<usize>, usize),
            usize,
        );
        let mut round = 0u64;
        let mut history: Vec<Step> = vec![];
        let mut moves = |memory: &mut LedgerCtx,
                         disk: &mut LedgerCtx,
                         root: &mut LedgerHash,
                         staged: &mut [Arc<MinaBaseStagedLedgerHashStableV1>; 2],
                         epoch: &[LedgerHash],
                         n: usize,
                         boundaries: usize,
                         history: &mut Vec<Step>| {
            for _ in 0..n {
                let changes = RelReviewChanges::round(round, &producer);
                let staged_changes = [
                    (round % 300, round + 1000),
                    ((round + 150) % 300, round + 2000),
                ];
                round += 1;
                let (new, memory_staged) =
                    root_move(memory, root, &staged[0], &changes, &staged_changes, epoch);
                let (same, disk_staged) =
                    root_move(disk, root, &staged[1], &changes, &staged_changes, epoch);
                assert_eq!(new, same);
                assert_eq!(memory_staged, disk_staged);
                *root = new;
                *staged = [memory_staged, disk_staged];
                let mut both = [&*memory, &*disk]
                    .map(|ctx| ctx.staged_ledgers.get(&staged[0]).unwrap().ledger());
                let roots = [both[0].merkle_root(), both[1].merkle_root()];
                let lists = [both[0].to_list(), both[1].to_list()];
                assert!(
                    lists[0] == lists[1] && roots[0] == roots[1],
                    "round {round}: same contents {}, staged roots {roots:?}",
                    lists[0] == lists[1]
                );
                let in_memory = root_staged(memory, root, &staged[0]);
                let on_disk = root_staged(disk, root, &staged[1]);
                history.push((round, in_memory, on_disk, files()));
                // The mask hangs from the root snarked ledger and is no root
                // of its own, on both sides; it owns the changes that stayed
                // staged, at most two a round, and no more than in memory.
                for (side, state) in [("in memory", in_memory), ("on disk", on_disk)] {
                    assert_eq!(
                        (state.0, state.1, state.2),
                        (true, false, None),
                        "round {round}, {side}: over the root snarked ledger, no root of its own"
                    );
                }
                // Finalized equal accounts are reclaimed after the rebase.
                // Keep the two changes per round that remain staged, plus
                // any changes retained across a boundary's parent move.
                assert_eq!(on_disk.3, in_memory.3, "round {round}: own accounts");
                assert!(
                    in_memory.3 <= 2 * round as usize + 2 + 4 * boundaries,
                    "round {round}: {} own accounts",
                    in_memory.3
                );
                // The kept ledgers' files only: no dropped ledger's.
                assert!(files() <= 3, "round {round}: {} files", files());
                // A restart here answers the same, and leaves nothing behind.
                let restart_dir = tempfile::tempdir().unwrap();
                let hashes = [epoch[0].clone(), epoch[1].clone(), root.clone()];
                let (mut restored, mut restored_staged) =
                    restarted(disk, &hashes, &staged[1], restart_dir.path());
                for hash in &hashes {
                    rel_review_same(
                        memory,
                        &mut restored,
                        hash,
                        &ids,
                        &producer,
                        &format!("round {round}, restarted"),
                    );
                }
                assert_eq!(
                    restored_staged.merkle_root(),
                    roots[0],
                    "round {round}: restarted staged root"
                );
                assert_eq!(
                    restored_staged.to_list(),
                    lists[0],
                    "round {round}: restarted staged accounts"
                );
                drop((restored_staged, restored));
                assert_eq!(std::fs::read_dir(restart_dir.path()).unwrap().count(), 0);
            }
        };

        // Epoch k, then three boundaries, each with a root move, and a few
        // root moves in each epoch.
        moves(
            &mut memory,
            &mut disk,
            &mut root,
            &mut staged,
            &[staking.clone(), next.clone()],
            3,
            0,
            &mut history,
        );
        for boundary in 1..=3 {
            (staking, next) = (next, root.clone());
            moves(
                &mut memory,
                &mut disk,
                &mut root,
                &mut staged,
                &[staking.clone(), next.clone()],
                5,
                boundary,
                &mut history,
            );
        }
        println!("REL-REVIEW root staged ledger (round, in memory, on disk, files): {history:?}");
        drop((staged, disk));
        assert_eq!(files(), 0, "no file outlives its ledger");
    }

    /// REL-REVIEW finding 1, review round: the move keeps every account the
    /// root staged ledger owns, equal to the new parent's or not. An account
    /// that transactions still in the scan state changed and changed back
    /// equals the snarked ledger's, and must stay the mask's own until those
    /// transactions are emitted: a proof emitted later may end part way
    /// through them, and the snarked ledger then holds a value the staged
    /// ledger must not show. A move that dropped such accounts as equal to
    /// the new parent's made the staged ledger and its child read that
    /// value. With the new parent in memory and on disk.
    #[test]
    fn rel_review_rebase_keeps_accounts_changed_and_changed_back() {
        fn balance(mask: &mut Mask, index: u64, amount: u64) {
            let mut account = mask.get_at_index(AccountIndex(index)).unwrap();
            account.balance = ledger::scan_state::currency::Balance::from_u64(amount);
            mask.set_at_index(AccountIndex(index), account).unwrap();
        }
        fn staged_ledger(
            ctx: &mut LedgerCtx,
            mask: &Mask,
        ) -> Arc<MinaBaseStagedLedgerHashStableV1> {
            let empty =
                StagedLedger::create_exn(constraint_constants().clone(), mask.clone()).unwrap();
            let scan = ledger_reclamation_tests::pending_scan([*mask
                .get_at_index(AccountIndex(0))
                .unwrap()]);
            let mut ledger = StagedLedger::from_persisted_unchecked(
                constraint_constants(),
                scan,
                mask.clone(),
                empty.pending_coinbase_collection().clone(),
            );
            let hash = Arc::new(MinaBaseStagedLedgerHashStableV1::from(&ledger.hash()));
            ctx.staged_ledgers.insert(hash.clone(), ledger);
            hash
        }
        for on_disk in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut old = ledger_with(8);
            let old_hash = merkle_root(&mut old);
            let mut ctx = LedgerCtx::default();
            if on_disk {
                ctx.set_ledger_disk_dir(dir.path().to_owned());
                // The root snarked ledger detaches at one own account.
                ctx.root_disk_limits = Some((8, 1));
                old.freeze_to_disk(dir.path()).unwrap().unwrap();
            }
            ctx.snarked_ledgers.insert(old_hash.clone(), old.clone());
            // The root staged ledger: account 0 changed and changed back by
            // transactions still in the scan state, account 1 changed; a
            // block's mask above it.
            let mut staged = old.make_child();
            balance(&mut staged, 0, 150);
            balance(&mut staged, 0, 100);
            balance(&mut staged, 1, 200);
            let expected = staged.copy_independent();
            let mut child = staged.make_child();
            balance(&mut child, 2, 300);
            let child_expected = child.copy_independent();
            let staged_hash = staged_ledger(&mut ctx, &staged);
            // The boundary: the proof emitted changed account 1 only. The root
            // snarked ledger is a mask over the old one, which is kept as the
            // next epoch ledger; on disk the root detaches from it.
            let mut snarked = old.make_child();
            balance(&mut snarked, 1, 200);
            let snarked_hash = merkle_root(&mut snarked);
            ctx.snarked_ledgers.insert(snarked_hash.clone(), snarked);
            ctx.freeze_root_snarked_ledger_hash(&snarked_hash);
            assert_eq!(ctx.snarked_ledgers[&snarked_hash].is_root(), on_disk);
            assert!(ctx.commit_root_staged_ledger(&staged_hash, &snarked_hash));
            assert_eq!(
                staged.get_parent().unwrap().get_uuid(),
                ctx.snarked_ledgers[&snarked_hash].get_uuid(),
                "on disk {on_disk}: moved onto the root snarked ledger"
            );
            assert_eq!(merkle_root(&mut staged), merkle_root(&mut expected.clone()));
            assert_eq!(
                staged.num_own_accounts(),
                1,
                "on disk {on_disk}: pending account kept, finalized account reclaimed"
            );
            // A later proof ends part way through account 0's changes, and
            // the root move folds it into the root snarked ledger.
            let mut next = ctx.snarked_ledgers[&snarked_hash].make_child();
            balance(&mut next, 0, 150);
            let next_hash = merkle_root(&mut next);
            ctx.snarked_ledgers.insert(next_hash.clone(), next);
            let mut keep = LedgersToKeep::new();
            keep.add_snarked(old_hash.clone());
            keep.add_snarked(next_hash.clone());
            keep.add_staged(staged_hash.clone());
            ctx.drop_snarked_ledgers(&keep);
            ctx.compact_snarked_ledgers(&keep);
            ctx.freeze_root_snarked_ledger_hash(&next_hash);
            assert!(ctx.commit_root_staged_ledger(&staged_hash, &next_hash));
            for (name, mask, twin) in [
                ("staged", &staged, &expected),
                ("child", &child, &child_expected),
            ] {
                assert_eq!(
                    mask.get_at_index(AccountIndex(0)).unwrap().balance,
                    twin.get_at_index(AccountIndex(0)).unwrap().balance,
                    "{name}, on disk {on_disk}: account 0 still reads what the staged ledger holds"
                );
                assert_eq!(
                    merkle_root(&mut mask.clone()),
                    merkle_root(&mut twin.clone()),
                    "{name}, on disk {on_disk}"
                );
                assert_eq!(mask.to_list(), twin.to_list(), "{name}, on disk {on_disk}");
            }
        }
    }

    /// The cost of the move on the Mesa devnet genesis ledger (92,540
    /// accounts) frozen on disk: masks owning 32 to 16,384 accounts, half of
    /// them equal to the new parent's, with 290 masks above, as the review's
    /// probe measured it. Prints a table.
    #[test]
    #[ignore = "needs MINA_GENESIS_BIN (mesa-devnet.bin); run with --release --nocapture"]
    fn rel_review_rebase_cost() {
        use crate::transition_frontier::genesis::PrebuiltGenesisConfig;
        fn balance(mask: &mut Mask, index: u64, amount: u64) {
            let mut account = mask.get_at_index(AccountIndex(index)).unwrap();
            account.balance = ledger::scan_state::currency::Balance::from_u64(amount);
            mask.set_at_index(AccountIndex(index), account).unwrap();
        }
        let path = std::env::var_os("MINA_GENESIS_BIN").expect("MINA_GENESIS_BIN");
        let (mut masks, _) =
            PrebuiltGenesisConfig::load_from_bytes(&std::fs::read(path).unwrap()).unwrap();
        let mut base = masks.remove(0);
        drop(masks);
        let accounts = base.num_accounts();
        let dir = tempfile::tempdir().unwrap();
        let _ = base.merkle_root();
        base.freeze_to_disk(dir.path()).unwrap().unwrap();
        println!("accounts {accounts}: own_before own_after rebase_ms root_rehash_ms tip_rehash_ms disk_reads");
        for own in [32usize, 256, 1450, 4096, 16384] {
            let mut staged = base.make_child();
            let mut onto = base.make_child();
            for i in 0..own {
                let index = (i * accounts / own) as u64;
                balance(&mut staged, index, 1_000_000_000 + i as u64);
                if i < own / 2 {
                    balance(&mut onto, index, 1_000_000_000 + i as u64);
                }
            }
            let onto_hash = merkle_root(&mut onto);
            onto.detach_into_root().unwrap();
            let mut descendants = vec![staged.clone()];
            for i in 0..290 {
                let mut child = descendants.last().unwrap().make_child();
                balance(
                    &mut child,
                    (i * 307 % accounts) as u64,
                    9_000_000_000 + i as u64,
                );
                let _ = child.merkle_root();
                descendants.push(child);
            }
            let before = staged.merkle_root();
            let tip_before = descendants.last_mut().unwrap().merkle_root();
            let ctx = LedgerCtx::default();
            let reads_before = ::ledger::database::disk::stats().reads;
            let started = redux::Instant::now();
            ctx.rebase_root_staged_ledger(&staged, &base, &onto, &onto_hash);
            let rebase_ms = started.elapsed().as_secs_f64() * 1000.0;
            let reads = ::ledger::database::disk::stats().reads - reads_before;
            assert_eq!(staged.get_parent().unwrap().get_uuid(), onto.get_uuid());
            let started = redux::Instant::now();
            assert_eq!(staged.merkle_root(), before);
            let root_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = redux::Instant::now();
            assert_eq!(descendants.last_mut().unwrap().merkle_root(), tip_before);
            let tip_ms = started.elapsed().as_secs_f64() * 1000.0;
            println!(
                "{own:>6} {:>6} {rebase_ms:>10.1} {root_ms:>8.1} {tip_ms:>8.1} {reads:>6}",
                staged.num_own_accounts()
            );
        }
    }

    /// PC3a: what the epoch ledgers' readers cost from memory and from disk,
    /// on the Mesa mainnet genesis ledger (288,965 accounts), with the answers
    /// compared. Prints a table.
    #[test]
    #[ignore = "needs MINA_GENESIS_BIN (mesa-mainnet.bin); run with --release --nocapture"]
    fn pc3a_epoch_ledger_read_costs() {
        use crate::transition_frontier::genesis::PrebuiltGenesisConfig;
        use std::time::Instant;

        fn time<T: PartialEq + std::fmt::Debug>(
            name: &str,
            ops: usize,
            memory: &mut LedgerCtx,
            disk: &mut LedgerCtx,
            f: impl Fn(&mut LedgerCtx) -> T,
        ) {
            let mut best = [f64::MAX; 2];
            let mut answers = vec![];
            for _ in 0..3 {
                for (i, ctx) in [&mut *memory, &mut *disk].into_iter().enumerate() {
                    let started = Instant::now();
                    let answer = f(ctx);
                    best[i] = best[i].min(started.elapsed().as_secs_f64() * 1000.0);
                    answers.push(answer);
                }
            }
            assert_eq!(answers[0], answers[1], "{name}");
            println!(
                "pc3a_read {name:<40} ops {ops:>7} memory_ms {:>9.2} disk_ms {:>9.2} ratio {:>6.2} disk_us_per_op {:>8.2}",
                best[0],
                best[1],
                best[1] / best[0],
                best[1] * 1000.0 / ops as f64
            );
        }

        let path = std::env::var_os("MINA_GENESIS_BIN").expect("set MINA_GENESIS_BIN");
        let (mut masks, _) =
            PrebuiltGenesisConfig::load_from_bytes(&std::fs::read(path).unwrap()).unwrap();
        let mut ledger = masks.remove(0);
        drop(masks);
        let hash = LedgerHash::from_fp(ledger.merkle_root());
        let dir = tempfile::tempdir().unwrap();
        let frozen = ledger.copy();
        let report = frozen.freeze_to_disk(dir.path()).unwrap().unwrap();
        println!("pc3a_freeze {report:?}");

        let mut counts = BTreeMap::<CompressedPubKey, usize>::new();
        let mut ids = vec![];
        let mut n = 0usize;
        ledger.iter(|account| {
            if let Some(delegate) = staking_delegate(account) {
                *counts.entry(delegate.clone()).or_default() += 1;
            }
            if n % 29 == 0 {
                ids.push(account.id());
            }
            n += 1;
        });
        let (producer, delegators) = counts.into_iter().max_by_key(|(_, n)| *n).unwrap();
        println!(
            "pc3a_producer {} delegators {delegators}",
            producer.into_address()
        );
        let producer_key = AccountPublicKey::from(producer.clone());
        let producer_id = AccountId::new(producer.clone(), TokenId::default());

        let mut memory = LedgerCtx::default();
        memory.snarked_ledgers.insert(hash.clone(), ledger.clone());
        let mut disk = LedgerCtx::default();
        disk.set_ledger_disk_dir(dir.path().to_owned());
        disk.snarked_ledgers.insert(hash.clone(), frozen);
        let (memory, disk) = (&mut memory, &mut disk);

        time("VRF delegator table, scan", n, memory, disk, |ctx| {
            ctx.producer_delegator_table(&hash, &producer_key)
        });
        time("producers_with_delegates (all)", n, memory, disk, |ctx| {
            ctx.producers_with_delegates(&hash, |_| true)
                .map(|p| p.len())
        });
        time("GraphQL delegators, scan", n, memory, disk, |ctx| {
            ctx.get_account_delegators(&hash, &producer_id)
                .map(|d| d.len())
        });
        // The same lookups with the delegate index a commit builds.
        disk.freeze_snarked_ledgers(BTreeSet::from([&hash]));
        assert!(disk.delegate_index.contains_key(&hash));
        time(
            "VRF delegator table, indexed",
            delegators,
            memory,
            disk,
            |ctx| ctx.producer_delegator_table(&hash, &producer_key),
        );
        time(
            "GraphQL delegators, indexed",
            delegators,
            memory,
            disk,
            |ctx| {
                ctx.get_account_delegators(&hash, &producer_id)
                    .map(|d| d.len())
            },
        );
        time("RPC account reads by id", ids.len(), memory, disk, |ctx| {
            ctx.get_accounts(hash.clone(), ids.clone())
        });
        time("serve num_accounts", 1, memory, disk, |ctx| {
            ctx.get_num_accounts(hash.clone())
        });
        let subtrees = (0..(n as u64).div_ceil(64))
            .step_by(4)
            .map(|i| LedgerAddress::from_index(AccountIndex(i), 29))
            .collect::<Vec<_>>();
        time(
            "serve child accounts (64 each)",
            subtrees.len() * 64,
            memory,
            disk,
            |ctx| {
                subtrees
                    .iter()
                    .map(|addr| {
                        ctx.get_child_accounts(hash.clone(), addr.clone())
                            .map(|a| a.len())
                    })
                    .collect::<Vec<_>>()
            },
        );
        let parents = (0..(n as u64).div_ceil(64))
            .step_by(4)
            .map(|i| LedgerAddress::from_index(AccountIndex(i), 29))
            .collect::<Vec<_>>();
        time(
            "serve child hashes (depth 6)",
            parents.len(),
            memory,
            disk,
            |ctx| {
                parents
                    .iter()
                    .map(|addr| ctx.get_child_hashes_for_serving(hash.clone(), addr.clone(), 6))
                    .collect::<Vec<_>>()
            },
        );
    }

    const GENESIS: u64 = 1;
    const STAKING: u64 = 2;
    const NEXT_EPOCH: u64 = 3;
    const PREVIOUS_ROOT: u64 = 4;
    const ROOT: u64 = 5;
    const MISSING: u64 = 99;

    fn ledger_hash(n: u64) -> LedgerHash {
        LedgerHash::from_fp(Fp::from(n))
    }

    /// A ledger of `num_accounts` random accounts, to tell copies apart.
    fn ledger_with(num_accounts: usize) -> Mask {
        let mut mask = Mask::create(LEDGER_DEPTH);
        for _ in 0..num_accounts {
            let account = Account::rand();
            mask.get_or_create_account(account.id(), account).unwrap();
        }
        mask
    }

    /// A root ledger sync during a bootstrap with a genesis config with
    /// ledgers: the genesis ledger is committed; the staking and next epoch
    /// ledgers are synced, and the previous root target synced or being
    /// synced, none of them committed until the bootstrap ends. Each holds as
    /// many accounts as its number.
    fn bootstrapping() -> LedgerCtx {
        let mut ctx = LedgerCtx::default();
        ctx.snarked_ledgers
            .insert(ledger_hash(GENESIS), ledger_with(GENESIS as usize));
        for n in [STAKING, NEXT_EPOCH, PREVIOUS_ROOT] {
            ctx.sync
                .snarked_ledgers
                .insert(ledger_hash(n), ledger_with(n as usize));
        }
        ctx
    }

    #[test]
    fn sync_copy_origin_is_the_first_candidate_committed_or_not() {
        let ctx = bootstrapping();
        let [genesis, staking, next_epoch, previous_root, missing] =
            [GENESIS, STAKING, NEXT_EPOCH, PREVIOUS_ROOT, MISSING].map(ledger_hash);

        // In the candidates' order: a committed ledger further down the list
        // doesn't jump ahead of a closer sync ledger.
        let candidates = [previous_root.clone(), genesis.clone()];
        assert_eq!(
            ctx.sync_copy_origin(&candidates),
            Some((&previous_root, false))
        );
        let candidates = [missing.clone(), next_epoch.clone(), staking.clone()];
        assert_eq!(
            ctx.sync_copy_origin(&candidates),
            Some((&next_epoch, false))
        );
        let candidates = [genesis.clone(), staking];
        assert_eq!(ctx.sync_copy_origin(&candidates), Some((&genesis, true)));
        assert_eq!(ctx.sync_copy_origin(&[missing]), None);
        assert_eq!(ctx.sync_copy_origin(&[]), None);
    }

    /// R14: the candidates `prepare_transition_frontier_root_ledger_for_sync`
    /// gives for a retargeted root ledger are the previous root target, the
    /// next epoch ledger and the staking ledger, none of them committed.
    #[test]
    fn a_retargeted_root_ledger_is_copied_from_the_previous_target() {
        let mut ctx = bootstrapping();
        let root = ledger_hash(ROOT);
        let candidates = [PREVIOUS_ROOT, NEXT_EPOCH, STAKING].map(ledger_hash);

        assert_eq!(
            ctx.copy_snarked_ledger_contents_for_sync(&candidates, root.clone(), false),
            Ok(true)
        );
        assert_eq!(
            ctx.sync.snarked_ledgers[&root].num_accounts(),
            PREVIOUS_ROOT as usize
        );

        // Once there, it is not copied again.
        let candidates = [ledger_hash(NEXT_EPOCH)];
        assert_eq!(
            ctx.copy_snarked_ledger_contents_for_sync(&candidates, root.clone(), false),
            Ok(false)
        );
        assert_eq!(
            ctx.sync.snarked_ledgers[&root].num_accounts(),
            PREVIOUS_ROOT as usize
        );
    }

    #[test]
    fn local_epoch_sync_replaces_an_incomplete_working_mask() {
        let mut ctx = bootstrapping();
        let target = ledger_hash(GENESIS);
        assert!(ctx.contains_snarked_ledger(&target));
        ctx.sync
            .snarked_ledgers
            .insert(target.clone(), ledger_with(99));
        assert_eq!(
            ctx.copy_snarked_ledger_contents_for_sync(&[], target.clone(), true),
            Ok(true)
        );
        assert_eq!(
            ctx.sync.snarked_ledgers[&target].num_accounts(),
            GENESIS as usize
        );
    }

    #[test]
    fn with_no_candidate_the_sync_starts_from_an_empty_ledger() {
        let mut ctx = bootstrapping();
        let root = ledger_hash(ROOT);

        assert_eq!(
            ctx.copy_snarked_ledger_contents_for_sync(&[ledger_hash(MISSING)], root.clone(), false),
            Ok(true)
        );
        assert_eq!(ctx.sync.snarked_ledgers[&root].num_accounts(), 0);
    }
    #[test]
    fn served_contents_and_hash_work_are_bounded() {
        use crate::p2p::channels::rpc::{CONTENTS_EMPTY, CONTENTS_TOO_LARGE};
        let mut ctx = LedgerCtx::default();
        let mut mask: Mask = Mask::new_root(Database::create(35));
        for n in 0..64 {
            mask.set_at_index(AccountIndex(n), Box::new(Account::empty()))
                .unwrap();
        }
        let hash = merkle_root(&mut mask);
        ctx.snarked_ledgers.insert(hash.clone(), mask);
        assert_eq!(
            ctx.get_child_accounts(hash.clone(), LedgerAddress::first(29))
                .unwrap()
                .len(),
            64
        );
        for length in [28, 3, 0] {
            assert_eq!(
                ctx.get_child_accounts(hash.clone(), LedgerAddress::first(length))
                    .unwrap_err(),
                CONTENTS_TOO_LARGE
            );
            let request = crate::ledger::read::LedgerReadRequest::GetChildAccountsAtAddr(
                hash.clone(),
                LedgerAddress::first(length),
            );
            assert_eq!(request.cost(), usize::MAX);
            assert!(!crate::ledger::read::LedgerReadState::default().can_admit(&request));
        }
        assert_eq!(
            ctx.get_child_accounts(hash.clone(), LedgerAddress::last(29))
                .unwrap_err(),
            CONTENTS_EMPTY
        );
        assert_eq!(
            ctx.get_child_hashes(hash.clone(), LedgerAddress::root(), 8)
                .unwrap()
                .len(),
            256
        );
        // OCaml rejects crossing the leaves rather than reducing the depth.
        for (length, depth, expected) in [
            (34, 1, Some(1)),
            (34, 8, None),
            (27, 1000, Some(8)),
            (28, 1000, None),
            (35, 1, None),
            (0, 0, None),
        ] {
            assert_eq!(
                crate::ledger::read::served_child_hashes_depth(
                    &LedgerAddress::first(length),
                    depth
                ),
                expected
            );
        }
        assert!(ctx
            .get_child_hashes(hash.clone(), LedgerAddress::first(34), 8)
            .is_none());
        for depth in [0, 9, usize::MAX] {
            assert!(ctx
                .get_child_hashes(hash.clone(), LedgerAddress::root(), depth)
                .is_none());
            assert_eq!(
                crate::ledger::read::LedgerReadRequest::GetChildHashesAtAddr(
                    hash.clone(),
                    LedgerAddress::root(),
                    depth
                )
                .cost(),
                usize::MAX
            );
        }
    }

    #[test]
    fn maximum_mesa_contents_answer_fits_serving_ceiling() {
        use ledger::{
            scan_state::currency::{Amount, Balance, Nonce, SlotSpan},
            Timing, VerificationKeyWire, ZkAppAccount, ZkAppUri,
        };
        use mina_p2p_messages::{
            binprot::BinProtWrite,
            rpc::AnswerSyncLedgerQueryV2,
            rpc_kernel::{NeedsLength, ResponsePayload, RpcMethod, RpcResult},
        };
        // Maximum encoded widths, not maximum unsigned values (u32::MAX
        // encodes as signed -1, which is shorter). Bounds come from OCaml:
        // account.ml Token_symbol.max_length = 6, zkapp_account.ml Uri = 255,
        // zkapp_state.ml Max_state_size = N32, permissions.ml Stable.V2,
        // side_loaded_verification_key.ml Repr and plonk_verification_key_evals.ml.
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/account.ml#L77-L126>
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/zkapp_account.ml#L153-L246>
        let wide32 = i32::MAX as u32;
        let wide64 = i64::MAX as u64;
        let mut account = Account::empty();
        account.delegate = Some(account.public_key.clone());
        account.token_symbol = ledger::TokenSymbol(vec![b'x'; 6]);
        account.balance = Balance::from_u64(wide64);
        account.nonce = Nonce::from_u32(wide32);
        account.timing = Timing::Timed {
            initial_minimum_balance: Balance::from_u64(wide64),
            cliff_time: Slot::from_u32(wide32),
            cliff_amount: Amount::from_u64(wide64),
            vesting_period: SlotSpan::from_u32(wide32),
            vesting_increment: Amount::from_u64(wide64),
        };
        account.permissions.set_verification_key.txn_version =
            ledger::scan_state::currency::TxnVersion::from_u32(wide32);
        account.zkapp = Some(Box::new(ZkAppAccount {
            app_state: [Fp::from(1u64); 32],
            action_state: [Fp::from(1u64); 5],
            verification_key: Some(VerificationKeyWire::dummy()),
            zkapp_version: wide32,
            last_action_slot: Slot::from_u32(wide32),
            proved_state: true,
            zkapp_uri: ZkAppUri::from(&mina_p2p_messages::string::ZkAppUri::from(vec![b'x'; 255])),
        }));
        let wire = v2::MinaBaseAccountBinableArgStableV2::from(&account);
        let mut bytes = Vec::new();
        wire.binprot_write(&mut bytes).unwrap();
        // Analytic OCaml bin_prot widths: base account (including zkapp option)
        // 243; zkapp 1025 + 1 + 1796 + 5 + 161 + 6 + 1 + 258 = 3253.
        // VK: 28 affine points * 64 + 2 vector unit tails + 2 enum tags.
        assert_eq!(bytes.len(), 243 + 3253);
        let mut mask = Mask::new_root(Database::create(35));
        for n in 0..64 {
            mask.set_at_index(AccountIndex(n), Box::new(account.clone()))
                .unwrap();
        }
        let hash = LedgerHash::from(MinaBaseLedgerHash0StableV1(Fp::from(1u64).into()));
        let mut ctx = LedgerCtx::default();
        ctx.snarked_ledgers.insert(hash.clone(), mask);
        let accounts = ctx
            .get_child_accounts(hash, LedgerAddress::first(29))
            .unwrap();
        assert_eq!(accounts.len(), 64);
        let response: ResponsePayload<<AnswerSyncLedgerQueryV2 as RpcMethod>::Response> =
            RpcResult(Ok(NeedsLength(RpcResult(Ok(
                v2::MinaLedgerSyncLedgerAnswerStableV2::ContentsAre(accounts.into_iter().collect()),
            )))));
        bytes.clear();
        response.binprot_write(&mut bytes).unwrap();
        // 64 * 3496, Contents_are + list = 2; wrapped result = 1;
        // payload length Nat0 = 5; outer Async RPC Ok = 1.
        assert_eq!(bytes.len(), 223_753);
        bytes.clear();
        mina_p2p_messages::rpc_kernel::Message::Response(mina_p2p_messages::rpc_kernel::Response {
            id: wide64,
            data: response,
        })
        .binprot_write(&mut bytes)
        .unwrap();
        // Message tag + maximum-width ID + eight-byte transport length.
        assert_eq!(bytes.len() + 8, 223_771);
        assert!(bytes.len() + 8 <= 64 * 3496 + 32);
        assert!(64 * 3496 + 32 <= crate::p2p::channels::rpc::MAX_SERVED_LEDGER_ANSWER_BYTES);
    }

    #[test]
    fn served_answer_size_and_oversized_account_return_errors() {
        use crate::p2p::channels::rpc::MAX_SERVED_LEDGER_ANSWER_BYTES;
        use std::io::Write;
        let mut size = LedgerAnswerSize(MAX_SERVED_LEDGER_ANSWER_BYTES - 1);
        size.write_all(&[0]).unwrap();
        assert!(size.write_all(&[0]).is_err());
        assert!(LedgerAnswerSize(usize::MAX).write_all(&[0]).is_err());
        let mut account = Account::empty();
        // Even an oversized locally stored account must not grow the answer
        // past its byte ceiling. No ledger hash is recomputed for this fixture.
        account.token_symbol = ledger::TokenSymbol(vec![b'x'; MAX_SERVED_LEDGER_ANSWER_BYTES]);
        let mut mask = Mask::new_root(Database::create(35));
        mask.set_at_index(AccountIndex(0), Box::new(account))
            .unwrap();
        let hash = LedgerHash::from(MinaBaseLedgerHash0StableV1(Fp::from(1u64).into()));
        let mut ctx = LedgerCtx::default();
        ctx.snarked_ledgers.insert(hash.clone(), mask);
        assert!(ctx
            .get_child_accounts(hash, LedgerAddress::first(29))
            .is_err());
    }

    #[test]
    fn staged_protocol_states_follow_ocaml_reverse_field_order() {
        use mina_p2p_messages::{binprot::BinProtWrite, rpc, rpc_kernel::ResponsePayload};
        let bytes = include_bytes!("../../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin");
        let tip = ResponsePayload::<rpc::GetBestTipV2Response>::binprot_read(&mut &bytes[..])
            .unwrap()
            .0
            .unwrap()
            .0
            .unwrap();
        let mut states = BTreeMap::new();
        // Field.compare orders integers numerically, not little-endian bytes.
        for n in [1u64, 255, 256] {
            let mut state = tip.data.header.protocol_state.clone();
            state.body.consensus_state.blockchain_length = (n as u32).into();
            states.insert(
                DataHashLibStateHashStableV1(Fp::from(n).into()).into(),
                state,
            );
        }
        let expected: Vec<_> = [256u64, 255, 1]
            .into_iter()
            .map(|n| {
                states[&StateHash::from(DataHashLibStateHashStableV1(Fp::from(n).into()))].clone()
            })
            .collect();
        let mut expected_bytes = Vec::new();
        List::from_iter(expected)
            .binprot_write(&mut expected_bytes)
            .unwrap();
        for order in [[1u64, 255, 256], [256, 1, 255], [255, 256, 1]] {
            let hashes = order
                .map(Fp::from)
                .into_iter()
                .collect::<std::collections::HashSet<_>>();
            let actual = ordered_required_protocol_states(hashes, &states).unwrap();
            let mut bytes = Vec::new();
            actual.binprot_write(&mut bytes).unwrap();
            assert_eq!(bytes, expected_bytes);
        }
        assert!(ordered_required_protocol_states([Fp::from(2u64)], &states).is_none());
    }

    /// The leaf-list root under the stable empty account is the ledger's own
    /// root: OCaml 4.0.2's `mina ledger hash` of its `mina ledger export`
    /// sample. Under `Account.Hardfork.empty` it differs, because the ledger
    /// is not full.
    #[test]
    fn leaf_roots_match_ocamls_ledger_hash() {
        let sample = include_str!("../daemon_json/testdata/ocaml-ledger-export-sample.json");
        let accounts: Vec<crate::daemon_json::Account> = serde_json::from_str(sample).unwrap();
        let leaves: Vec<Fp> = accounts
            .iter()
            .map(|a| a.to_account().unwrap().hash())
            .collect();
        let stable = merkle_root_of_leaves(&leaves, LEDGER_DEPTH, Account::empty().hash());
        assert_eq!(
            LedgerHash::from_fp(stable).to_string(),
            "jwUATMQxpP7A3xwoW7uEwS3M83zGi3prDUPSqEMpqUzvssSMaW3"
        );
        let migrated =
            merkle_root_of_leaves(&leaves, LEDGER_DEPTH, migrated_empty_account().hash());
        assert_ne!(stable, migrated);
        // An empty tree is the empty subtree at the root.
        assert_eq!(
            merkle_root_of_leaves(&[], LEDGER_DEPTH, Account::empty().hash()),
            <ledger::V2 as ledger::TreeVersion>::empty_hash_at_height(LEDGER_DEPTH)
        );
    }

    #[test]
    fn migrated_empty_account_is_ocamls() {
        let account = migrated_empty_account();
        let permissions = &account.permissions;
        assert_eq!(permissions.set_verification_key.txn_version.as_u32(), 5);
        assert_eq!(
            permissions.set_verification_key.auth,
            ledger::AuthRequired::Signature
        );
        assert_eq!(permissions.receive, ledger::AuthRequired::None);
        assert_eq!(permissions.access, ledger::AuthRequired::None);
        assert_eq!(permissions.send, ledger::AuthRequired::Signature);
        // Only the permissions differ from the stable empty account.
        let mut stable = Account::empty();
        stable.permissions = account.permissions.clone();
        assert_eq!(stable.hash(), account.hash());
    }

    mod light {
        use super::*;
        use crate::rpc::{LightBlockRef, LightLedgerKind};
        use mina_p2p_messages::v2::StateHash;
        use poseidon::hash::{hash_with_kimchi, params::get_merkle_param_for_height};

        const TARGET: &str = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";
        const OTHER: &str = "B62qnzbXmRNo9q32n4SNu2mpB8e7FYYLH8NmaX6oFCBYjjQ8SbD7uzV";

        fn id(address: &str) -> AccountId {
            AccountId::new(
                CompressedPubKey::from_address(address).unwrap(),
                TokenId::default(),
            )
        }

        /// The target's account in a token other than MINA.
        fn token_id(address: &str) -> AccountId {
            AccountId::new(
                CompressedPubKey::from_address(address).unwrap(),
                TokenId(Fp::from(424_242u64)),
            )
        }

        /// `before` random accounts, then the target's MINA account with
        /// `balance`, its token account with `balance + 1`, then three more.
        fn ledger_with_target(before: usize, balance: u64) -> Mask {
            let mut mask = ledger_with(before);
            for (account_id, balance) in [(id(TARGET), balance), (token_id(TARGET), balance + 1)] {
                let account = Account::create_with(
                    account_id,
                    ledger::scan_state::currency::Balance::from_u64(balance),
                );
                mask.get_or_create_account(account.id(), account).unwrap();
            }
            for _ in 0..3 {
                let account = Account::rand();
                mask.get_or_create_account(account.id(), account).unwrap();
            }
            mask
        }

        fn read(kind: LightLedgerKind, hash: &LedgerHash, ids: &[AccountId]) -> LightAccountRead {
            LightAccountRead {
                ledger: kind,
                ledger_hash: hash.clone(),
                block: LightBlockRef {
                    state_hash: StateHash::zero(),
                    height: 7,
                    global_slot_since_genesis: 9,
                },
                account_ids: ids.to_vec(),
            }
        }

        /// The answer for one MINA account.
        fn one(
            ctx: &LedgerCtx,
            kind: LightLedgerKind,
            hash: &LedgerHash,
            address: &str,
        ) -> RpcLightAccountEntry {
            let mut entries = ctx.light_account(&read(kind, hash, &[id(address)]))?;
            assert_eq!(entries.len(), 1);
            entries.pop().unwrap()
        }

        /// OCaml's `Merkle_path.implied_root`, written out again from the
        /// index bits rather than the path's own directions: bit `h` set
        /// means the node at height `h` is a right child.
        ///
        /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/merkle_ledger/merkle_path.ml#L32-L42>
        fn implied_root(leaf: Fp, index: u64, path: &[LightMerklePathElement]) -> Fp {
            path.iter()
                .enumerate()
                .fold(leaf, |node, (height, element)| {
                    let is_right = (index >> height) & 1 == 1;
                    let sibling = match (element, is_right) {
                        (LightMerklePathElement::Left(sibling), false)
                        | (LightMerklePathElement::Right(sibling), true) => {
                            sibling.to_field::<Fp>().unwrap()
                        }
                        _ => {
                            panic!("path direction at height {height} disagrees with index {index}")
                        }
                    };
                    let (left, right) = if is_right {
                        (sibling, node)
                    } else {
                        (node, sibling)
                    };
                    hash_with_kimchi(get_merkle_param_for_height(height), &[left, right])
                })
        }

        /// Each ledger kind, held where the node holds it, answers with an
        /// account whose path folds to exactly the hash it was asked for.
        #[test]
        fn light_paths_fold_to_the_ledger_hash_for_every_kind() {
            let mut snarked = ledger_with_target(5, 1_000);
            let snarked_hash = merkle_root(&mut snarked);
            // The staged ledger: a mask over the snarked one, the target's
            // balance changed after it.
            let mut staged = snarked.make_child();
            let address = staged.location_of_account(&id(TARGET)).unwrap();
            let mut account = staged.get(address.clone()).unwrap();
            account.balance = ledger::scan_state::currency::Balance::from_u64(2_000);
            staged.set(address, account);
            let staged_hash = merkle_root(&mut staged);
            let staged_ledger =
                StagedLedger::create_exn(constraint_constants().clone(), staged.clone()).unwrap();
            let mut staking = ledger_with_target(2, 3_000);
            let staking_hash = merkle_root(&mut staking);
            let mut next = ledger_with_target(9, 4_000);
            let next_hash = merkle_root(&mut next);

            let mut ctx = LedgerCtx::default();
            ctx.snarked_ledgers.insert(snarked_hash.clone(), snarked);
            ctx.staged_ledgers.insert_by_recomputing_hash(staged_ledger);
            // The node keeps epoch ledgers as snarked ledgers, or, from a
            // genesis config, as additional ones.
            ctx.snarked_ledgers.insert(staking_hash.clone(), staking);
            ctx.additional_snarked_ledgers
                .insert(next_hash.clone(), next);

            // Both of the target's accounts, MINA and a custom token, in one
            // read of each kind.
            // The staged ledger changed only the MINA account.
            for (kind, hash, index, balance, token_balance) in [
                (LightLedgerKind::Staged, &staged_hash, 5, 2_000, 1_001),
                (LightLedgerKind::Snarked, &snarked_hash, 5, 1_000, 1_001),
                (LightLedgerKind::Staking, &staking_hash, 2, 3_000, 3_001),
                (LightLedgerKind::Next, &next_hash, 9, 4_000, 4_001),
            ] {
                let ids = [id(TARGET), token_id(TARGET)];
                let entries = ctx.light_account(&read(kind, hash, &ids)).unwrap();
                assert_eq!(entries.len(), 2);
                for ((entry, expected_id), (index, balance)) in entries
                    .into_iter()
                    .zip(ids)
                    .zip([(index, balance), (index + 1, token_balance)])
                {
                    let answer = entry.unwrap();
                    assert_eq!(answer.ledger, kind);
                    assert_eq!(&answer.ledger_hash, hash);
                    assert_eq!(answer.block.height, 7);
                    assert_eq!(answer.index, index, "{kind}");
                    assert_eq!(answer.merkle_path.len(), LEDGER_DEPTH);
                    let account = Account::try_from(&answer.account).unwrap();
                    assert_eq!(account.id(), expected_id);
                    assert_eq!(account.balance.as_u64(), balance, "{kind}");
                    let leaf = answer.leaf_hash.to_field::<Fp>().unwrap();
                    assert_eq!(leaf, account.hash(), "{kind}: leaf is the account's hash");
                    assert_eq!(
                        implied_root(leaf, answer.index, &answer.merkle_path),
                        hash.to_field::<Fp>().unwrap(),
                        "{kind}: path folds to the ledger hash"
                    );
                }
            }
        }

        /// Exercise the endpoint read, not just the database path helper.
        fn assert_disk_paths(
            ctx: &LedgerCtx,
            kind: LightLedgerKind,
            hash: &LedgerHash,
            expected: &[(AccountId, u64, u64)],
        ) {
            let ids = expected
                .iter()
                .map(|(id, ..)| id.clone())
                .collect::<Vec<_>>();
            let entries = ctx.light_account(&read(kind, hash, &ids)).unwrap();
            assert_eq!(entries.len(), expected.len());
            for (entry, (id, index, balance)) in entries.into_iter().zip(expected) {
                let answer = entry.unwrap();
                assert_eq!(answer.ledger_hash, *hash);
                assert_eq!(answer.index, *index);
                assert_eq!(answer.merkle_path.len(), LEDGER_DEPTH);
                let account = Account::try_from(&answer.account).unwrap();
                assert_eq!(account.id(), *id);
                assert_eq!(account.balance.as_u64(), *balance);
                let leaf = answer.leaf_hash.to_field::<Fp>().unwrap();
                assert_eq!(leaf, account.hash());
                assert_eq!(
                    implied_root(leaf, answer.index, &answer.merkle_path),
                    hash.to_field::<Fp>().unwrap()
                );
            }
        }

        #[test]
        fn light_paths_on_frozen_databases_with_cold_and_warm_account_cache() {
            let dir = tempfile::tempdir().unwrap();
            for kind in [
                LightLedgerKind::Snarked,
                LightLedgerKind::Staking,
                LightLedgerKind::Next,
            ] {
                let mut mask = ledger_with_target(5, 1_000);
                let hash = merkle_root(&mut mask);
                assert!(!mask.freeze_to_disk(dir.path()).unwrap().unwrap().refreeze);
                assert_eq!(mask.disk_state(), Some(0));
                assert_eq!(mask.disk_cached(), Some(0));
                let mut ctx = LedgerCtx::default();
                if kind == LightLedgerKind::Next {
                    ctx.additional_snarked_ledgers
                        .insert(hash.clone(), mask.clone());
                } else {
                    ctx.snarked_ledgers.insert(hash.clone(), mask.clone());
                }
                // Frozen accounts and id index, compacted hash matrix, then
                // the same reads served from the decoded-account cache.
                for _ in 0..2 {
                    assert_disk_paths(
                        &ctx,
                        kind,
                        &hash,
                        &[(id(TARGET), 5, 1_000), (token_id(TARGET), 6, 1_001)],
                    );
                }
                assert!(mask.disk_cached().unwrap() >= 2);
            }
        }

        #[test]
        fn light_paths_on_pc3b_overlay_before_and_after_rewrite() {
            let dir = tempfile::tempdir().unwrap();
            let mut parent = ledger_with_target(5, 1_000);
            let parent_hash = merkle_root(&mut parent);
            parent.freeze_to_disk(dir.path()).unwrap().unwrap();
            let mut ctx = LedgerCtx::default();
            ctx.snarked_ledgers
                .insert(parent_hash.clone(), parent.clone());
            let original = [(id(TARGET), 5, 1_000), (token_id(TARGET), 6, 1_001)];
            // Warm the file cache before changing its accounts. The overlay
            // must take precedence over those cached decoded records.
            assert_disk_paths(&ctx, LightLedgerKind::Next, &parent_hash, &original);
            let mut root = parent.make_child();
            for (account_id, balance) in [(id(TARGET), 2_000), (token_id(TARGET), 2_001)] {
                let address = root.location_of_account(&account_id).unwrap();
                let mut account = root.get(address.clone()).unwrap();
                account.balance = ledger::scan_state::currency::Balance::from_u64(balance);
                root.set(address, account);
            }
            let account = Account::create_with(
                id(OTHER),
                ledger::scan_state::currency::Balance::from_u64(3_000),
            );
            root.get_or_create_account(account.id(), account).unwrap();
            let hash = merkle_root(&mut root);
            ctx.snarked_ledgers.insert(hash.clone(), root.clone());
            let changed = [
                (id(TARGET), 5, 2_000),
                (token_id(TARGET), 6, 2_001),
                (id(OTHER), 10, 3_000),
            ];
            // An attached mask over a frozen epoch ledger, then PC3b's
            // detached root with updates and an appended slot in its overlay.
            assert_disk_paths(&ctx, LightLedgerKind::Snarked, &hash, &changed);
            assert_eq!(root.detach_into_root(), Some(3));
            assert_eq!(root.disk_state(), Some(3));
            assert_disk_paths(&ctx, LightLedgerKind::Snarked, &hash, &changed);
            let report = root.freeze_to_disk(dir.path()).unwrap().unwrap();
            assert!(report.refreeze);
            assert_eq!(report.changed, 3);
            assert_eq!(root.disk_state(), Some(0));
            assert_eq!(root.disk_cached(), Some(0));
            for _ in 0..2 {
                assert_disk_paths(&ctx, LightLedgerKind::Snarked, &hash, &changed);
                assert_disk_paths(&ctx, LightLedgerKind::Next, &parent_hash, &original);
            }
            assert!(root.disk_cached().unwrap() >= 3);
            assert_eq!(merkle_root(&mut parent), parent_hash);
        }

        #[test]
        fn light_account_errors() {
            let mut snarked = ledger_with_target(3, 1_000);
            let hash = merkle_root(&mut snarked);
            let mut ctx = LedgerCtx::default();
            ctx.snarked_ledgers.insert(hash.clone(), snarked);

            // An account the ledger doesn't have.
            assert_eq!(
                one(&ctx, LightLedgerKind::Snarked, &hash, OTHER),
                Err(RpcLightError::AccountNotFound(LightLedgerKind::Snarked))
            );
            // In a batch, only that account's entry fails.
            let entries = ctx
                .light_account(&read(
                    LightLedgerKind::Snarked,
                    &hash,
                    &[id(OTHER), id(TARGET)],
                ))
                .unwrap();
            assert!(entries[0].is_err() && entries[1].is_ok());
            // A ledger the node doesn't hold.
            let missing = ledger_hash(424242);
            assert_eq!(
                one(&ctx, LightLedgerKind::Staking, &missing, TARGET),
                Err(RpcLightError::LedgerUnavailable {
                    kind: LightLedgerKind::Staking,
                    hash: missing.clone(),
                })
            );
            // A ledger being synced is never served, though `mask` finds it.
            ctx.sync.snarked_ledger_mut(missing.clone()).unwrap();
            assert!(ctx.mask(&missing).is_some());
            assert!(matches!(
                one(&ctx, LightLedgerKind::Next, &missing, TARGET),
                Err(RpcLightError::LedgerUnavailable { .. })
            ));
            // A ledger held under a hash that isn't its root is refused.
            let wrong = ledger_hash(7);
            ctx.snarked_ledgers
                .insert(wrong.clone(), ledger_with_target(1, 5));
            assert!(matches!(
                one(&ctx, LightLedgerKind::Snarked, &wrong, TARGET),
                Err(RpcLightError::Internal(error)) if error.contains("does not reach")
            ));
        }
    }
}
