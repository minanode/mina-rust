use std::{collections::LinkedList, sync::Arc};

use binprot_derive::{BinProtRead, BinProtWrite};
use derive_more::From;
use mina_p2p_messages::{
    list::List,
    number::{UInt32, UInt64},
    v2,
};
use serde::{Deserialize, Serialize};

use crate::channels::rpc::StagedLedgerAuxAndPendingCoinbases;

pub type StagedLedgerPartsResponseFull = Arc<StagedLedgerAuxAndPendingCoinbases>;

/// Smaller sized parts of the staged ledger parts packed in one message.
#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, Debug, Clone)]
pub struct StagedLedgerPartsBase {
    pub staged_ledger_hash: v2::LedgerHash,
    pub pending_coinbase: Box<v2::MinaBasePendingCoinbaseStableV2>,
    pub needed_blocks: List<v2::MinaStateProtocolStateValueStableV2>,
}

/// Scan state smaller fields + newly emitted scan state proof (if any).
#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, Debug, Clone)]
pub struct ScanStateBase {
    pub trees: UInt32,
    pub acc: Option<(
        Box<v2::LedgerProofProdStableV2>,
        List<v2::TransactionSnarkScanStateTransactionWithWitnessStableV2>,
    )>,
    pub curr_job_seq_no: UInt64,
    pub max_base_jobs: UInt64,
    pub delay: UInt64,
}

impl ScanStateBase {
    /// Whether `trees` is at most the number of trees a scan state can have
    /// with this network's constants: OCaml's `max_trees`.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/parallel_scan/parallel_scan.ml#L1053-L1054>
    pub fn trees_within_bound(&self) -> bool {
        let constants = mina_core::constants::constraint_constants();
        let max_trees = (constants.transaction_capacity_log_2 + 1) * (constants.work_delay + 1) + 1;
        u64::from(self.trees.as_u32()) <= max_trees
    }
}

/// Scan state previous incomplete zkapp updates.
pub type PreviousIncompleteZkappUpdates = (
    List<v2::TransactionSnarkScanStateTransactionWithWitnessStableV2>,
    v2::TransactionSnarkScanStateStableV2PreviousIncompleteZkappUpdates1,
);

/// Individual scan state tree.
pub type ScanStateTree = v2::TransactionSnarkScanStateStableV2ScanStateTreesA;

