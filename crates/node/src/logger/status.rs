//! What an operator follows at the default log level (O1), besides the lines
//! of single events: a status line at most once a minute, the first sync,
//! losing sync and getting it back, and the SNARK work the node sold.
//!
//! [`StatusLog`] is outside the state machine: the node's event loop calls
//! [`StatusLog::update`] with the state after each round of actions. Its
//! decisions are in [`StatusLog::observe`], which takes the time and a
//! [`StatusSnapshot`] and returns what to log.

use std::{collections::BTreeSet, fmt, time::Duration};

use mina_p2p_messages::v2::StateHash;
use redux::Timestamp;

use crate::{
    transition_frontier::sync::{
        ledger::TransitionFrontierSyncLedgerState, SyncPhase, TransitionFrontierSyncLedgerPending,
        TransitionFrontierSyncState,
    },
    State,
};

/// Peers we are connected to and can talk to.
pub fn ready_peers(state: &State) -> usize {
    state
        .p2p
        .ready()
        .map_or(0, |p2p| p2p.ready_peers_iter().count())
}

/// `3NK6FYGa..mrie5D`: enough to tell blocks apart and find one in an
/// explorer's list.
pub fn short_hash(hash: &str) -> String {
    match (hash.get(..8), hash.get(hash.len().saturating_sub(6)..)) {
        (Some(head), Some(tail)) if hash.len() > 16 => format!("{head}..{tail}"),
        _ => hash.to_owned(),
    }
}

/// What the status line reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub peers: usize,
    pub sync: SyncStatus,
    /// The height of the best tip the node syncs to, while it syncs.
    pub target: Option<u32>,
    /// The best tip's height and hash.
    pub best_tip: Option<(u32, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    /// No best tip from peers yet.
    Waiting,
    /// A ledger the frontier needs, `"staking epoch"`, `"next epoch"` or
    /// `"root"`: its accounts and hashes fetched of an estimate, while those
    /// are fetched.
    Ledger {
        ledger: &'static str,
        progress: Option<(u64, u64)>,
        staged: bool,
    },
    /// The blocks from the root to the best tip.
    Blocks {
        fetched: usize,
        applied: usize,
        total: usize,
    },
    /// The synced chain is being committed.
    Commit,
    Synced,
}

impl SyncStatus {
    pub fn from_state(sync: &TransitionFrontierSyncState) -> Self {
        fn ledger(name: &'static str, pending: &TransitionFrontierSyncLedgerPending) -> SyncStatus {
            let (progress, staged) = match &pending.ledger {
                TransitionFrontierSyncLedgerState::Snarked(snarked) => (
                    snarked
                        .estimation()
                        .map(|progress| (progress.fetched, progress.estimation)),
                    false,
                ),
                TransitionFrontierSyncLedgerState::Staged(_) => (None, true),
                _ => (None, false),
            };
            SyncStatus::Ledger {
                ledger: name,
                progress,
                staged,
            }
        }
        let done = |name| SyncStatus::Ledger {
            ledger: name,
            progress: None,
            staged: false,
        };
        match sync {
            TransitionFrontierSyncState::Idle => Self::Waiting,
            TransitionFrontierSyncState::Init { .. } => done("staking epoch"),
            TransitionFrontierSyncState::StakingLedgerPending(pending) => {
                ledger("staking epoch", pending)
            }
            TransitionFrontierSyncState::StakingLedgerSuccess { .. } => done("next epoch"),
            TransitionFrontierSyncState::NextEpochLedgerPending(pending) => {
                ledger("next epoch", pending)
            }
            TransitionFrontierSyncState::NextEpochLedgerSuccess { .. } => done("root"),
            TransitionFrontierSyncState::RootLedgerPending(pending) => ledger("root", pending),
            TransitionFrontierSyncState::RootLedgerSuccess { .. } => Self::Blocks {
                fetched: 0,
                applied: 0,
                total: 0,
            },
            TransitionFrontierSyncState::BlocksPending { chain, .. } => Self::Blocks {
                fetched: chain.iter().filter(|block| block.block().is_some()).count(),
                applied: chain
                    .iter()
                    .filter(|block| block.is_apply_success())
                    .count(),
                total: chain.len(),
            },
            TransitionFrontierSyncState::BlocksSuccess { chain, .. } => Self::Blocks {
                fetched: chain.len(),
                applied: chain.len(),
                total: chain.len(),
            },
            TransitionFrontierSyncState::CommitPending { .. }
            | TransitionFrontierSyncState::CommitSuccess { .. } => Self::Commit,
            TransitionFrontierSyncState::Synced { .. } => Self::Synced,
        }
    }
}

