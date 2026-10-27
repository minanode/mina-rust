use std::collections::VecDeque;

use mina_core::block::ArcBlockWithHash;
use mina_p2p_messages::v2::{LedgerHash, StateHash};
use redux::Timestamp;
use serde::{Deserialize, Serialize};

use crate::transition_frontier::sync::{
    ledger::SyncLedgerTargetKind, TransitionFrontierSyncBlockState,
};

/// Recent targets for the overview/history panel. The latest snapshot keeps
/// the entire sync chain for progress counts and apply-time distributions.
pub const MAX_SYNC_SNAPSHOTS: usize = 16;
/// Recent resync causes, carried forward to subsequent targets.
pub const MAX_RESYNC_EVENTS: usize = 32;
pub const MAX_RESYNC_ERROR_BYTES: usize = 200;

#[derive(Default, Clone)]
pub struct SyncStats {
    pub(super) snapshots: VecDeque<SyncStatsSnapshot>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SyncStatsSnapshot {
    pub kind: SyncKind,
    pub best_tip_received: Timestamp,
    pub synced: Option<Timestamp>,
    pub ledgers: SyncLedgers,
    pub blocks: Vec<SyncBlock>,
    /// The latest `MAX_RESYNC_EVENTS` causes, also carried to later targets.
    pub resyncs: Vec<LedgerResyncEvent>,
    /// The restore from the frontier store this run started with (F-dash3):
    /// on its [`SyncKind::Restored`] record, and kept on the catch-ups after
    /// it as `ledgers` are.
    #[serde(default)]
    pub restored: Option<SyncRestored>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum SyncKind {
    Bootstrap,
    Catchup,
    /// The frontier store's chain, replayed at startup (R20 D5, I5d1): the
    /// ledgers came from the store, none was fetched from peers.
    Restored,
}

/// A restore from the frontier store, for the dashboard (F-dash3): what was
/// restored, and how long loading and replaying it took.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SyncRestored {
    pub generation: u64,
    /// The load's number among the generation's restore attempts, from 1.
    pub attempt: u32,
    /// The stored checkpoint's root.
    pub root: StateHash,
    pub root_height: u32,
    /// The stored chain's tip: the best tip the node restarts from.
    pub tip: StateHash,
    pub tip_height: u32,
    /// Blocks replayed after the root.
    pub blocks: u32,
    /// Snarked ledgers loaded from the store.
    pub ledgers: u32,
    /// When the node asked the store thread for the load.
    pub load_start: Timestamp,
    /// The store thread's own time on the load and its checks.
    pub load_ms: f64,
    /// When the checked load arrived and the replay started.
    pub replay_start: Timestamp,
    /// When the replay committed; `None` while it runs, or if it failed.
    pub replay_end: Option<Timestamp>,
    /// The replay failed: the store is dropped and the node syncs from the
    /// network. `None` unless it did.
    #[serde(default)]
    pub failed: Option<SyncRestoreFailed>,
}

/// Why and when a restore's replay failed (F-dash3).
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SyncRestoreFailed {
    pub time: Timestamp,
    /// The block that failed and why, cut to [`RESTORE_FAILURE_MAX_LEN`]
    /// bytes.
    pub reason: String,
}

/// The longest restore failure reason a record keeps.
pub const RESTORE_FAILURE_MAX_LEN: usize = 200;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum LedgerResyncKind {
    FetchStagedLedgerError(String),
    RootLedgerChange,
    EpochChange,
    BestChainChange,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LedgerResyncEvent {
    pub kind: LedgerResyncKind,
    pub time: Timestamp,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SyncLedgers {
    pub staking_epoch: Option<SyncLedger>,
    pub next_epoch: Option<SyncLedger>,
    pub root: Option<SyncLedger>,
}

impl SyncLedgers {
    /// Figure out if a resync is required, and if so, for what reason.
    fn resync_kind(
        &self,
        best_tip: &ArcBlockWithHash,
        root_block: &ArcBlockWithHash,
    ) -> Option<LedgerResyncKind> {
        let consensus_state = &best_tip.block.header.protocol_state.body.consensus_state;
        let new_staking_epoch_ledger_hash = &consensus_state.staking_epoch_data.ledger.hash;
        let new_root_ledger_hash = root_block.snarked_ledger_hash();

        let staking_epoch_ledger_changed = self
            .staking_epoch
            .as_ref()
            .and_then(|sync| sync.snarked.hash.as_ref())
            .map(|prev_staking_epoch_ledger_hash| {
                prev_staking_epoch_ledger_hash != new_staking_epoch_ledger_hash
            })
            .unwrap_or(false);

        if let Some(prev_next_epoch_snarked_hash) = self
            .next_epoch
            .as_ref()
            .and_then(|sync| sync.snarked.hash.as_ref())
        {
            if prev_next_epoch_snarked_hash == new_staking_epoch_ledger_hash {
                // Previous next epoch moved to staking ledger, which means we advanced one epoch
                return Some(LedgerResyncKind::EpochChange);
            } else if staking_epoch_ledger_changed {
                // If we didn't advance an epoch and neither epoch ledger hash matches, then the best chain changed
                return Some(LedgerResyncKind::BestChainChange);
            }
        }

        if let Some(prev_root_ledger_hash) = self
            .root
            .as_ref()
            .and_then(|sync| sync.snarked.hash.as_ref())
        {
            if prev_root_ledger_hash != new_root_ledger_hash {
                return Some(LedgerResyncKind::RootLedgerChange);
            }
        }

        None
    }
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SyncLedger {
    pub snarked: SyncSnarkedLedger,
    pub staged: SyncStagedLedger,
    /// Loaded from the frontier store at startup, not fetched from peers
    /// (F-dash3).
    #[serde(default)]
    pub from_store: bool,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SyncSnarkedLedger {
    // TODO(openapi): generate ToSchema in Base58CheckOfBinProt macro
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub hash: Option<LedgerHash>,
    pub fetch_hashes_start: Option<Timestamp>,
    pub fetch_hashes_end: Option<Timestamp>,
    pub fetch_accounts_start: Option<Timestamp>,
    pub fetch_accounts_end: Option<Timestamp>,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SyncStagedLedger {
    // TODO(openapi): generate ToSchema in Base58CheckOfBinProt macro
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub hash: Option<LedgerHash>,
    pub fetch_parts_start: Option<Timestamp>,
    pub fetch_parts_end: Option<Timestamp>,
    pub reconstruct_start: Option<Timestamp>,
    pub reconstruct_end: Option<Timestamp>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SyncBlock {
    pub global_slot: Option<u32>,
    pub height: u32,
    pub hash: StateHash,
    pub pred_hash: StateHash,
    pub status: SyncBlockStatus,
    pub fetch_start: Option<Timestamp>,
    pub fetch_end: Option<Timestamp>,
    pub apply_start: Option<Timestamp>,
    pub apply_end: Option<Timestamp>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum SyncBlockStatus {
    Missing,
    Fetching,
    Fetched,
    Applying,
    ApplyFailed,
    Applied,
}

pub enum SyncingLedger {
    Init {
        snarked_ledger_hash: LedgerHash,
        staged_ledger_hash: Option<LedgerHash>,
    },
    FetchHashes {
        start: Timestamp,
        end: Timestamp,
    },
    FetchAccounts {
        start: Timestamp,
        end: Timestamp,
    },
    FetchParts {
        start: Timestamp,
        end: Option<Timestamp>,
    },
    ApplyParts {
        start: Timestamp,
        end: Option<Timestamp>,
    },
}

impl SyncStats {
    pub fn new_target(
        &mut self,
        time: Timestamp,
        best_tip: &ArcBlockWithHash,
        root_block: &ArcBlockWithHash,
    ) -> &mut Self {
        // A restore whose replay failed (or never committed) is followed by
        // a bootstrap, which starts afresh: the store's ledgers and chain
        // were dropped.
        let restore_failed = self
            .snapshots
            .back()
            .is_some_and(|s| s.kind == SyncKind::Restored && s.synced.is_none());
        let kind = match self.snapshots.back().is_none_or(|s| {
            matches!(s.kind, SyncKind::Bootstrap | SyncKind::Restored) && s.synced.is_none()
        }) {
            true => SyncKind::Bootstrap,
            false => SyncKind::Catchup,
        };

        if self.snapshots.len() >= MAX_SYNC_SNAPSHOTS {
            self.snapshots.pop_front();
        }

        // Retain the target ledger information from previous epochs in `ledgers`.
        // This ensures that the frontend continues to have access to historical ledger data even
        // after the node completes synchronization (at which point the sync stats no longer receive
        // updates about the older epoch or root ledgers).
        let (ledgers, restored) = match self.snapshots.back() {
            Some(snapshot) if !restore_failed => {
                (snapshot.ledgers.clone(), snapshot.restored.clone())
            }
            _ => Default::default(),
        };

        let mut resyncs = self
            .snapshots
            .back()
            .map_or_else(Default::default, |snapshot| snapshot.resyncs.clone());

        if let Some(prev_snapshot) = self.snapshots.back() {
            if prev_snapshot.synced.is_none() {
                if let Some(kind) = prev_snapshot.ledgers.resync_kind(best_tip, root_block) {
                    resyncs.push(LedgerResyncEvent { kind, time });
                }
            }
        }

        trim_resyncs(&mut resyncs);
        self.snapshots.push_back(SyncStatsSnapshot {
            kind,
            best_tip_received: time,
            synced: None,
            ledgers,
            blocks: vec![SyncBlock::best_tip(best_tip)],
            resyncs,
            restored,
        });

        self
    }

    /// The replay of a restore from the frontier store starts (F-dash3): a
    /// [`SyncKind::Restored`] record, whose ledgers are the store's
    /// (`stored_ledgers`) rather than fetched. Its blocks are the replay's,
    /// from [`Self::blocks_init`]; [`Self::restore_done`] and
    /// [`Self::synced`] end it.
    pub fn restore_start(
        &mut self,
        root_block: &ArcBlockWithHash,
        best_tip: &ArcBlockWithHash,
        stored_ledgers: &[LedgerHash],
        restored: SyncRestored,
    ) -> &mut Self {
        let ledger = |snarked: &LedgerHash, staged: Option<&LedgerHash>| SyncLedger {
            snarked: SyncSnarkedLedger {
                hash: Some(snarked.clone()),
                ..Default::default()
            },
            staged: SyncStagedLedger {
                hash: staged.cloned(),
                ..Default::default()
            },
            from_store: stored_ledgers.contains(snarked),
        };
        // The ledgers a bootstrap to this tip would fetch: its epoch ledgers,
        // and the root's snarked and staged ledgers, the staged one always
        // from the store.
        let ledgers = SyncLedgers {
            staking_epoch: Some(ledger(best_tip.staking_epoch_ledger_hash(), None)),
            next_epoch: Some(ledger(best_tip.next_epoch_ledger_hash(), None)),
            root: Some(ledger(
                root_block.snarked_ledger_hash(),
                Some(&root_block.staged_ledger_hashes().non_snark.ledger_hash),
            )),
        };

        if self.snapshots.len() >= MAX_SYNC_SNAPSHOTS {
            self.snapshots.pop_front();
        }
        self.snapshots.push_back(SyncStatsSnapshot {
            kind: SyncKind::Restored,
            best_tip_received: restored.replay_start,
            synced: None,
            ledgers,
            blocks: vec![SyncBlock::best_tip(best_tip)],
            resyncs: Vec::new(),
            restored: Some(restored),
        });
        self
    }

    /// The restore's replay committed (F-dash3).
    pub fn restore_done(&mut self, time: Timestamp) -> &mut Self {
        if let Some(restored) = self.restore_in_progress() {
            restored.replay_end = Some(time);
        }
        self
    }

    /// The restore's replay failed (F-dash3): its record is closed as such,
    /// with `reason` cut short. The node bootstraps; the next sync target
    /// starts a fresh [`SyncKind::Bootstrap`] record.
    pub fn restore_failed(&mut self, time: Timestamp, reason: &str) -> &mut Self {
        if let Some(restored) = self.restore_in_progress() {
            let mut end = reason.len().min(RESTORE_FAILURE_MAX_LEN);
            while !reason.is_char_boundary(end) {
                end -= 1;
            }
            restored.failed = Some(SyncRestoreFailed {
                time,
                reason: reason[..end].to_owned(),
            });
        }
        self
    }

    /// The latest record's restore, while its replay neither committed nor
    /// failed.
    fn restore_in_progress(&mut self) -> Option<&mut SyncRestored> {
        self.snapshots
            .back_mut()
            .filter(|s| s.kind == SyncKind::Restored)
            .and_then(|s| s.restored.as_mut())
            .filter(|r| r.replay_end.is_none() && r.failed.is_none())
    }

    pub fn ledger(&mut self, kind: SyncLedgerTargetKind, update: SyncingLedger) -> &mut Self {
        let Some(mut snapshot) = self.snapshots.pop_back() else {
            return self;
        };
        let ledger = snapshot.ledgers.get_or_insert(kind);

        match update {
            SyncingLedger::Init {
                snarked_ledger_hash,
                staged_ledger_hash,
            } => {
                if ledger.snarked.hash.as_ref() != Some(&snarked_ledger_hash) {
                    // Not the ledger a restore loaded: this one is fetched.
                    ledger.from_store = false;
                }
                ledger.snarked.hash = Some(snarked_ledger_hash);
                ledger.staged.hash = staged_ledger_hash;

                if let Some(prev_sync) = &self.snapshots.back().and_then(|s| s.ledgers.get(kind)) {
                    if prev_sync.snarked.hash == ledger.snarked.hash {
                        ledger.snarked = prev_sync.snarked.clone();
                    }

                    if prev_sync.staged.hash == ledger.staged.hash {
                        ledger.staged = prev_sync.staged.clone();
                    }
                }
            }
            SyncingLedger::FetchHashes { start, end } => {
                ledger.snarked.fetch_hashes_start.get_or_insert(start);
                let cur_end = ledger.snarked.fetch_hashes_end.get_or_insert(end);
                *cur_end = end.max(*cur_end);
            }
            SyncingLedger::FetchAccounts { start, end } => {
                ledger.snarked.fetch_accounts_start.get_or_insert(start);
                let cur_end = ledger.snarked.fetch_accounts_end.get_or_insert(end);
                *cur_end = end.max(*cur_end);
            }
            SyncingLedger::FetchParts { start, end } => {
                ledger.staged.fetch_parts_start.get_or_insert(start);
                if let Some(end) = end {
                    let cur_end = ledger.staged.fetch_parts_end.get_or_insert(end);
                    *cur_end = end.max(*cur_end);
                }
            }
            SyncingLedger::ApplyParts { start, end } => {
                ledger.staged.reconstruct_start.get_or_insert(start);
                if let Some(end) = end {
                    let cur_end = ledger.staged.reconstruct_end.get_or_insert(end);
                    *cur_end = end.max(*cur_end);
                }
            }
        }

        self.snapshots.push_back(snapshot);

        self
    }

    pub fn blocks_init(&mut self, states: &[TransitionFrontierSyncBlockState]) -> &mut Self {
        let Some(snapshot) = self.snapshots.back_mut() else {
            return self;
        };
        let Some((_root_height, best_tip_height)) = states
            .last()
            .and_then(|s| s.block())
            .map(|b| (b.root_block_height(), b.height()))
        else {
            return self;
        };

        snapshot.blocks = states
            .iter()
            .rev()
            // .take_while(|s| {
            //     !s.is_apply_success() || s.block().is_some_and( |b| b.height() == root_height)
            // })
            .enumerate()
            .map(|(i, s)| {
                let height = best_tip_height.checked_sub(i as u32).expect("underflow");
                let hash = s.block_hash().clone();
                let pred_hash = s
                    .block()
                    .map(|b| b.pred_hash())
                    .unwrap_or_else(|| {
                        states
                            .get(
                                states
                                    .len()
                                    .saturating_sub(i)
                                    .checked_sub(2)
                                    .expect("underflow"),
                            )
                            .unwrap() // can't fail because index is less than states.len()
                            .block_hash()
                    })
                    .clone();
                let mut stats = SyncBlock::new(height, hash, pred_hash);
                stats.update_with_block_state(s);
                stats
            })
            .collect();

        self
    }

    pub fn block_update(&mut self, state: &TransitionFrontierSyncBlockState) -> &mut Self {
        let Some(snapshot) = self.snapshots.back_mut() else {
            return self;
        };
        let block_hash = state.block_hash();
        let Some(stats) = snapshot.blocks.iter_mut().find(|b| &b.hash == block_hash) else {
            return self;
        };
        stats.update_with_block_state(state);
        self
    }

    pub fn synced(&mut self, time: Timestamp) -> &mut Self {
        let Some(snapshot) = self.snapshots.back_mut() else {
            return self;
        };
        snapshot.synced = Some(time);
        self
    }

    pub fn collect_stats(&self, limit: Option<usize>) -> Vec<SyncStatsSnapshot> {
        let limit = limit.unwrap_or(usize::MAX);
        self.snapshots.iter().rev().take(limit).cloned().collect()
    }

    pub fn staging_ledger_fetch_failure(&mut self, error: String, time: Timestamp) {
        if let Some(snapshot) = self.snapshots.back_mut() {
            snapshot.resyncs.push(LedgerResyncEvent {
                kind: LedgerResyncKind::FetchStagedLedgerError(truncate_error(error)),
                time,
            });
            trim_resyncs(&mut snapshot.resyncs);
        }
    }
}

fn trim_resyncs(events: &mut Vec<LedgerResyncEvent>) {
    let excess = events.len().saturating_sub(MAX_RESYNC_EVENTS);
    events.drain(..excess);
}

fn truncate_error(mut error: String) -> String {
    let mut end = error.len().min(MAX_RESYNC_ERROR_BYTES);
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    error.truncate(end);
    // Truncating a String does not release its original allocation.
    error.shrink_to_fit();
    error
}

impl SyncLedgers {
    pub fn get(&self, kind: SyncLedgerTargetKind) -> Option<&SyncLedger> {
        match kind {
            SyncLedgerTargetKind::StakingEpoch => self.staking_epoch.as_ref(),
            SyncLedgerTargetKind::NextEpoch => self.next_epoch.as_ref(),
            SyncLedgerTargetKind::Root => self.root.as_ref(),
        }
    }

    fn get_or_insert(&mut self, kind: SyncLedgerTargetKind) -> &mut SyncLedger {
        match kind {
            SyncLedgerTargetKind::StakingEpoch => {
                self.staking_epoch.get_or_insert_with(Default::default)
            }
            SyncLedgerTargetKind::NextEpoch => self.next_epoch.get_or_insert_with(Default::default),
            SyncLedgerTargetKind::Root => self.root.get_or_insert_with(Default::default),
        }
    }
}

impl SyncBlock {
    /// A sync's target, which the sync has from the start.
    fn best_tip(best_tip: &ArcBlockWithHash) -> Self {
        Self {
            global_slot: Some(best_tip.global_slot()),
            height: best_tip.height(),
            hash: best_tip.hash().clone(),
            pred_hash: best_tip.pred_hash().clone(),
            status: SyncBlockStatus::Fetched,
            fetch_start: None,
            fetch_end: None,
            apply_start: None,
            apply_end: None,
        }
    }

    pub fn new(height: u32, hash: StateHash, pred_hash: StateHash) -> Self {
        Self {
            global_slot: None,
            height,
            hash,
            pred_hash,
            status: SyncBlockStatus::Missing,
            fetch_start: None,
            fetch_end: None,
            apply_start: None,
            apply_end: None,
        }
    }

    pub fn update_with_block_state(&mut self, state: &TransitionFrontierSyncBlockState) {
        match state {
            TransitionFrontierSyncBlockState::FetchPending { attempts, .. } => {
                if let Some(time) = attempts
                    .iter()
                    .filter_map(|(_, v)| v.fetch_pending_since())
                    .min()
                {
                    self.status = SyncBlockStatus::Fetching;
                    self.fetch_start.get_or_insert(time);
                } else {
                    self.status = SyncBlockStatus::Missing;
                }
            }
            TransitionFrontierSyncBlockState::FetchSuccess { time, block, .. } => {
                self.global_slot.get_or_insert_with(|| block.global_slot());
                self.status = SyncBlockStatus::Fetched;
                self.fetch_end = Some(*time);
            }
            TransitionFrontierSyncBlockState::ApplyPending { time, block, .. } => {
                self.global_slot.get_or_insert_with(|| block.global_slot());
                self.status = SyncBlockStatus::Applying;
                self.apply_start = Some(*time);
            }
            TransitionFrontierSyncBlockState::ApplyError { time, block, .. } => {
                self.global_slot.get_or_insert_with(|| block.global_slot());
                self.status = SyncBlockStatus::ApplyFailed;
                self.apply_end = Some(*time);
            }
            TransitionFrontierSyncBlockState::ApplySuccess { time, block, .. } => {
                self.global_slot.get_or_insert_with(|| block.global_slot());
                self.status = SyncBlockStatus::Applied;
                self.apply_end = Some(*time);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mina_core::block::AppliedBlock;
    use mina_p2p_messages::v2;

    use super::*;
    use crate::persist::test_fixtures::{synthetic_chain, ChainLedgers};

    const SEC: u64 = 1_000_000_000;

    fn at(secs: u64) -> Timestamp {
        Timestamp::new(1_792_700_000 * SEC + secs * SEC)
    }

    fn ledger_hash(n: u64) -> LedgerHash {
        LedgerHash::from_fp(n.into())
    }

    /// A stored chain: its root at #100 and 5 blocks after it.
    fn chain() -> Vec<ArcBlockWithHash> {
        let ledgers = ChainLedgers {
            staking: ledger_hash(1),
            next: ledger_hash(2),
            snarked: ledger_hash(3),
            staged: v2::MinaBaseStagedLedgerHashStableV1::zero(
                ledger_hash(4),
                v2::PendingCoinbaseHash::from_fp(5u64.into()),
            ),
        };
        synthetic_chain(StateHash::from_fp(9u64.into()), 6, &ledgers)
    }

    /// What the effects record when the store's checked load arrives at 9 s
    /// (asked for at 2 s, 6.5 s on the store thread).
    fn restore_start(stats: &mut SyncStats, chain: &[ArcBlockWithHash]) {
        let (root, tip) = (&chain[0], chain.last().unwrap());
        stats.restore_start(
            root,
            tip,
            &[ledger_hash(1), ledger_hash(2), ledger_hash(3)],
            SyncRestored {
                generation: 7,
                attempt: 1,
                root: root.hash().clone(),
                root_height: root.height(),
                tip: tip.hash().clone(),
                tip_height: tip.height(),
                blocks: chain.len() as u32 - 1,
                ledgers: 3,
                load_start: at(2),
                load_ms: 6_500.0,
                replay_start: at(9),
                replay_end: None,
                failed: None,
            },
        );
    }

    /// The replay as the sync holds it: the root applied, the rest fetched
    /// (they come from the store), and then all applied.
    fn replay(stats: &mut SyncStats, chain: &[ArcBlockWithHash], applied: bool) {
        let states = chain
            .iter()
            .enumerate()
            .map(|(i, block)| {
                if i == 0 || applied {
                    TransitionFrontierSyncBlockState::ApplySuccess {
                        time: at(10),
                        block: AppliedBlock {
                            block: block.clone(),
                            just_emitted_a_proof: false,
                        },
                    }
                } else {
                    TransitionFrontierSyncBlockState::FetchSuccess {
                        time: at(9),
                        block: block.clone(),
                    }
                }
            })
            .collect::<Vec<_>>();
        stats.blocks_init(&states);
    }

    fn latest(stats: &SyncStats) -> SyncStatsSnapshot {
        stats.collect_stats(Some(1)).remove(0)
    }

    #[test]
    fn recent_targets_are_bounded_without_truncating_sync_blocks() {
        let chain = chain();
        let mut stats = SyncStats::default();
        restore_start(&mut stats, &chain);
        stats.restore_done(at(12)).synced(at(12));
        for target in 0..MAX_SYNC_SNAPSHOTS + 5 {
            stats.new_target(at(30 + target as u64), chain.last().unwrap(), &chain[0]);
            replay(&mut stats, &chain, true);
            stats.synced(at(31 + target as u64));
        }
        let records = stats.collect_stats(None);
        assert_eq!(records.len(), MAX_SYNC_SNAPSHOTS);
        assert_eq!(
            records[0].best_tip_received,
            at(30 + MAX_SYNC_SNAPSHOTS as u64 + 4)
        );
        assert_eq!(records.last().unwrap().best_tip_received, at(35));
        assert_eq!(records[0].blocks.len(), chain.len());
        assert!(records[0].blocks.iter().all(|b| b.apply_end.is_some()));
        assert_eq!(records[0].restored.as_ref().unwrap().tip_height, 105);
        assert!(records[0].ledgers.root.as_ref().unwrap().from_store);
        assert!(stats.collect_stats(Some(0)).is_empty());
        assert_eq!(stats.collect_stats(Some(1)).len(), 1);
        assert_eq!(
            stats.collect_stats(Some(usize::MAX)).len(),
            MAX_SYNC_SNAPSHOTS
        );
        // Repeated restore starts obey the same bound.
        for _ in 0..MAX_SYNC_SNAPSHOTS + 3 {
            restore_start(&mut stats, &chain);
        }
        assert_eq!(stats.snapshots.len(), MAX_SYNC_SNAPSHOTS);
    }

    #[test]
    fn resync_causes_and_utf8_errors_are_bounded_and_carried_forward() {
        let chain = chain();
        let mut stats = SyncStats::default();
        stats.new_target(at(1), chain.last().unwrap(), &chain[0]);
        for i in 0..MAX_RESYNC_EVENTS + 7 {
            stats.staging_ledger_fetch_failure("é".repeat(500), at(i as u64));
        }
        let record = latest(&stats);
        assert_eq!(record.resyncs.len(), MAX_RESYNC_EVENTS);
        assert_eq!(record.resyncs[0].time, at(7));
        for event in &record.resyncs {
            let LedgerResyncKind::FetchStagedLedgerError(error) = &event.kind else {
                panic!()
            };
            assert_eq!(error, &"é".repeat(MAX_RESYNC_ERROR_BYTES / 2));
        }
        stats.new_target(at(90), chain.last().unwrap(), &chain[0]);
        assert_eq!(latest(&stats).resyncs.len(), MAX_RESYNC_EVENTS);
        // A changed root also appends through the bounded path.
        stats.ledger(
            SyncLedgerTargetKind::Root,
            SyncingLedger::Init {
                snarked_ledger_hash: ledger_hash(99),
                staged_ledger_hash: None,
            },
        );
        stats.new_target(at(91), chain.last().unwrap(), &chain[0]);
        let record = latest(&stats);
        assert_eq!(record.resyncs.len(), MAX_RESYNC_EVENTS);
        assert_eq!(record.resyncs[0].time, at(8));
        assert!(matches!(
            record.resyncs.last().unwrap().kind,
            LedgerResyncKind::RootLedgerChange
        ));
    }

    /// F-dash3: a restart from the frontier store reads as a restore, with
    /// what was restored, the times, and the store's ledgers, not as a
    /// bootstrap with no ledgers.
    #[test]
    fn a_restore_is_recorded_as_one() {
        let chain = chain();
        let mut stats = SyncStats::default();
        restore_start(&mut stats, &chain);
        replay(&mut stats, &chain, false);

        let record = latest(&stats);
        assert_eq!(record.kind, SyncKind::Restored);
        assert_eq!(record.synced, None);
        assert_eq!(record.best_tip_received, at(9));
        let restored = record.restored.as_ref().unwrap();
        assert_eq!((restored.root_height, restored.tip_height), (100, 105));
        assert_eq!(restored.replay_end, None);
        // The replay's blocks, best tip first: the root applied, the rest
        // fetched (from the store) and not applied yet.
        let status = |b: &SyncBlock| (b.height, format!("{:?}", b.status));
        assert_eq!(
            record.blocks.iter().map(status).collect::<Vec<_>>(),
            [
                (105, "Fetched"),
                (104, "Fetched"),
                (103, "Fetched"),
                (102, "Fetched"),
                (101, "Fetched"),
                (100, "Applied"),
            ]
            .map(|(height, status)| (height, status.to_owned()))
        );
        // The ledgers a bootstrap would have fetched, all from the store.
        let ledgers = &record.ledgers;
        for (ledger, hash) in [
            (&ledgers.staking_epoch, 1),
            (&ledgers.next_epoch, 2),
            (&ledgers.root, 3),
        ] {
            let ledger = ledger.as_ref().unwrap();
            assert!(ledger.from_store);
            assert_eq!(ledger.snarked.hash, Some(ledger_hash(hash)));
            assert_eq!(ledger.snarked.fetch_hashes_start, None);
        }
        assert_eq!(
            ledgers.root.as_ref().unwrap().staged.hash,
            Some(ledger_hash(4))
        );

        // The replay commits at 12 s.
        replay(&mut stats, &chain, true);
        stats.restore_done(at(12)).synced(at(12));
        let record = latest(&stats);
        assert_eq!(record.kind, SyncKind::Restored);
        assert_eq!(record.synced, Some(at(12)));
        assert_eq!(record.restored.unwrap().replay_end, Some(at(12)));
        assert!(record
            .blocks
            .iter()
            .all(|b| matches!(b.status, SyncBlockStatus::Applied)));
    }

    /// The JSON the dashboard reads (`/stats/sync`).
    #[test]
    fn a_restore_record_as_the_dashboard_reads_it() {
        let chain = chain();
        let mut stats = SyncStats::default();
        restore_start(&mut stats, &chain);
        stats.restore_done(at(12)).synced(at(12));
        let json = serde_json::to_value(latest(&stats)).unwrap();
        assert_eq!(json["kind"], "Restored");
        assert_eq!(json["restored"]["tip_height"], 105);
        assert_eq!(json["restored"]["root_height"], 100);
        assert_eq!(json["restored"]["load_start"], u64::from(at(2)));
        assert_eq!(json["restored"]["replay_end"], u64::from(at(12)));
        assert_eq!(json["restored"]["load_ms"], 6_500.0);
        assert_eq!(json["ledgers"]["staking_epoch"]["from_store"], true);
        assert_eq!(json["ledgers"]["root"]["from_store"], true);
        assert_eq!(json["restored"]["failed"], serde_json::Value::Null);
        // Older records, without the new fields, still read.
        let mut old = json.clone();
        old.as_object_mut().unwrap().remove("restored");
        old["kind"] = "Bootstrap".into();
        for ledger in ["staking_epoch", "next_epoch", "root"] {
            old["ledgers"][ledger]
                .as_object_mut()
                .unwrap()
                .remove("from_store");
        }
        let old: SyncStatsSnapshot = serde_json::from_value(old).unwrap();
        assert!(old.restored.is_none());
        assert!(!old.ledgers.root.unwrap().from_store);
    }

    /// The catch-ups that follow a restore keep it, as they keep the
    /// ledgers, so that the dashboard can still say the node restored.
    #[test]
    fn catch_ups_after_a_restore_keep_it() {
        let chain = chain();
        let mut stats = SyncStats::default();
        restore_start(&mut stats, &chain);
        stats.restore_done(at(12)).synced(at(12));

        let (root, tip) = (&chain[1], chain.last().unwrap());
        stats.new_target(at(30), tip, root);
        let record = latest(&stats);
        assert_eq!(record.kind, SyncKind::Catchup);
        assert_eq!(record.restored.unwrap().tip_height, 105);
        assert!(record.ledgers.staking_epoch.unwrap().from_store);
        assert!(record.resyncs.is_empty());

        // A new staking ledger, as for a later bootstrap, is fetched.
        stats.ledger(
            SyncLedgerTargetKind::StakingEpoch,
            SyncingLedger::Init {
                snarked_ledger_hash: ledger_hash(11),
                staged_ledger_hash: None,
            },
        );
        let ledgers = latest(&stats).ledgers;
        assert!(!ledgers.staking_epoch.unwrap().from_store);
        assert!(ledgers.next_epoch.unwrap().from_store);
    }

    /// A replay that fails is closed as failed, with when and why, not
    /// left as a restore under way.
    #[test]
    fn a_failed_replay_closes_the_restore_as_failed() {
        let chain = chain();
        let mut stats = SyncStats::default();
        restore_start(&mut stats, &chain);
        replay(&mut stats, &chain, false);
        let reason = format!(
            "block {} at 103: staged ledger hash mismatch: {}",
            chain[3].hash(),
            "é".repeat(200)
        );
        stats.restore_failed(at(11), &reason);

        let record = latest(&stats);
        assert_eq!(record.kind, SyncKind::Restored);
        assert_eq!(record.synced, None);
        let restored = record.restored.unwrap();
        assert_eq!(restored.replay_end, None);
        let failed = restored.failed.unwrap();
        assert_eq!(failed.time, at(11));
        // Cut short, on a character boundary.
        assert!(failed.reason.len() <= RESTORE_FAILURE_MAX_LEN);
        assert!(failed.reason.len() >= RESTORE_FAILURE_MAX_LEN - 1);
        assert!(reason.starts_with(&failed.reason));
        assert!(failed.reason.contains("staged ledger hash mismatch"));

        // Neither a later commit nor a second failure changes it.
        stats.restore_done(at(12)).restore_failed(at(13), "again");
        let restored = latest(&stats).restored.unwrap();
        assert_eq!(restored.replay_end, None);
        assert_eq!(restored.failed.unwrap().time, at(11));

        // As the dashboard reads it.
        let json = serde_json::to_value(latest(&stats)).unwrap();
        assert_eq!(json["restored"]["failed"]["time"], u64::from(at(11)));
        assert!(json["restored"]["failed"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("block "));
    }

    /// After a failed replay the node bootstraps, and its record starts
    /// afresh rather than with the dropped store's ledgers.
    #[test]
    fn a_failed_restore_is_followed_by_a_fresh_bootstrap() {
        let chain = chain();
        let mut stats = SyncStats::default();
        restore_start(&mut stats, &chain);
        stats.restore_failed(at(11), "block at 103: failed");

        let (root, tip) = (&chain[1], chain.last().unwrap());
        stats.new_target(at(30), tip, root);
        let record = latest(&stats);
        assert_eq!(record.kind, SyncKind::Bootstrap);
        assert!(record.restored.is_none());
        assert!(record.ledgers.staking_epoch.is_none());
        assert!(record.ledgers.root.is_none());
        // The failed restore stays in the history.
        assert_eq!(stats.collect_stats(Some(2))[1].kind, SyncKind::Restored);
    }

    /// Without a restore, nothing changes: a bootstrap, then catch-ups.
    #[test]
    fn a_bootstrap_is_still_one() {
        let chain = chain();
        let (root, tip) = (&chain[0], chain.last().unwrap());
        let mut stats = SyncStats::default();
        stats.new_target(at(1), tip, root);
        assert_eq!(latest(&stats).kind, SyncKind::Bootstrap);
        stats.new_target(at(2), tip, root);
        assert_eq!(latest(&stats).kind, SyncKind::Bootstrap);
        stats.synced(at(3));
        stats.new_target(at(4), tip, root);
        let record = latest(&stats);
        assert_eq!(record.kind, SyncKind::Catchup);
        assert!(record.restored.is_none());
    }
}