#[derive(BinProtWrite, BinProtRead, Serialize, Deserialize, From, Debug, Clone)]
pub enum StagedLedgerPartsResponse {
    Base(StagedLedgerPartsBase),
    ScanStateBase(ScanStateBase),
    PreviousIncompleteZkappUpdates(PreviousIncompleteZkappUpdates),
    ScanStateTree(ScanStateTree),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum StagedLedgerPartsSendProgress {
    /// Haven't yet requested from ledger to fetch the parts.
    LedgerGetIdle {
        time: redux::Timestamp,
    },
    LedgerGetPending {
        time: redux::Timestamp,
    },
    LedgerGetSuccess {
        time: redux::Timestamp,
        data: Option<StagedLedgerPartsResponseFull>,
    },
    BaseSent {
        time: redux::Timestamp,
        data: StagedLedgerPartsResponseFull,
    },
    ScanStateBaseSent {
        time: redux::Timestamp,
        data: StagedLedgerPartsResponseFull,
    },
    PreviousIncompleteZkappUpdatesSent {
        time: redux::Timestamp,
        data: StagedLedgerPartsResponseFull,
    },
    ScanStateTreesSending {
        time: redux::Timestamp,
        data: StagedLedgerPartsResponseFull,
        tree_index: usize,
    },
    /// Sending the parts from the last
    /// ([`super::P2pStreamingRpcRequest::StagedLedgerPartsFromLast`]):
    /// `sent` of them so far, in [`from_last_part`]'s order.
    FromLastSending {
        time: redux::Timestamp,
        data: StagedLedgerPartsResponseFull,
        sent: usize,
    },
    Success {
        time: redux::Timestamp,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum StagedLedgerPartsReceiveProgress {
    BasePending {
        time: redux::Timestamp,
    },
    BaseSuccess {
        time: redux::Timestamp,
        base: StagedLedgerPartsBase,
    },
    ScanStateBasePending {
        time: redux::Timestamp,
        base: StagedLedgerPartsBase,
    },
    ScanStateBaseSuccess {
        time: redux::Timestamp,
        base: StagedLedgerPartsBase,
        scan_state_base: ScanStateBase,
    },
    PreviousIncompleteZkappUpdatesPending {
        time: redux::Timestamp,
        base: StagedLedgerPartsBase,
        scan_state_base: ScanStateBase,
    },
    PreviousIncompleteZkappUpdatesSuccess {
        time: redux::Timestamp,
        base: StagedLedgerPartsBase,
        scan_state_base: ScanStateBase,
        previous_incomplete_zkapp_updates: PreviousIncompleteZkappUpdates,
    },
    ScanStateTreesPending {
        time: redux::Timestamp,
        base: StagedLedgerPartsBase,
        scan_state_base: ScanStateBase,
        previous_incomplete_zkapp_updates: PreviousIncompleteZkappUpdates,
        trees: LinkedList<ScanStateTree>,
        is_next_tree_requested: bool,
    },
    Success {
        time: redux::Timestamp,
        data: StagedLedgerPartsResponseFull,
    },
}

fn base_part(data: &StagedLedgerAuxAndPendingCoinbases) -> StagedLedgerPartsResponse {
    StagedLedgerPartsBase {
        staged_ledger_hash: data.staged_ledger_hash.clone(),
        pending_coinbase: Box::new(data.pending_coinbase.clone()),
        needed_blocks: data.needed_blocks.clone(),
    }
    .into()
}

fn scan_state_base_part(data: &StagedLedgerAuxAndPendingCoinbases) -> StagedLedgerPartsResponse {
    ScanStateBase {
        trees: (tree_count(data) as u32).into(),
        acc: data
            .scan_state
            .scan_state
            .acc
            .as_ref()
            .map(|(a, b)| (a.clone().into(), b.clone())),
        curr_job_seq_no: data.scan_state.scan_state.curr_job_seq_no,
        max_base_jobs: data.scan_state.scan_state.max_base_jobs,
        delay: data.scan_state.scan_state.delay,
    }
    .into()
}

fn zkapp_updates_part(data: &StagedLedgerAuxAndPendingCoinbases) -> StagedLedgerPartsResponse {
    data.scan_state
        .previous_incomplete_zkapp_updates
        .clone()
        .into()
}

fn tree_count(data: &StagedLedgerAuxAndPendingCoinbases) -> usize {
    data.scan_state.scan_state.trees.1.len() + 1
}

/// The scan state's tree `index`, the first being 0.
///
/// Trees go without their completed merge jobs, as OCaml sends them. A node
/// that applied the blocks itself keeps those jobs' proofs, about half of a
/// full scan state's bytes, and the receiver neither hashes nor uses them.
fn tree_part(
    data: &StagedLedgerAuxAndPendingCoinbases,
    index: usize,
) -> Option<StagedLedgerPartsResponse> {
    let trees = &data.scan_state.scan_state.trees;
    match index.checked_sub(1) {
        None => Some(trees.0.with_leaner_trees()),
        Some(rest) => trees
            .1
            .iter()
            .nth(rest)
            .map(ScanStateTree::with_leaner_trees),
    }
    .map(Into::into)
}

/// Part `sent` of a stream from the first: the base, the scan state's
/// base, the previous incomplete zkapp updates, then the trees in order.
pub fn from_first_part(
    data: &StagedLedgerAuxAndPendingCoinbases,
    sent: usize,
) -> Option<StagedLedgerPartsResponse> {
    match sent {
        0 => Some(base_part(data)),
        1 => Some(scan_state_base_part(data)),
        2 => Some(zkapp_updates_part(data)),
        k => tree_part(data, k - 3),
    }
}

/// Part `sent` of a stream from the last: the scan state's base, its trees
/// last first, the previous incomplete zkapp updates, then the base. The
/// scan state's base comes first so that the receiver knows how many trees
/// to expect from either end.
pub fn from_last_part(
    data: &StagedLedgerAuxAndPendingCoinbases,
    sent: usize,
) -> Option<StagedLedgerPartsResponse> {
    let trees = tree_count(data);
    match sent {
        0 => Some(scan_state_base_part(data)),
        k if k <= trees => tree_part(data, trees - k),
        k if k == trees + 1 => Some(zkapp_updates_part(data)),
        k if k == trees + 2 => Some(base_part(data)),
        _ => None,
    }
}

/// How many parts a stream sends: the trees and three more.
pub fn parts_count(data: &StagedLedgerAuxAndPendingCoinbases) -> usize {
    tree_count(data) + 3
}

impl StagedLedgerPartsSendProgress {
    pub fn next_msg(&self) -> Option<StagedLedgerPartsResponse> {
        match self {
            Self::LedgerGetSuccess { data, .. } => data.as_deref().map(base_part),
            Self::BaseSent { data, .. } => Some(scan_state_base_part(data)),
            Self::ScanStateBaseSent { data, .. } => Some(zkapp_updates_part(data)),
            Self::PreviousIncompleteZkappUpdatesSent { data, .. } => tree_part(data, 0),
            Self::ScanStateTreesSending {
                data, tree_index, ..
            } => tree_part(data, tree_index + 1),
            Self::FromLastSending { data, sent, .. } => from_last_part(data, *sent),
            _ => None,
        }
    }
}

impl Default for StagedLedgerPartsSendProgress {
    fn default() -> Self {
        Self::LedgerGetIdle {
            time: redux::Timestamp::ZERO,
        }
    }
}

impl StagedLedgerPartsReceiveProgress {
    pub fn last_updated(&self) -> redux::Timestamp {
        match self {
            Self::BasePending { time }
            | Self::BaseSuccess { time, .. }
            | Self::PreviousIncompleteZkappUpdatesPending { time, .. }
            | Self::PreviousIncompleteZkappUpdatesSuccess { time, .. }
            | Self::ScanStateBasePending { time, .. }
            | Self::ScanStateBaseSuccess { time, .. }
            | Self::ScanStateTreesPending { time, .. }
            | Self::Success { time, .. } => *time,
        }
    }

    pub fn update(&mut self, time: redux::Timestamp, resp: StagedLedgerPartsResponse) -> bool {
        match (std::mem::take(self), resp) {
            (Self::BasePending { .. }, StagedLedgerPartsResponse::Base(base)) => {
                *self = Self::BaseSuccess { time, base };
                true
            }
            (
                Self::ScanStateBasePending { base, .. },
                StagedLedgerPartsResponse::ScanStateBase(data),
            ) => {
                *self = Self::ScanStateBaseSuccess {
                    time,
                    base,
                    scan_state_base: data,
                };
                true
            }
            (
                Self::PreviousIncompleteZkappUpdatesPending {
                    base,
                    scan_state_base,
                    ..
                },
                StagedLedgerPartsResponse::PreviousIncompleteZkappUpdates(data),
            ) => {
                *self = Self::PreviousIncompleteZkappUpdatesSuccess {
                    time,
                    base,
                    scan_state_base,
                    previous_incomplete_zkapp_updates: data,
                };
                true
            }
            (
                Self::ScanStateTreesPending {
                    base,
                    scan_state_base,
                    previous_incomplete_zkapp_updates,
                    mut trees,
                    ..
                },
                StagedLedgerPartsResponse::ScanStateTree(tree),
            ) => {
                trees.extend(std::iter::once(tree));

                *self = if trees.len() >= scan_state_base.trees.as_u32() as usize {
                    // `trees` holds the tree just come.
                    let Some(data) = assemble(
                        base,
                        scan_state_base,
                        previous_incomplete_zkapp_updates,
                        trees,
                    ) else {
                        return false;
                    };
                    Self::Success {
                        time,
                        data: data.into(),
                    }
                } else {
                    Self::ScanStateTreesPending {
                        time,
                        base,
                        scan_state_base,
                        previous_incomplete_zkapp_updates,
                        trees,
                        is_next_tree_requested: false,
                    }
                };
                true
            }
            (old_state, _) => {
                *self = old_state;
                false
            }
        }
    }

    pub fn is_part_pending(&self) -> bool {
        // must match code below.
        match self {
            Self::BaseSuccess { .. }
            | Self::ScanStateBaseSuccess { .. }
            | Self::PreviousIncompleteZkappUpdatesSuccess { .. }
            | Self::Success { .. } => false,
            Self::ScanStateTreesPending {
                is_next_tree_requested,
                ..
            } => *is_next_tree_requested,
            _ => true,
        }
    }

    pub fn set_next_pending(&mut self, time: redux::Timestamp) -> bool {
        match std::mem::take(self) {
            Self::BaseSuccess { base, .. } => {
                *self = Self::ScanStateBasePending { time, base };
                true
            }
            Self::ScanStateBaseSuccess {
                base,
                scan_state_base,
                ..
            } => {
                *self = Self::PreviousIncompleteZkappUpdatesPending {
                    time,
                    base,
                    scan_state_base,
                };
                true
            }
            Self::PreviousIncompleteZkappUpdatesSuccess {
                base,
                scan_state_base,
                previous_incomplete_zkapp_updates,
                ..
            } => {
                *self = Self::ScanStateTreesPending {
                    time,
                    base,
                    scan_state_base,
                    previous_incomplete_zkapp_updates,
                    trees: Default::default(),
                    is_next_tree_requested: true,
                };
                true
            }
            Self::ScanStateTreesPending {
                time,
                base,
                scan_state_base,
                previous_incomplete_zkapp_updates,
                trees,
                ..
            } => {
                *self = Self::ScanStateTreesPending {
                    time,
                    base,
                    scan_state_base,
                    previous_incomplete_zkapp_updates,
                    trees,
                    is_next_tree_requested: true,
                };
                true
            }
            old_state => {
                *self = old_state;
                false
            }
        }
    }

    pub fn progress(&self) -> (u64, u64) {
        const EXPECTED_FETCH_TOTAL: u64 = 27;
        let total = |trees: u32| (trees as u64) + 3;
        match self {
            Self::BasePending { .. } => (0, EXPECTED_FETCH_TOTAL),
            Self::BaseSuccess { .. } | Self::ScanStateBasePending { .. } => {
                (1, EXPECTED_FETCH_TOTAL)
            }
            Self::ScanStateBaseSuccess {
                scan_state_base, ..
            }
            | Self::PreviousIncompleteZkappUpdatesPending {
                scan_state_base, ..
            } => (2, total(scan_state_base.trees.as_u32())),
            Self::PreviousIncompleteZkappUpdatesSuccess {
                scan_state_base, ..
            } => (3, total(scan_state_base.trees.as_u32())),
            Self::ScanStateTreesPending {
                scan_state_base,
                trees,
                ..
            } => (
                3 + trees.len() as u64,
                total(scan_state_base.trees.as_u32()),
            ),
            Self::Success { data, .. } => {
                let total = calc_total_pieces_to_transfer(data);
                (total, total)
            }
        }
    }
}

pub fn calc_total_pieces_to_transfer(parts: &StagedLedgerAuxAndPendingCoinbases) -> u64 {
    let total_trees = parts.scan_state.scan_state.trees.1.len() + 1;
    3 + total_trees as u64
}

impl Default for StagedLedgerPartsReceiveProgress {
    fn default() -> Self {
        Self::BasePending {
            time: redux::Timestamp::ZERO,
        }
    }
}

/// Puts the parts together, `None` without a tree: a scan state has at
/// least one.
fn assemble(
    base: StagedLedgerPartsBase,
    scan_state_base: ScanStateBase,
    previous_incomplete_zkapp_updates: PreviousIncompleteZkappUpdates,
    trees: impl IntoIterator<Item = ScanStateTree>,
) -> Option<StagedLedgerAuxAndPendingCoinbases> {
    let mut trees = trees.into_iter();
    let first = trees.next()?;
    Some(StagedLedgerAuxAndPendingCoinbases {
        scan_state: v2::TransactionSnarkScanStateStableV2 {
            scan_state: v2::TransactionSnarkScanStateStableV2ScanState {
                trees: (first, trees.collect()),
                acc: scan_state_base.acc.map(|(a, b)| (*a, b)),
                curr_job_seq_no: scan_state_base.curr_job_seq_no,
                max_base_jobs: scan_state_base.max_base_jobs,
                delay: scan_state_base.delay,
            },
            previous_incomplete_zkapp_updates,
        },
        staged_ledger_hash: base.staged_ledger_hash,
        pending_coinbase: *base.pending_coinbase,
        needed_blocks: base.needed_blocks,
    })
}

/// What a stream from the last has brought, in the order it comes: the
/// scan state's base, its trees last first, the previous incomplete zkapp
/// updates, then the base
/// ([`super::P2pStreamingRpcRequest::StagedLedgerPartsFromLast`]).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StagedLedgerPartsFromLastProgress {
    pub time: redux::Timestamp,
    pub scan_state_base: Option<ScanStateBase>,
    /// Last first.
    pub trees: Vec<ScanStateTree>,
    pub previous_incomplete_zkapp_updates: Option<PreviousIncompleteZkappUpdates>,
    pub base: Option<StagedLedgerPartsBase>,
    /// Whether a part has been asked for and not come yet. The first is
    /// sent without being asked for.
    pub is_next_requested: bool,
    /// Every part, put together, once the stream or the two together have
    /// all of them.
    pub data: Option<StagedLedgerPartsResponseFull>,
}

impl Default for StagedLedgerPartsFromLastProgress {
    fn default() -> Self {
        Self {
            time: redux::Timestamp::ZERO,
            scan_state_base: None,
            trees: Vec::new(),
            previous_incomplete_zkapp_updates: None,
            base: None,
            is_next_requested: false,
            data: None,
        }
    }
}

impl StagedLedgerPartsFromLastProgress {
    pub fn new(time: redux::Timestamp) -> Self {
        Self {
            time,
            is_next_requested: true,
            ..Default::default()
        }
    }

    fn trees_expected(&self) -> Option<usize> {
        self.scan_state_base
            .as_ref()
            .map(|base| base.trees.as_u32() as usize)
    }

    /// Whether `resp` is the next part in this stream's order.
    pub fn accepts(&self, resp: &StagedLedgerPartsResponse) -> bool {
        if self.data.is_some() {
            return false;
        }
        match (resp, self.trees_expected()) {
            (StagedLedgerPartsResponse::ScanStateBase(base), None) => base.trees.as_u32() >= 1,
            (StagedLedgerPartsResponse::ScanStateTree(_), Some(trees)) => self.trees.len() < trees,
            (StagedLedgerPartsResponse::PreviousIncompleteZkappUpdates(_), Some(trees)) => {
                self.trees.len() == trees && self.previous_incomplete_zkapp_updates.is_none()
            }
            (StagedLedgerPartsResponse::Base(_), Some(_)) => {
                self.previous_incomplete_zkapp_updates.is_some() && self.base.is_none()
            }
            _ => false,
        }
    }

    pub fn update(&mut self, time: redux::Timestamp, resp: StagedLedgerPartsResponse) -> bool {
        if !self.accepts(&resp) {
            return false;
        }
        match resp {
            StagedLedgerPartsResponse::ScanStateBase(base) => self.scan_state_base = Some(base),
            StagedLedgerPartsResponse::ScanStateTree(tree) => self.trees.push(tree),
            StagedLedgerPartsResponse::PreviousIncompleteZkappUpdates(updates) => {
                self.previous_incomplete_zkapp_updates = Some(updates)
            }
            StagedLedgerPartsResponse::Base(base) => self.base = Some(base),
        }
        self.time = time;
        self.is_next_requested = false;
        if self.base.is_some() {
            let (Some(base), Some(scan_state_base), Some(updates)) = (
                self.base.take(),
                self.scan_state_base.take(),
                self.previous_incomplete_zkapp_updates.take(),
            ) else {
                return true;
            };
            let trees = std::mem::take(&mut self.trees);
            // `accepts` took no scan state base of no trees.
            self.data =
                assemble(base, scan_state_base, updates, trees.into_iter().rev()).map(Into::into);
        }
        true
    }

    pub fn is_part_pending(&self) -> bool {
        self.is_next_requested
    }

    /// Parts come and parts expected, as [`StagedLedgerPartsReceiveProgress::progress`].
    pub fn progress(&self) -> (u64, u64) {
        const EXPECTED_FETCH_TOTAL: u64 = 27;
        if let Some(data) = &self.data {
            let total = calc_total_pieces_to_transfer(data);
            return (total, total);
        }
        let fetched = u64::from(self.scan_state_base.is_some())
            + self.trees.len() as u64
            + u64::from(self.previous_incomplete_zkapp_updates.is_some())
            + u64::from(self.base.is_some());
        let total = self
            .trees_expected()
            .map_or(EXPECTED_FETCH_TOTAL, |trees| trees as u64 + 3);
        (fetched, total)
    }

    pub fn set_next_pending(&mut self, time: redux::Timestamp) -> bool {
        if self.data.is_some() || self.is_next_requested {
            return false;
        }
        self.time = time;
        self.is_next_requested = true;
        true
    }

    fn held(&self) -> Held<'_> {
        Held {
            base: self.base.is_some(),
            scan_state_base: self.scan_state_base.as_ref(),
            zkapp_updates: self.previous_incomplete_zkapp_updates.is_some(),
            trees: self.trees.len(),
        }
    }
}

/// What a stream has brought so far, for putting two together.
struct Held<'a> {
    base: bool,
    scan_state_base: Option<&'a ScanStateBase>,
    zkapp_updates: bool,
    trees: usize,
}