impl fmt::Display for SyncStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Waiting => write!(f, "waiting for a best tip from peers"),
            Self::Ledger {
                ledger,
                staged: true,
                ..
            } => write!(f, "syncing the {ledger} staged ledger"),
            Self::Ledger {
                ledger,
                progress: Some((fetched, estimate)),
                ..
            } => write!(
                f,
                "syncing the {ledger} ledger, {}% ({fetched} of about {estimate} accounts and hashes)",
                fetched.saturating_mul(100) / (*estimate).max(1)
            ),
            Self::Ledger { ledger, .. } => write!(f, "syncing the {ledger} ledger"),
            Self::Blocks {
                fetched,
                applied,
                total,
            } => write!(
                f,
                "syncing blocks, {fetched} of {total} fetched, {applied} applied"
            ),
            Self::Commit => write!(f, "committing the synced chain"),
            Self::Synced => write!(f, "synced"),
        }
    }
}

impl fmt::Display for StatusSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.sync)?;
        if let Some(height) = self.target {
            write!(f, " to height {height}")?;
        }
        if let Some((height, hash)) = &self.best_tip {
            write!(f, ", best tip {height} {}", short_hash(hash))?;
        }
        match self.peers {
            1 => write!(f, ", 1 peer"),
            peers => write!(f, ", {peers} peers"),
        }
    }
}

impl StatusSnapshot {
    pub fn from_state(state: &State) -> Self {
        let frontier = &state.transition_frontier;
        Self {
            peers: ready_peers(state),
            sync: match SyncStatus::from_state(&frontier.sync) {
                SyncStatus::Synced if !synced_to_network(state) => SyncStatus::Waiting,
                sync => sync,
            },
            target: Some(&frontier.sync)
                .filter(|sync| !sync.is_synced())
                .and_then(|sync| sync.best_tip())
                .map(|tip| tip.height()),
            best_tip: frontier
                .best_tip()
                .filter(|tip| !tip.is_genesis())
                .map(|tip| (tip.height(), tip.hash().to_string())),
        }
    }
}

/// The frontier starts "synced" to the genesis block, and is "synced" again
/// to a tip restored from the frontier store, before any peer sent a best
/// tip: neither is synced to the network. The node is when `/status` says
/// `Synced` (F-dash3).
fn synced_to_network(state: &State) -> bool {
    matches!(state.reported_sync_phase(), SyncPhase::Synced)
}

/// A line [`StatusLog`] logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusEvent {
    Status(StatusSnapshot),
    /// Synced for the first time since the start.
    FirstSynced {
        after: Duration,
    },
    /// Not synced for [`StatusLog::LOST_SYNC_AFTER`], after being synced.
    LostSync(StatusSnapshot),
    /// Synced again after losing sync.
    Resynced {
        after: Duration,
    },
}

/// SNARK work of ours in a block of the best chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoldWork {
    pub height: u32,
    pub hash: String,
    /// Bundles (one or two proofs each) and their fees, in nanomina.
    pub bundles: usize,
    pub fees: u64,
}

pub struct StatusLog {
    last_update: Option<Timestamp>,
    started: Option<Timestamp>,
    /// When the last status line was logged, and what it said.
    last_status: Option<(Timestamp, StatusSnapshot)>,
    synced_once: bool,
    /// Whether the last line about sync said the node is synced.
    in_sync: bool,
    /// Since when the node has not been synced, while `in_sync`.
    unsynced_since: Option<Timestamp>,
    /// When the node lost sync, while not `in_sync`.
    lost_at: Option<Timestamp>,
    /// The best chain's blocks already looked at for SNARK work we sold.
    seen_blocks: BTreeSet<StateHash>,
    seen_tip: Option<StateHash>,
}

impl Default for StatusLog {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusLog {
    /// At most one status line this often...
    pub const INTERVAL: Duration = Duration::from_secs(60);
    /// ...and one at least this often, changed or not.
    pub const HEARTBEAT: Duration = Duration::from_secs(600);
    /// A node gets each new block in a few seconds: being unsynced this long
    /// is a lost sync.
    pub const LOST_SYNC_AFTER: Duration = Duration::from_secs(60);

