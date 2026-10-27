//! # Statistics Module
//!
//! This module collects and manages runtime statistics for the node. It tracks
//! three main categories of statistics that can be queried via RPC endpoints.
//!
//! ## Components
//!
//! | Sub-module        | Purpose                                              |
//! |-------------------|------------------------------------------------------|
//! | [`actions`]       | Tracks action dispatch frequency and timing          |
//! | [`sync`]          | Tracks blockchain synchronization progress           |
//! | [`block_producer`]| Tracks block production attempts and outcomes        |
//!
//! ## Architecture
//!
//! The [`Stats`] struct is held by the [`Service`](crate::Service) trait
//! implementation and is accessed via `store.service.stats()` in effects.
//! Statistics are updated during effect execution, not in reducers, since
//! they are side-effects that don't affect the core state machine.
//!
//! ## Usage
//!
//! Statistics are collected in effects throughout the codebase:
//!
//! - **Action stats**: Updated in [`effects()`](crate::effects()) for every
//!   dispatched action
//! - **Sync stats**: Updated in [`transition_frontier`](crate::transition_frontier)
//!   effects during synchronization
//! - **Block producer stats**: Updated in
//!   [`block_producer_effectful`](crate::block_producer_effectful) during block
//!   production
//!
//! ## RPC Endpoints
//!
//! Statistics are exposed via RPC for monitoring and debugging. These endpoints
//! are consumed by the frontend dashboard to display node health and performance:
//!
//! - `ActionStatsGet` - Returns action dispatch statistics
//! - `SyncStatsGet` - Returns synchronization statistics
//! - `BlockProducerStatsGet` - Returns block production statistics

mod stats_actions;
pub mod actions {
    pub use super::stats_actions::*;
}
use actions::{ActionStats, ActionStatsForBlock, ActionStatsSnapshot};

mod stats_sync;
pub mod sync {
    pub use super::stats_sync::*;
}
use sync::{SyncRestored, SyncStats, SyncStatsSnapshot, SyncingLedger};

mod stats_block_producer;
pub mod block_producer {
    pub use super::stats_block_producer::*;
}
use block_producer::BlockProducerStats;

use mina_core::block::{AppliedBlock, ArcBlockWithHash};
use mina_p2p_messages::v2::LedgerHash;
use redux::{ActionMeta, ActionWithMeta, Timestamp};

use crate::{
    transition_frontier::sync::{
        ledger::{staged::PeerStagedLedgerPartsFetchError, SyncLedgerTargetKind},
        TransitionFrontierSyncBlockState,
    },
    ActionKind,
};

pub type ActionKindWithMeta = ActionWithMeta<ActionKind>;

#[derive(Clone)]
pub struct Stats {
    last_action: ActionKindWithMeta,
    action_stats: ActionStats,
    sync_stats: SyncStats,
    block_producer_stats: BlockProducerStats,
}

/// Streaming JSON byte count: useful for trends without retaining a snapshot.
/// It is an encoding proxy, not a heap measurement.
pub(crate) fn serialized_size<T: serde::Serialize>(value: &T) -> Option<usize> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.0)
}

impl Stats {
    /// Diagnostic sizes are serialized byte counts, not heap allocation sizes.
    pub fn memory_debug(&self) -> serde_json::Value {
        serde_json::json!({
            "sync_snapshots": self.sync_stats.snapshots.len(),
            "action_blocks": self.action_stats.per_block.len(),
            "producer_attempts": self.block_producer_stats.attempts.len(),
            "sync_bytes": serialized_size(&self.sync_stats.snapshots),
            "action_bytes": serialized_size(&self.action_stats.per_block),
            "action_heap_bytes": self.action_stats.per_block_heap_bytes(),
            "action_since_start_heap_bytes": self.action_stats.since_start.heap_bytes(),
            "action_since_start_bytes": serialized_size(&self.action_stats.since_start),
            "producer_epochs": self.block_producer_stats.vrf_evaluator.len(),
            "producer_bytes": serialized_size(&self.block_producer_stats),
        })
    }

    pub fn new() -> Self {
        Self {
            last_action: ActionMeta::ZERO.with_action(ActionKind::None),
            action_stats: Default::default(),
            sync_stats: Default::default(),
            block_producer_stats: Default::default(),
        }
    }

    pub fn block_producer(&mut self) -> &mut BlockProducerStats {
        &mut self.block_producer_stats
    }

    pub fn new_sync_target(
        &mut self,
        time: Timestamp,
        best_tip: &ArcBlockWithHash,
        root_block: &ArcBlockWithHash,
    ) -> &mut Self {
        self.sync_stats.new_target(time, best_tip, root_block);
        self
    }

    /// A restore from the frontier store starts its replay (F-dash3).
    pub fn restore_start(
        &mut self,
        root_block: &ArcBlockWithHash,
        best_tip: &ArcBlockWithHash,
        stored_ledgers: &[LedgerHash],
        restored: SyncRestored,
    ) -> &mut Self {
        self.sync_stats
            .restore_start(root_block, best_tip, stored_ledgers, restored);
        self
    }

    /// The restore's replay committed (F-dash3).
    pub fn restore_done(&mut self, time: Timestamp) -> &mut Self {
        self.sync_stats.restore_done(time);
        self
    }

    /// The restore's replay failed (F-dash3).
    pub fn restore_failed(&mut self, time: Timestamp, reason: &str) -> &mut Self {
        self.sync_stats.restore_failed(time, reason);
        self
    }

    pub fn syncing_ledger(
        &mut self,
        kind: SyncLedgerTargetKind,
        update: SyncingLedger,
    ) -> &mut Self {
        self.sync_stats.ledger(kind, update);
        self
    }

    pub fn syncing_blocks_init(
        &mut self,
        states: &[TransitionFrontierSyncBlockState],
    ) -> &mut Self {
        self.sync_stats.blocks_init(states);
        self
    }

    pub fn syncing_block_update(&mut self, state: &TransitionFrontierSyncBlockState) -> &mut Self {
        self.sync_stats.block_update(state);
        self
    }

    pub fn new_best_chain(&mut self, time: Timestamp, chain: &[AppliedBlock]) -> &mut Self {
        let best_tip = chain.last().unwrap().block_with_hash();
        self.action_stats
            .new_best_tip(time, best_tip.height(), best_tip.hash().clone());
        self.sync_stats.synced(time);
        self.block_producer_stats.new_best_chain(time, chain);
        self
    }

    pub fn new_action(&mut self, kind: ActionKind, meta: ActionMeta) -> &mut Self {
        let action = meta.with_action(kind);
        self.action_stats.add(&action, &self.last_action);
        self.last_action = action;
        self
    }

    pub fn collect_action_stats_since_start(&self) -> ActionStatsSnapshot {
        self.action_stats.since_start.clone()
    }

    pub fn collect_action_stats_for_block_with_id(
        &self,
        id: Option<u64>,
    ) -> Option<ActionStatsForBlock> {
        self.action_stats.collect_stats_for_block_with_id(id)
    }

    pub fn collect_sync_stats(&self, limit: Option<usize>) -> Vec<SyncStatsSnapshot> {
        self.sync_stats.collect_stats(limit)
    }

    pub fn get_sync_time(&self) -> Option<Timestamp> {
        self.sync_stats
            .collect_stats(Some(1))
            .first()
            .and_then(|stats| stats.synced)
    }

    pub fn staging_ledger_fetch_failure(
        &mut self,
        error: &PeerStagedLedgerPartsFetchError,
        time: Timestamp,
    ) {
        self.sync_stats
            .staging_ledger_fetch_failure(format!("{error:?}"), time)
    }
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}
