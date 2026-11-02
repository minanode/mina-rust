use ledger::{
    scan_state::{
        currency::{Amount, Nonce, Slot},
        transaction_logic::valid::UserCommand,
    },
    transaction_pool::{
        diff::{BestTipDiff, DiffVerified},
        Config, ValidCommandWithHash,
    },
    Account, AccountId,
};
use mina_core::{
    consensus::ConsensusConstants,
    distributed_pool::DistributedPool,
    transaction::{TransactionPoolMessageSource, TransactionWithHash},
};
use mina_p2p_messages::{
    list::List,
    v2::{self, TransactionHash},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use super::candidate::TransactionPoolCandidatesState;

pub(super) type PendingId = u32;

/// Includes cancelled reads until their ledger response arrives. Tip churn
/// therefore cannot grow the ledger worker's request queue past this limit.
pub const MAX_ACCOUNT_READS: usize = 4;

/// Rate-limit reconciliation recovery without delaying block production.
pub(super) const ACCOUNT_READ_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(super) enum PoolWork {
    Verify {
        commands: List<TransactionWithHash>,
        account_ids: BTreeSet<AccountId>,
        from_source: TransactionPoolMessageSource,
    },
    Apply {
        diff: DiffVerified,
        from_source: TransactionPoolMessageSource,
    },
    /// A barrier: snapshot this ledger before the frontier can prune it, and
    /// reconcile it before subsequent pool mutations. Producers use the pool
    /// contents already available and never wait for this barrier.
    Reconcile { diff: Option<BestTipDiff> },
}

impl PoolWork {
    pub fn source(&self) -> TransactionPoolMessageSource {
        match self {
            Self::Verify { from_source, .. } | Self::Apply { from_source, .. } => *from_source,
            Self::Reconcile { .. } => TransactionPoolMessageSource::None,
        }
    }
    pub fn is_reconcile(&self) -> bool {
        matches!(self, Self::Reconcile { .. })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(super) enum AccountReadState {
    Queued,
    Pending,
    Ready(Result<BTreeMap<AccountId, Account>, String>),
    Applying,
    Retry { retry_at: redux::Timestamp },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(super) struct PoolAccountRead {
    pub work: PoolWork,
    pub ledger_hash: v2::LedgerHash,
    pub state: AccountReadState,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TransactionPoolState {
    pub candidates: TransactionPoolCandidatesState,
    /// OCaml network and pool status counters. Kept with the pool so a state
    /// snapshot and a metrics scrape observe the same applied actions.
    #[serde(default)]
    pub telemetry: TransactionPoolTelemetry,
    // TODO(binier): ideally this and `.pool` should be merged together.
    pub(super) dpool: DistributedPool<TransactionState, v2::TransactionHash>,
    pub(super) pool: ledger::transaction_pool::TransactionPool,
    /// Typed work and read lifecycle; never stored Redux continuation actions.
    pub(super) pending_actions: BTreeMap<PendingId, PoolAccountRead>,
    pub(super) account_read_order: VecDeque<PendingId>,
    pub(super) account_reads_in_flight: BTreeSet<PendingId>,
    /// At most MAX_ACCOUNT_READS entries; slot accounting never scans the backlog.
    #[serde(default)]
    pub(super) account_reads_ready: BTreeSet<PendingId>,
    pub(super) account_read_generation: u64,
    pub(super) pending_id: PendingId,
    pub(super) best_tip_hash: Option<v2::LedgerHash>,
    /// For debug only
    #[serde(skip)]
    pub(super) file: Option<std::fs::File>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TransactionPoolTelemetry {
    pub diffs_received: u64,
    pub diffs_broadcasted: u64,
    pub transactions_added: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TransactionState {
    pub time: redux::Timestamp,
    pub hash: TransactionHash,
}

impl AsRef<TransactionHash> for TransactionState {
    fn as_ref(&self) -> &TransactionHash {
        &self.hash
    }
}

impl Clone for TransactionPoolState {
    fn clone(&self) -> Self {
        Self {
            candidates: self.candidates.clone(),
            telemetry: self.telemetry.clone(),
            dpool: self.dpool.clone(),
            pool: self.pool.clone(),
            pending_actions: self.pending_actions.clone(),
            account_read_order: self.account_read_order.clone(),
            account_reads_in_flight: self.account_reads_in_flight.clone(),
            account_reads_ready: self.account_reads_ready.clone(),
            account_read_generation: self.account_read_generation,
            pending_id: self.pending_id,
            best_tip_hash: self.best_tip_hash.clone(),
            file: None,
        }
    }
}

impl TransactionPoolState {
    /// Diagnostic counts and encodings for maps whose keys are not JSON keys.
    pub fn memory_debug(&self) -> serde_json::Value {
        serde_json::json!({
            "pool_entries": self.size(),
            "pool_commands_json_bytes": crate::stats::serialized_size(&self.pool.get_all_transactions()),
            "distributed_entries": self.for_propagation_size(),
            "pending_actions": self.pending_actions.len(),
            "pending_actions_json_bytes": crate::stats::serialized_size(&self.pending_actions.values().collect::<Vec<_>>()),
            "account_reads_in_flight": self.account_reads_in_flight.len(),
            "account_reads_ready": self.account_reads_ready.len(),
        })
    }

    pub fn new(config: Config, consensus_constants: &ConsensusConstants) -> Self {
        Self {
            candidates: Default::default(),
            telemetry: Default::default(),
            dpool: Default::default(),
            pool: ledger::transaction_pool::TransactionPool::new(config, consensus_constants),
            pending_actions: Default::default(),
            account_read_order: Default::default(),
            account_reads_in_flight: Default::default(),
            account_reads_ready: Default::default(),
            account_read_generation: 0,
            pending_id: 0,
            best_tip_hash: None,
            file: None,
        }
    }

    pub fn size(&self) -> usize {
        self.pool.size()
    }

    pub fn max_size(&self) -> usize {
        self.pool.config.pool_max_size
    }

    pub fn minimum_fee_per_weight_unit(&self) -> Option<String> {
        self.pool.minimum_fee_per_weight_unit()
    }

    pub fn for_propagation_size(&self) -> usize {
        self.dpool.len()
    }

    pub fn contains(&self, hash: &TransactionHash) -> bool {
        self.get(hash).is_some()
    }

    pub fn get(&self, hash: &TransactionHash) -> Option<&UserCommand> {
        self.pool.pool.get(hash).map(|v| &v.data)
    }

    pub fn transactions(&mut self, limit: usize) -> Vec<ValidCommandWithHash> {
        self.pool.transactions(limit)
    }

    pub fn list_includable_transactions(&self, limit: usize) -> Vec<ValidCommandWithHash> {
        self.pool.list_includable_transactions(limit)
    }

    pub fn get_all_transactions(&self) -> Vec<ValidCommandWithHash> {
        self.pool.get_all_transactions()
    }

    pub fn get_pending_amount_and_nonce(&self) -> HashMap<AccountId, (Option<Nonce>, Amount)> {
        self.pool.get_pending_amount_and_nonce()
    }

    fn next_pending_id(&mut self) -> PendingId {
        loop {
            let id = self.pending_id;
            self.pending_id = self.pending_id.wrapping_add(1);
            if !self.pending_actions.contains_key(&id)
                && !self.account_reads_in_flight.contains(&id)
            {
                return id;
            }
        }
    }

    pub(super) fn enqueue(&mut self, work: PoolWork, ledger_hash: v2::LedgerHash) -> PendingId {
        let id = self.next_pending_id();
        self.pending_actions.insert(
            id,
            PoolAccountRead {
                work,
                ledger_hash,
                state: AccountReadState::Queued,
            },
        );
        self.account_read_order.push_back(id);
        id
    }

    pub(super) fn take_work(&mut self, id: PendingId) -> Option<PoolWork> {
        if self.account_read_order.front() != Some(&id)
            || !matches!(
                self.pending_actions.get(&id)?.state,
                AccountReadState::Applying
            )
        {
            return None;
        }
        self.account_read_order.pop_front();
        self.pending_actions.remove(&id).map(|r| r.work)
    }

    pub(super) fn account_read_slots(&self) -> usize {
        self.account_reads_in_flight.len() + self.account_reads_ready.len()
    }

    pub(crate) fn reconciling(&self) -> bool {
        // BestTipChanged moves ordinary work behind all reconciliation barriers.
        self.account_read_order.front().is_some_and(|id| {
            self.pending_actions
                .get(id)
                .is_some_and(|r| r.work.is_reconcile())
        })
    }

    pub(crate) fn account_read_retry_due(&self, time: redux::Timestamp) -> bool {
        self.account_read_order.front().is_some_and(|id| {
            self.pending_actions.get(id).is_some_and(
                |r| matches!(r.state, AccountReadState::Retry { retry_at } if time >= retry_at),
            )
        })
    }

    pub(super) fn retry_account_read(&mut self) -> (PendingId, v2::LedgerHash) {
        let old_id = self.account_read_order.pop_front().unwrap();
        let mut read = self.pending_actions.remove(&old_id).unwrap();
        let id = self.next_pending_id();
        let hash = read.ledger_hash.clone();
        read.state = AccountReadState::Queued;
        self.pending_actions.insert(id, read);
        self.account_read_order.push_front(id);
        (id, hash)
    }

    #[allow(dead_code)]
    fn save_actions(state: &mut crate::Substate<Self>) {
        let substate = state.get_substate_mut().unwrap();
        if substate.file.is_none() {
            let mut file = std::fs::File::create("/tmp/pool.bin").unwrap();
            postcard::to_io(&state.unsafe_get_state(), &mut file).unwrap();
            let substate = state.get_substate_mut().unwrap();
            substate.file = Some(file);
        }
    }

    /// The nonce the pool expects next from `account_id`, if it holds any of
    /// its commands.
    pub fn inferred_nonce(&self, account_id: &ledger::AccountId) -> Option<Nonce> {
        self.pool.inferred_nonce(account_id)
    }

    pub(super) fn global_slots(state: &crate::State) -> Option<(Slot, Slot)> {
        Some((
            Slot::from_u32(state.cur_global_slot()?),
            Slot::from_u32(state.cur_global_slot_since_genesis()?),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{super::TransactionPoolActionWithMeta, *};
    use crate::State;
    use redux::Dispatcher;

    /// A debugging tool rather than a test: replays the pool actions that
    /// `save_actions` wrote to `/tmp/pool.bin` while a node ran.
    #[allow(unused)]
    #[test]
    #[ignore = "debugging tool: needs /tmp/pool.bin, written by save_actions"]
    fn test_replay_pool() {
        let vec = std::fs::read("/tmp/pool.bin").unwrap();
        let slice = vec.as_slice();

        let (mut state, rest) = postcard::take_from_bytes::<State>(slice).unwrap();
        let mut slice = rest;

        while let Ok((action, rest)) =
            postcard::take_from_bytes::<TransactionPoolActionWithMeta>(slice)
        {
            slice = rest;

            let mut dispatcher = Dispatcher::new();
            let state = crate::Substate::<TransactionPoolState>::new(&mut state, &mut dispatcher);
            let (action, meta) = action.split();

            TransactionPoolState::handle_action(state, meta.with_action(&action));
        }
    }
}