    /// How often [`Self::update`] looks at the state.
    pub const CHECK_INTERVAL: Duration = Duration::from_secs(1);

    pub fn new() -> Self {
        Self {
            last_update: None,
            started: None,
            last_status: None,
            synced_once: false,
            in_sync: false,
            unsynced_since: None,
            lost_at: None,
            seen_blocks: Default::default(),
            seen_tip: None,
        }
    }

    /// Logs what [`Self::observe`] and [`Self::sold_work`] find, looking at
    /// `state` at most once per [`Self::CHECK_INTERVAL`].
    pub fn update(&mut self, state: &State) {
        let now = state.time();
        if self
            .last_update
            .is_some_and(|last| now.checked_sub(last).unwrap_or_default() < Self::CHECK_INTERVAL)
        {
            return;
        }
        self.last_update = Some(now);

        let snapshot = StatusSnapshot::from_state(state);
        for event in self.observe(now, &snapshot) {
            match event {
                StatusEvent::Status(snapshot) => mina_core::info!(
                    now;
                    kind = "NodeStatus",
                    summary = snapshot.to_string(),
                    peers = snapshot.peers,
                ),
                StatusEvent::FirstSynced { after } => mina_core::info!(
                    now;
                    kind = "NodeSynced",
                    summary = format!("synced, {} after the start", HumanDuration(after)),
                ),
                StatusEvent::LostSync(snapshot) => mina_core::warn!(
                    now;
                    kind = "NodeLostSync",
                    summary = format!(
                        "lost sync, not synced for {}: {snapshot}",
                        HumanDuration(Self::LOST_SYNC_AFTER)
                    ),
                ),
                StatusEvent::Resynced { after } => mina_core::info!(
                    now;
                    kind = "NodeSynced",
                    summary = format!("synced again, {} after losing sync", HumanDuration(after)),
                ),
            }
        }
        for sold in self.sold_work(state) {
            mina_core::info!(
                now;
                kind = "SnarkWorkSold",
                summary = format!(
                    "sold {} SNARK work bundle{} for {} MINA in block {} {}",
                    sold.bundles,
                    if sold.bundles == 1 { "" } else { "s" },
                    Mina(sold.fees),
                    sold.height,
                    short_hash(&sold.hash),
                ),
                block_hash = sold.hash,
            );
        }
    }

    /// Our SNARK work in the best chain's blocks not looked at yet, once
    /// the node is synced. The blocks of the first synced chain are only
    /// marked as seen: their work was sold before this run, if at all.
    pub fn sold_work(&mut self, state: &State) -> Vec<SoldWork> {
        let frontier = &state.transition_frontier;
        let Some(prover) = state.config.snarker.as_ref().map(|c| c.public_key.as_ref()) else {
            return Vec::new();
        };
        let Some(tip) = frontier.best_tip().filter(|_| synced_to_network(state)) else {
            return Vec::new();
        };
        if self.seen_tip.as_ref() == Some(tip.hash()) {
            return Vec::new();
        }
        let chain = frontier.best_chain.iter().map(|block| {
            let fees = block
                .completed_works_iter()
                .filter(|work| &work.prover == prover)
                .map(|work| work.fee.as_u64())
                .collect();
            (block.hash().clone(), block.height(), fees)
        });
        self.sold_in(tip.hash().clone(), chain)
    }

    /// [`Self::sold_work`] on the best chain, root first: each block's hash,
    /// height and the fees of our work it includes.
    fn sold_in(
        &mut self,
        tip: StateHash,
        chain: impl Iterator<Item = (StateHash, u32, Vec<u64>)>,
    ) -> Vec<SoldWork> {
        let first = self.seen_tip.replace(tip).is_none();
        let mut seen = BTreeSet::new();
        let mut sold = Vec::new();
        for (hash, height, fees) in chain {
            if !first && !fees.is_empty() && !self.seen_blocks.contains(&hash) {
                sold.push(SoldWork {
                    height,
                    hash: hash.to_string(),
                    bundles: fees.len(),
                    fees: fees
                        .iter()
                        .fold(0, |sum: u64, fee| sum.saturating_add(*fee)),
                });
            }
            seen.insert(hash);
        }
        self.seen_blocks = seen;
        sold
    }