impl StagedLedgerPartsReceiveProgress {
    fn held(&self) -> Option<Held<'_>> {
        let held = |base, scan_state_base, zkapp_updates, trees| Held {
            base,
            scan_state_base,
            zkapp_updates,
            trees,
        };
        Some(match self {
            Self::BasePending { .. } => held(false, None, false, 0),
            Self::BaseSuccess { .. } | Self::ScanStateBasePending { .. } => {
                held(true, None, false, 0)
            }
            Self::ScanStateBaseSuccess {
                scan_state_base, ..
            }
            | Self::PreviousIncompleteZkappUpdatesPending {
                scan_state_base, ..
            } => held(true, Some(scan_state_base), false, 0),
            Self::PreviousIncompleteZkappUpdatesSuccess {
                scan_state_base, ..
            } => held(true, Some(scan_state_base), true, 0),
            Self::ScanStateTreesPending {
                scan_state_base,
                trees,
                ..
            } => held(true, Some(scan_state_base), true, trees.len()),
            Self::Success { .. } => return None,
        })
    }

    /// The parts, for putting together, leaving the progress at its start.
    #[allow(clippy::type_complexity)]
    fn take_parts(
        &mut self,
    ) -> (
        Option<StagedLedgerPartsBase>,
        Option<ScanStateBase>,
        Option<PreviousIncompleteZkappUpdates>,
        LinkedList<ScanStateTree>,
    ) {
        match std::mem::take(self) {
            Self::BaseSuccess { base, .. } | Self::ScanStateBasePending { base, .. } => {
                (Some(base), None, None, Default::default())
            }
            Self::ScanStateBaseSuccess {
                base,
                scan_state_base,
                ..
            }
            | Self::PreviousIncompleteZkappUpdatesPending {
                base,
                scan_state_base,
                ..
            } => (Some(base), Some(scan_state_base), None, Default::default()),
            Self::PreviousIncompleteZkappUpdatesSuccess {
                base,
                scan_state_base,
                previous_incomplete_zkapp_updates,
                ..
            } => (
                Some(base),
                Some(scan_state_base),
                Some(previous_incomplete_zkapp_updates),
                Default::default(),
            ),
            Self::ScanStateTreesPending {
                base,
                scan_state_base,
                previous_incomplete_zkapp_updates,
                trees,
                ..
            } => (
                Some(base),
                Some(scan_state_base),
                Some(previous_incomplete_zkapp_updates),
                trees,
            ),
            Self::BasePending { .. } | Self::Success { .. } => {
                (None, None, None, Default::default())
            }
        }
    }
}