    /// What to log at `now`, given the node's status.
    pub fn observe(&mut self, now: Timestamp, snapshot: &StatusSnapshot) -> Vec<StatusEvent> {
        let since = |then: Timestamp| now.checked_sub(then).unwrap_or_default();
        let started = *self.started.get_or_insert(now);
        let synced = snapshot.sync == SyncStatus::Synced;
        let mut events = Vec::new();

        if synced {
            self.unsynced_since = None;
            if !self.synced_once {
                self.synced_once = true;
                events.push(StatusEvent::FirstSynced {
                    after: since(started),
                });
            } else if !self.in_sync {
                // `unsynced_since` was cleared when the loss was logged; the
                // time is in `lost_at`.
                events.push(StatusEvent::Resynced {
                    after: self.lost_at.map(since).unwrap_or_default(),
                });
            }
            self.in_sync = true;
            self.lost_at = None;
        } else if self.in_sync {
            let unsynced_since = *self.unsynced_since.get_or_insert(now);
            if since(unsynced_since) >= Self::LOST_SYNC_AFTER {
                self.in_sync = false;
                self.lost_at = Some(unsynced_since);
                self.unsynced_since = None;
                events.push(StatusEvent::LostSync(snapshot.clone()));
            }
        }

        let due = match &self.last_status {
            None => true,
            Some((at, last)) => {
                since(*at) >= Self::HEARTBEAT || (since(*at) >= Self::INTERVAL && last != snapshot)
            }
        };
        if due {
            self.last_status = Some((now, snapshot.clone()));
            events.push(StatusEvent::Status(snapshot.clone()));
        }
        events
    }
}

/// `45s`, `12m 30s`, `2h 05m`.
struct HumanDuration(Duration);

impl fmt::Display for HumanDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secs = self.0.as_secs();
        match secs {
            0..60 => write!(f, "{secs}s"),
            60..3600 => write!(f, "{}m {:02}s", secs / 60, secs % 60),
            _ => write!(f, "{}h {:02}m", secs / 3600, secs % 3600 / 60),
        }
    }
}

/// Nanomina as MINA, without trailing zeros: `0.1`, `2`.
struct Mina(u64);

impl fmt::Display for Mina {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (whole, nano) = (self.0 / 1_000_000_000, self.0 % 1_000_000_000);
        if nano == 0 {
            write!(f, "{whole}")
        } else {
            let fraction = format!("{nano:09}");
            write!(f, "{whole}.{}", fraction.trim_end_matches('0'))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> Timestamp {
        Timestamp::new(1_000_000_000_000 + secs * 1_000_000_000)
    }

    fn snapshot(peers: usize, sync: SyncStatus) -> StatusSnapshot {
        StatusSnapshot {
            peers,
            sync,
            target: None,
            best_tip: None,
        }
    }

    fn statuses(events: &[StatusEvent]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, StatusEvent::Status(_)))
            .count()
    }

    #[test]
    fn status_is_logged_at_most_once_a_minute_and_only_when_it_changed() {
        let mut log = StatusLog::new();
        assert_eq!(
            statuses(&log.observe(at(0), &snapshot(5, SyncStatus::Waiting))),
            1
        );
        // A change within the minute waits.
        assert_eq!(
            statuses(&log.observe(at(10), &snapshot(6, SyncStatus::Waiting))),
            0
        );
        assert_eq!(
            statuses(&log.observe(at(59), &snapshot(7, SyncStatus::Waiting))),
            0
        );
        // A minute after the last line, the change is logged.
        let events = log.observe(at(60), &snapshot(7, SyncStatus::Waiting));
        assert_eq!(
            events,
            vec![StatusEvent::Status(snapshot(7, SyncStatus::Waiting))]
        );
        // No change: nothing, until the heartbeat.
        for secs in [120, 300, 659] {
            assert_eq!(
                statuses(&log.observe(at(secs), &snapshot(7, SyncStatus::Waiting))),
                0
            );
        }
        assert_eq!(
            statuses(&log.observe(at(660), &snapshot(7, SyncStatus::Waiting))),
            1
        );
    }

    #[test]
    fn first_sync_lost_sync_and_resync_are_logged_once_each() {
        let mut log = StatusLog::new();
        let syncing = snapshot(
            5,
            SyncStatus::Blocks {
                fetched: 1,
                applied: 0,
                total: 2,
            },
        );
        let synced = snapshot(5, SyncStatus::Synced);
        let sync_events = |events: Vec<StatusEvent>| {
            events
                .into_iter()
                .filter(|event| !matches!(event, StatusEvent::Status(_)))
                .collect::<Vec<_>>()
        };

        assert_eq!(sync_events(log.observe(at(0), &syncing)), vec![]);
        assert_eq!(
            sync_events(log.observe(at(750), &synced)),
            vec![StatusEvent::FirstSynced {
                after: Duration::from_secs(750)
            }]
        );
        // Each new block is a short catch-up: not a lost sync.
        assert_eq!(sync_events(log.observe(at(800), &syncing)), vec![]);
        assert_eq!(sync_events(log.observe(at(805), &synced)), vec![]);
        // A minute without sync is.
        assert_eq!(sync_events(log.observe(at(900), &syncing)), vec![]);
        assert_eq!(sync_events(log.observe(at(959), &syncing)), vec![]);
        assert_eq!(
            sync_events(log.observe(at(960), &syncing)),
            vec![StatusEvent::LostSync(syncing.clone())]
        );
        assert_eq!(sync_events(log.observe(at(1000), &syncing)), vec![]);
        assert_eq!(
            sync_events(log.observe(at(1020), &synced)),
            vec![StatusEvent::Resynced {
                after: Duration::from_secs(120)
            }]
        );
        assert_eq!(sync_events(log.observe(at(1030), &synced)), vec![]);
    }

    #[test]
    fn status_line_reads_as_a_sentence() {
        let status = StatusSnapshot {
            peers: 12,
            sync: SyncStatus::Ledger {
                ledger: "staking epoch",
                progress: Some((1_500, 6_000)),
                staged: false,
            },
            target: Some(578_470),
            best_tip: Some((
                578_465,
                "3NK6FYGaHWPokFhk8KYSbVnET5XrdVqEVKmxkWt4bfCNTJmrie5D".to_owned(),
            )),
        };
        assert_eq!(
            status.to_string(),
            "syncing the staking epoch ledger, 25% (1500 of about 6000 accounts and hashes) \
             to height 578470, best tip 578465 3NK6FYGa..mrie5D, 12 peers"
        );
        let synced = StatusSnapshot {
            peers: 1,
            sync: SyncStatus::Synced,
            target: None,
            best_tip: None,
        };
        assert_eq!(synced.to_string(), "synced, 1 peer");
    }

    #[test]
    fn sold_work_is_reported_once_per_block_and_not_for_the_first_chain() {
        let hash = |n: u64| StateHash::from_fp(n.into());
        let chain = |blocks: &[(u64, &[u64])]| {
            blocks
                .iter()
                .map(|(n, fees)| (hash(*n), *n as u32, fees.to_vec()))
                .collect::<Vec<_>>()
                .into_iter()
        };
        let mut log = StatusLog::new();
        // The chain the node first syncs to: sold before this run, if at all.
        assert_eq!(
            log.sold_in(hash(3), chain(&[(1, &[5]), (2, &[]), (3, &[7])])),
            vec![]
        );
        // A new block with two bundles of ours.
        let sold = log.sold_in(hash(4), chain(&[(2, &[]), (3, &[7]), (4, &[100, 200])]));
        assert_eq!(
            sold,
            vec![SoldWork {
                height: 4,
                hash: hash(4).to_string(),
                bundles: 2,
                fees: 300,
            }]
        );
        // A fork: block 40 of ours replaces 4, and 6 has none of our work.
        let sold = log.sold_in(hash(6), chain(&[(3, &[7]), (40, &[1]), (6, &[])]));
        assert_eq!(sold.iter().map(|s| s.height).collect::<Vec<_>>(), vec![40]);
    }

    #[test]
    fn durations_and_fees_read_as_people_write_them() {
        let human = |secs| HumanDuration(Duration::from_secs(secs)).to_string();
        assert_eq!(human(45), "45s");
        assert_eq!(human(750), "12m 30s");
        assert_eq!(human(7_500), "2h 05m");
        assert_eq!(Mina(100_000_000).to_string(), "0.1");
        assert_eq!(Mina(2_000_000_000).to_string(), "2");
        assert_eq!(Mina(1_250_000_001).to_string(), "1.250000001");
    }
}