/// Whether two scan state bases describe the same scan state, as far as
/// putting trees together goes: an honest peer sends the same staged ledger
/// for a block, and the hash check after catches the rest.
fn same_shape(a: &ScanStateBase, b: &ScanStateBase) -> bool {
    a.trees == b.trees
        && a.curr_job_seq_no == b.curr_job_seq_no
        && a.max_base_jobs == b.max_base_jobs
        && a.delay == b.delay
}

/// Whether a stream from the first and one from the last of the same staged
/// ledger have every part between them. `None` if they can't be put
/// together: either is done on its own, or their scan state bases differ.
pub fn staged_ledger_parts_complete(
    first: &StagedLedgerPartsReceiveProgress,
    last: &StagedLedgerPartsFromLastProgress,
) -> Option<bool> {
    let first = first.held()?;
    if last.data.is_some() {
        return None;
    }
    let last = last.held();
    let scan_state_base = match (first.scan_state_base, last.scan_state_base) {
        (Some(a), Some(b)) if !same_shape(a, b) => return None,
        (a, b) => a.or(b),
    };
    let Some(scan_state_base) = scan_state_base else {
        return Some(false);
    };
    let trees = scan_state_base.trees.as_u32() as usize;
    Some(
        (first.base || last.base)
            && (first.zkapp_updates || last.zkapp_updates)
            && first.trees.saturating_add(last.trees) >= trees,
    )
}

/// Puts two streams of the same staged ledger together, from its two ends:
/// the trees from the first up to where those from the last begin. Call it
/// once [`staged_ledger_parts_complete`]; it leaves both progresses at
/// their start.
pub fn take_staged_ledger_parts(
    first: &mut StagedLedgerPartsReceiveProgress,
    last: &mut StagedLedgerPartsFromLastProgress,
) -> Option<StagedLedgerAuxAndPendingCoinbases> {
    let (base, scan_state_base, zkapp_updates, first_trees) = first.take_parts();
    let last = std::mem::take(last);
    let base = base.or(last.base)?;
    let scan_state_base = scan_state_base.or(last.scan_state_base)?;
    let zkapp_updates = zkapp_updates.or(last.previous_incomplete_zkapp_updates)?;
    let trees = scan_state_base.trees.as_u32() as usize;
    let from_first = trees.checked_sub(last.trees.len())?;
    if first_trees.len() < from_first {
        return None;
    }
    let trees = first_trees
        .into_iter()
        .take(from_first)
        .chain(last.trees.into_iter().rev());
    assemble(base, scan_state_base, zkapp_updates, trees)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A staged ledger of `trees` scan state trees, tree `i` marked by a
    /// base weight of `i`.
    fn staged_ledger(trees: u64) -> StagedLedgerAuxAndPendingCoinbases {
        let tree = |i: u64| {
            v2::TransactionSnarkScanStateStableV2ScanStateTreesA::Leaf(vec![(
                v2::ParallelScanWeightStableV1 {
                    base: i.into(),
                    merge: 0.into(),
                },
                v2::TransactionSnarkScanStateStableV2ScanStateTreesABaseT1::Empty,
            )])
        };
        StagedLedgerAuxAndPendingCoinbases {
            scan_state: v2::TransactionSnarkScanStateStableV2 {
                scan_state: v2::TransactionSnarkScanStateStableV2ScanState {
                    trees: (tree(0), (1..trees).map(tree).collect()),
                    acc: None,
                    curr_job_seq_no: 7.into(),
                    max_base_jobs: 1.into(),
                    delay: 2.into(),
                },
                previous_incomplete_zkapp_updates: (
                    List::new(),
                    v2::TransactionSnarkScanStateStableV2PreviousIncompleteZkappUpdates1::Border_block_continued_in_the_next_tree(false),
                ),
            },
            staged_ledger_hash: v2::LedgerHash::zero(),
            pending_coinbase: v2::MinaBasePendingCoinbaseStableV2 {
                tree: v2::MinaBasePendingCoinbaseMerkleTreeVersionedStableV2 {
                    indexes: List::new(),
                    depth: 5.into(),
                    tree: v2::MinaBasePendingCoinbaseMerkleTreeVersionedStableV2Tree::Hash(
                        v2::MinaBasePendingCoinbaseHashVersionedStableV1(
                            v2::MinaBasePendingCoinbaseHashBuilderStableV1(
                                mina_p2p_messages::bigint::BigInt::zero(),
                            ),
                        )
                        .into(),
                    ),
                },
                pos_list: List::new(),
                new_pos: v2::MinaBasePendingCoinbaseStackIdStableV1(0.into()),
            },
            needed_blocks: List::new(),
        }
    }

    fn tree_marks(data: &StagedLedgerAuxAndPendingCoinbases) -> Vec<u64> {
        let trees = &data.scan_state.scan_state.trees;
        std::iter::once(&trees.0)
            .chain(trees.1.iter())
            .map(|tree| match tree {
                v2::TransactionSnarkScanStateStableV2ScanStateTreesA::Leaf(leaves) => {
                    leaves[0].0.base.as_u64()
                }
                _ => u64::MAX,
            })
            .collect()
    }

    fn part_mark(part: &StagedLedgerPartsResponse) -> String {
        match part {
            StagedLedgerPartsResponse::Base(_) => "base".to_owned(),
            StagedLedgerPartsResponse::ScanStateBase(_) => "scan".to_owned(),
            StagedLedgerPartsResponse::PreviousIncompleteZkappUpdates(_) => "zkapp".to_owned(),
            StagedLedgerPartsResponse::ScanStateTree(
                v2::TransactionSnarkScanStateStableV2ScanStateTreesA::Leaf(leaves),
            ) => format!("t{}", leaves[0].0.base.as_u64()),
            StagedLedgerPartsResponse::ScanStateTree(_) => "tree".to_owned(),
        }
    }

    /// The parts a stream from the first sends, in order.
    fn parts_from_first(
        data: &StagedLedgerAuxAndPendingCoinbases,
    ) -> Vec<StagedLedgerPartsResponse> {
        (0..)
            .map_while(|sent| from_first_part(data, sent))
            .collect()
    }

    fn parts_from_last(
        data: &StagedLedgerAuxAndPendingCoinbases,
    ) -> Vec<StagedLedgerPartsResponse> {
        (0..).map_while(|sent| from_last_part(data, sent)).collect()
    }

    fn at(n: u64) -> redux::Timestamp {
        redux::Timestamp::new(n)
    }

    fn first_after(parts: &[StagedLedgerPartsResponse]) -> StagedLedgerPartsReceiveProgress {
        let mut progress = StagedLedgerPartsReceiveProgress::BasePending { time: at(0) };
        for part in parts {
            if !progress.is_part_pending() {
                assert!(progress.set_next_pending(at(1)));
            }
            assert!(progress.update(at(1), part.clone()), "{progress:?}");
        }
        progress
    }

    fn last_after(parts: &[StagedLedgerPartsResponse]) -> StagedLedgerPartsFromLastProgress {
        let mut progress = StagedLedgerPartsFromLastProgress::new(at(0));
        for part in parts {
            if !progress.is_part_pending() {
                assert!(progress.set_next_pending(at(1)));
            }
            assert!(progress.update(at(1), part.clone()), "{progress:?}");
        }
        progress
    }

    #[test]
    fn a_stream_from_the_last_sends_the_trees_last_first() {
        let data = staged_ledger(4);
        let order: Vec<_> = parts_from_last(&data).iter().map(part_mark).collect();
        assert_eq!(
            order,
            ["scan", "t3", "t2", "t1", "t0", "zkapp", "base"].map(String::from)
        );
        assert_eq!(parts_count(&data), 7);
    }

    /// Alone, a stream from the last brings the whole staged ledger, as one
    /// from the first does.
    #[test]
    fn a_stream_from_the_last_alone_brings_every_part() {
        let data = staged_ledger(4);
        let progress = last_after(&parts_from_last(&data));
        assert_eq!(progress.progress(), (7, 7));
        let got = progress.data.expect("done");
        assert_eq!(tree_marks(&got), [0, 1, 2, 3]);
    }

    /// A part out of order, or a scan state of no trees, is not taken.
    #[test]
    fn a_stream_from_the_last_takes_its_parts_in_order_only() {
        let data = staged_ledger(3);
        let parts = parts_from_last(&data);
        let fresh = StagedLedgerPartsFromLastProgress::new(at(0));
        assert!(
            !fresh.accepts(&parts[1]),
            "a tree before the scan state base"
        );
        assert!(!fresh.accepts(&base_part(&data)));
        let after_trees = last_after(&parts[..4]);
        assert!(!after_trees.accepts(&parts[1]), "a fourth tree of three");
        assert!(
            !after_trees.accepts(&base_part(&data)),
            "the base before the zkapp updates"
        );
        assert!(after_trees.accepts(&parts[4]));

        let StagedLedgerPartsResponse::ScanStateBase(mut empty) = scan_state_base_part(&data)
        else {
            unreachable!()
        };
        empty.trees = 0.into();
        assert!(!fresh.accepts(&empty.into()));
    }

    /// Two streams of five trees: the first has the bases and two trees,
    /// the last three trees. Together they have all of it, in order.
    #[test]
    fn two_streams_that_meet_are_put_together_in_order() {
        let data = staged_ledger(5);
        let first_parts = parts_from_first(&data);
        let last_parts = parts_from_last(&data);

        let mut first = first_after(&first_parts[..5]);
        let mut last = last_after(&last_parts[..3]);
        assert_eq!(staged_ledger_parts_complete(&first, &last), Some(false));
        last = last_after(&last_parts[..4]);
        assert_eq!(staged_ledger_parts_complete(&first, &last), Some(true));

        let got = take_staged_ledger_parts(&mut first, &mut last).expect("complete");
        assert_eq!(tree_marks(&got), [0, 1, 2, 3, 4]);
        assert_eq!(got.scan_state.scan_state.curr_job_seq_no.as_u64(), 7);
    }

    /// The last tree may come from both, the faster stream not waiting for
    /// the slower: it is taken once.
    #[test]
    fn a_tree_from_both_is_taken_once() {
        let data = staged_ledger(5);
        let mut first = first_after(&parts_from_first(&data)[..6]);
        let mut last = last_after(&parts_from_last(&data)[..4]);
        assert_eq!(staged_ledger_parts_complete(&first, &last), Some(true));
        let got = take_staged_ledger_parts(&mut first, &mut last).expect("complete");
        assert_eq!(tree_marks(&got), [0, 1, 2, 3, 4]);
    }

    /// When the first has not got past its base, the last brings the rest,
    /// the zkapp updates included.
    #[test]
    fn the_last_can_bring_nearly_everything() {
        let data = staged_ledger(3);
        let mut first = first_after(&parts_from_first(&data)[..1]);
        let mut last = last_after(&parts_from_last(&data)[..4]);
        assert_eq!(staged_ledger_parts_complete(&first, &last), Some(false));
        last = last_after(&parts_from_last(&data)[..5]);
        assert_eq!(staged_ledger_parts_complete(&first, &last), Some(true));
        let got = take_staged_ledger_parts(&mut first, &mut last).expect("complete");
        assert_eq!(tree_marks(&got), [0, 1, 2]);
    }

    /// Streams of different scan states are not put together, and a stream
    /// done on its own is not either.
    #[test]
    fn streams_of_different_scan_states_are_not_put_together() {
        let data = staged_ledger(5);
        let mut other = staged_ledger(5);
        other.scan_state.scan_state.curr_job_seq_no = 8.into();
        let first = first_after(&parts_from_first(&data)[..5]);
        let last = last_after(&parts_from_last(&other)[..4]);
        assert_eq!(staged_ledger_parts_complete(&first, &last), None);

        let done = first_after(&parts_from_first(&data));
        assert!(matches!(
            done,
            StagedLedgerPartsReceiveProgress::Success { .. }
        ));
        let last = last_after(&parts_from_last(&data)[..2]);
        assert_eq!(staged_ledger_parts_complete(&done, &last), None);
    }
}
