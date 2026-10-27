//! The running log of our SNARK work that blocks included (P5.4): for each
//! best-chain block that bought work from our worker's key, its height, hash,
//! time, producer, our bundles in it and their fees, and whether the block is
//! still pending, final (at or behind the root) or dropped by a reorg.
//!
//! A bundle is one completed work of a block, one or two proofs, as OCaml's
//! `Transaction_snark_work.t`. Its fee goes to its prover in the block's fee
//! transfers.
//!
//! The log is bounded ([`MAX_LOG_BLOCKS`]) and kept in the work directory
//! (the service writes it whenever its [`IncludedWorkLog::revision`]
//! changes), so it survives restarts. Totals of final work count since the
//! log began, beyond the entries kept. It looks for the work of the last
//! [`MAX_WATCHED_KEYS`] (eight) keys the worker has used: an older key's
//! work in later blocks is not logged.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use mina_core::{
    block::AppliedBlock,
    snark::{Snark, SnarkJobId},
};
use mina_p2p_messages::v2;
use serde::{Deserialize, Serialize};

use crate::account::AccountPublicKey;

/// Blocks kept in the log, the newest; older entries are dropped, their
/// final totals kept.
pub const MAX_LOG_BLOCKS: usize = 500;
/// Keys the log looks for: every key the worker has used, the newest.
pub const MAX_WATCHED_KEYS: usize = 8;
/// zkApp command bundles our worker offered, the newest, to tell which
/// included bundles prove a zkApp command.
pub const MAX_ZKAPP_JOBS: usize = 500;
/// The newest best-chain blocks that bought work, in [`SnarkMarket`].
pub const MARKET_PURCHASES: usize = 5;
/// Best-chain blocks the market looks back over, newest first: `k`, about
/// the frontier, so a purchase hours ago still shows. The market's cost is
/// linear in their completed works.
pub const MARKET_BLOCKS: usize = 290;
/// Provers in [`SnarkMarket::offers`]: the cheapest, plus this node's own
/// keys (at most [`MAX_WATCHED_KEYS`]) whatever they ask.
pub const MARKET_OFFERS: usize = 10;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IncludedWorkStatus {
    /// In the best chain, above the root: a reorg may still drop it.
    Pending,
    /// At or behind the root: it stays.
    Final,
    /// No longer in the best chain: another block holds its height.
    Dropped,
    /// The node lost sight of the block while it was pending: it restarted,
    /// or switched chains, more than `k` blocks later. Not counted.
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct IncludedWorkEntry {
    pub height: u32,
    pub state_hash: v2::StateHash,
    /// The block's timestamp, in milliseconds since the Unix epoch.
    pub time_ms: u64,
    /// The block's creator.
    pub producer: v2::NonZeroCurvePoint,
    /// Our key whose bundles the block bought (the first, if several).
    pub prover: v2::NonZeroCurvePoint,
    /// Our bundles in the block.
    pub bundles: u32,
    /// Those of them known to prove a zkApp command.
    pub zkapp_bundles: u32,
    /// The fees of our bundles, in nanomina.
    pub fees: u64,
    pub status: IncludedWorkStatus,
}

/// Final work since the log began.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct IncludedWorkTotals {
    pub blocks: u64,
    pub bundles: u64,
    pub zkapp_bundles: u64,
    /// In nanomina.
    pub fees: u64,
}

impl IncludedWorkTotals {
    fn add(&mut self, entry: &IncludedWorkEntry) {
        self.blocks = self.blocks.saturating_add(1);
        self.bundles = self.bundles.saturating_add(entry.bundles.into());
        self.zkapp_bundles = self
            .zkapp_bundles
            .saturating_add(entry.zkapp_bundles.into());
        self.fees = self.fees.saturating_add(entry.fees);
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct IncludedWorkLog {
    /// Keys whose work is logged, oldest first: at most
    /// [`MAX_WATCHED_KEYS`], the latest the worker used.
    pub keys: Vec<v2::NonZeroCurvePoint>,
    /// When the log began, in milliseconds since the Unix epoch.
    pub started_ms: Option<u64>,
    /// Oldest first.
    pub entries: VecDeque<IncludedWorkEntry>,
    pub final_totals: IncludedWorkTotals,
    /// zkApp command bundles our worker offered, oldest first.
    pub zkapp_jobs: VecDeque<SnarkJobId>,
    /// Changes whenever the log does: the service saves it then.
    #[serde(default)]
    pub revision: u64,
}

impl IncludedWorkLog {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.entries.is_empty()
    }

    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Looks for `key`'s work in the blocks from now on.
    pub fn watch(&mut self, key: &AccountPublicKey, now_ms: u64) {
        let key: v2::NonZeroCurvePoint = key.clone().into();
        if self.keys.last() == Some(&key) {
            return;
        }
        self.keys.retain(|k| k != &key);
        self.keys.push(key);
        let excess = self.keys.len().saturating_sub(MAX_WATCHED_KEYS);
        self.keys.drain(..excess);
        self.started_ms.get_or_insert(now_ms);
        self.changed();
    }

    /// Our worker offered work for a zkApp command job.
    pub fn note_zkapp_job(&mut self, job_id: &SnarkJobId) {
        if self.zkapp_jobs.contains(job_id) {
            return;
        }
        self.zkapp_jobs.push_back(job_id.clone());
        if self.zkapp_jobs.len() > MAX_ZKAPP_JOBS {
            self.zkapp_jobs.pop_front();
        }
        self.changed();
    }

    /// Updates the log for a new best chain, root first. `retired_roots` are
    /// the frontier's former roots, the blocks behind the root it still
    /// knows (`RootHistory`), with their heights. `is_zkapp_job` tells whether a job is a zkApp
    /// command's, if the node still holds it. Returns whether the log
    /// changed.
    pub fn on_best_chain<'a>(
        &mut self,
        chain: &[AppliedBlock],
        retired_roots: impl IntoIterator<Item = (&'a v2::StateHash, u32)>,
        is_zkapp_job: impl Fn(&SnarkJobId) -> bool,
    ) -> bool {
        let Some(root) = chain.first() else {
            return false;
        };
        if self.keys.is_empty() && self.entries.is_empty() {
            return false;
        }
        let root_height = root.height();
        let in_chain = chain.iter().map(|b| b.hash()).collect::<BTreeSet<_>>();
        let (retired, retired_heights): (BTreeSet<_>, BTreeSet<_>) =
            retired_roots.into_iter().unzip();
        let revision = self.revision;

        // New blocks with our work.
        let logged = self
            .entries
            .iter()
            .map(|e| e.state_hash.clone())
            .collect::<BTreeSet<_>>();
        let mut added = Vec::new();
        for block in chain.iter().filter(|b| !logged.contains(b.hash())) {
            if let Some(entry) = self.entry_for(block, &is_zkapp_job) {
                added.push(entry);
            }
        }
        let mut changed = !added.is_empty();
        if changed {
            self.entries.extend(added);
            self.entries
                .make_contiguous()
                .sort_by(|a, b| (a.height, &a.state_hash).cmp(&(b.height, &b.state_hash)));
        }

        // Statuses.
        for entry in self.entries.iter_mut() {
            let status = if entry.status == IncludedWorkStatus::Final {
                continue;
            } else if retired.contains(&entry.state_hash)
                || (in_chain.contains(&entry.state_hash) && entry.height <= root_height)
            {
                IncludedWorkStatus::Final
            } else if in_chain.contains(&entry.state_hash) {
                IncludedWorkStatus::Pending
            } else if entry.height >= root_height || retired_heights.contains(&entry.height) {
                // Not in the best chain, which may take it back until the
                // root passes its height; then another block is final there.
                IncludedWorkStatus::Dropped
            } else if entry.status == IncludedWorkStatus::Pending {
                IncludedWorkStatus::Unknown
            } else {
                // Dropped for good, or unknown.
                entry.status
            };
            if status != entry.status {
                entry.status = status;
                changed = true;
                if status == IncludedWorkStatus::Final {
                    self.final_totals.add(entry);
                }
            }
        }

        let excess = self.entries.len().saturating_sub(MAX_LOG_BLOCKS);
        self.entries.drain(..excess);

        if changed || excess > 0 {
            self.changed();
        }
        self.revision != revision
    }

    fn entry_for(
        &self,
        block: &AppliedBlock,
        is_zkapp_job: &impl Fn(&SnarkJobId) -> bool,
    ) -> Option<IncludedWorkEntry> {
        let mut ours = block
            .completed_works_iter()
            .filter(|work| self.keys.contains(&work.prover))
            .peekable();
        let prover = ours.peek()?.prover.clone();
        let mut entry = IncludedWorkEntry {
            height: block.height(),
            state_hash: block.hash().clone(),
            time_ms: u64::from(block.timestamp()) / 1_000_000,
            producer: block.producer().clone(),
            prover,
            bundles: 0,
            zkapp_bundles: 0,
            fees: 0,
            status: IncludedWorkStatus::Pending,
        };
        for work in ours {
            let job_id = SnarkJobId::from(&work.proofs);
            entry.bundles = entry.bundles.saturating_add(1);
            if self.zkapp_jobs.contains(&job_id) || is_zkapp_job(&job_id) {
                entry.zkapp_bundles = entry.zkapp_bundles.saturating_add(1);
            }
            entry.fees = entry.fees.saturating_add(work.fee.0.as_u64());
        }
        Some(entry)
    }

    /// The log as the operator sees it, newest first.
    pub fn view(&self) -> RpcIncludedWork {
        RpcIncludedWork {
            network: mina_core::NetworkConfig::global().name.to_owned(),
            keys: self.keys.clone(),
            started_ms: self.started_ms,
            final_totals: self.final_totals.clone(),
            pending_totals: self.pending_totals(),
            max_blocks: MAX_LOG_BLOCKS,
            entries: self.entries.iter().rev().cloned().collect(),
            best_tip_height: None,
            market: None,
        }
    }

    /// Pending work: in the best chain, above the root.
    pub fn pending_totals(&self) -> IncludedWorkTotals {
        let mut totals = IncludedWorkTotals::default();
        for entry in &self.entries {
            if entry.status == IncludedWorkStatus::Pending {
                totals.add(entry);
            }
        }
        totals
    }
}

/// The log as the operator sees it (`GET /snarker/included`).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcIncludedWork {
    /// `devnet` or `mainnet`, for explorer links.
    pub network: String,
    pub keys: Vec<v2::NonZeroCurvePoint>,
    pub started_ms: Option<u64>,
    /// Final work since the log began.
    pub final_totals: IncludedWorkTotals,
    /// Work in pending blocks.
    pub pending_totals: IncludedWorkTotals,
    /// Blocks the log keeps.
    pub max_blocks: usize,
    /// Newest first.
    pub entries: Vec<IncludedWorkEntry>,
    /// The best tip's height when the log was read: a block's confirmations
    /// (blocks on top of it) are this minus its height.
    #[serde(default)]
    pub best_tip_height: Option<u32>,
    /// What blocks buy from every prover and what the pool offers, for
    /// comparing this worker's fee (F-ux1).
    #[serde(default)]
    pub market: Option<SnarkMarket>,
}

impl RpcIncludedWork {
    /// Adds the best tip and the market of `chain` (the best chain, root
    /// first) and `pool` (the completed work in this node's SNARK pool).
    pub fn with_market<'a>(
        mut self,
        chain: &[AppliedBlock],
        pool: impl IntoIterator<Item = &'a Snark>,
    ) -> Self {
        self.best_tip_height = chain.last().map(|tip| tip.height());
        self.market = Some(SnarkMarket::new(chain, pool, &self.keys));
        self
    }
}

/// The SNARK work market as this node sees it: what its best chain bought,
/// from every prover, and the completed work its SNARK pool holds. A block
/// buys at most one bundle per job, the cheapest it knows, so the pool keeps
/// one offer per job: the lowest.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct SnarkMarket {
    /// Best-chain blocks looked at: the newest, at most [`MARKET_BLOCKS`].
    pub blocks: u32,
    /// Of those, the blocks that bought work.
    pub buying_blocks: u32,
    /// The bundles they bought.
    pub bundles: u64,
    /// The newest blocks that bought work, newest first: at most
    /// [`MARKET_PURCHASES`].
    pub purchases: Vec<SnarkPurchase>,
    /// Completed work in the pool per prover, lowest fee first: the
    /// [`MARKET_OFFERS`] cheapest other provers and this node's own keys.
    pub offers: Vec<SnarkOffer>,
    /// Provers other than this node's keys with work in the pool, listed in
    /// `offers` or not.
    #[serde(default)]
    pub other_provers: u32,
    /// The highest fee any of them asks, in nanomina; 0 when there are none.
    #[serde(default)]
    pub highest_other_fee: u64,
}

/// A best-chain block's purchase of SNARK work.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SnarkPurchase {
    pub height: u32,
    pub state_hash: v2::StateHash,
    /// The block's timestamp, in milliseconds since the Unix epoch.
    pub time_ms: u64,
    pub bundles: u32,
    /// Distinct provers it paid.
    pub provers: u32,
    /// The lowest and highest fee of its bundles, in nanomina.
    pub min_fee: u64,
    pub max_fee: u64,
}

/// One prover's completed work in the pool.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SnarkOffer {
    pub prover: v2::NonZeroCurvePoint,
    pub bundles: u32,
    /// The lowest and highest fee of its bundles, in nanomina.
    pub min_fee: u64,
    pub max_fee: u64,
}

/// A prover's key as a hash key: its curve point's x and parity, without
/// the base58 encoding.
type ProverKey = ([u8; 32], bool);

fn prover_key(key: &v2::NonZeroCurvePoint) -> ProverKey {
    let point = key.inner();
    (point.x.to_bytes(), point.is_odd)
}

impl SnarkMarket {
    /// The market of `chain` (the best chain, root first) and `pool`, with
    /// `ours` (this node's worker keys) always among the offers. One pass over
    /// the newest [`MARKET_BLOCKS`] blocks' works and one over the pool, into
    /// hash maps; the cheapest provers come from a linear-time selection, and
    /// only the at most [`MARKET_OFFERS`] + [`MAX_WATCHED_KEYS`] listed are
    /// sorted. So the cost is linear in the works and the pool, and the
    /// answer is bounded.
    pub fn new<'a>(
        chain: &[AppliedBlock],
        pool: impl IntoIterator<Item = &'a Snark>,
        ours: &[v2::NonZeroCurvePoint],
    ) -> Self {
        let mut market = SnarkMarket::default();
        for block in chain.iter().rev().take(MARKET_BLOCKS) {
            market.blocks += 1;
            let mut purchase: Option<SnarkPurchase> = None;
            let mut provers = HashSet::new();
            for work in block.completed_works_iter() {
                let fee = work.fee.0.as_u64();
                provers.insert(prover_key(&work.prover));
                let p = purchase.get_or_insert_with(|| SnarkPurchase {
                    height: block.height(),
                    state_hash: block.hash().clone(),
                    time_ms: u64::from(block.timestamp()) / 1_000_000,
                    bundles: 0,
                    provers: 0,
                    min_fee: fee,
                    max_fee: fee,
                });
                p.bundles = p.bundles.saturating_add(1);
                p.min_fee = p.min_fee.min(fee);
                p.max_fee = p.max_fee.max(fee);
            }
            let Some(mut purchase) = purchase else {
                continue;
            };
            purchase.provers = u32::try_from(provers.len()).unwrap_or(u32::MAX);
            market.buying_blocks += 1;
            market.bundles = market.bundles.saturating_add(purchase.bundles.into());
            if market.purchases.len() < MARKET_PURCHASES {
                market.purchases.push(purchase);
            }
        }

        let mut by_prover: HashMap<ProverKey, SnarkOffer> = HashMap::new();
        for snark in pool {
            let fee = snark.fee.0.as_u64();
            let offer = by_prover
                .entry(prover_key(&snark.snarker))
                .or_insert_with(|| SnarkOffer {
                    prover: snark.snarker.clone(),
                    bundles: 0,
                    min_fee: fee,
                    max_fee: fee,
                });
            offer.bundles = offer.bundles.saturating_add(1);
            offer.min_fee = offer.min_fee.min(fee);
            offer.max_fee = offer.max_fee.max(fee);
        }
        let ours = ours.iter().map(prover_key).collect::<HashSet<_>>();
        let (mut offers, mut others): (Vec<_>, Vec<_>) = by_prover
            .into_iter()
            .partition(|(key, _)| ours.contains(key));
        market.other_provers = u32::try_from(others.len()).unwrap_or(u32::MAX);
        market.highest_other_fee = others.iter().map(|(_, o)| o.max_fee).max().unwrap_or(0);
        let cheaper = |(_, a): &(ProverKey, SnarkOffer), (_, b): &(ProverKey, SnarkOffer)| {
            (a.min_fee, std::cmp::Reverse(a.bundles), &a.prover).cmp(&(
                b.min_fee,
                std::cmp::Reverse(b.bundles),
                &b.prover,
            ))
        };
        if others.len() > MARKET_OFFERS {
            others.select_nth_unstable_by(MARKET_OFFERS, cheaper);
            others.truncate(MARKET_OFFERS);
        }
        offers.append(&mut others);
        offers.sort_unstable_by(cheaper);
        market.offers = offers.into_iter().map(|(_, offer)| offer).collect();
        market
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mina_core::block::BlockWithHash;
    use mina_p2p_messages::{
        list::List,
        rpc::{GetBestTipV2, GetCompletedSnarksV1},
        rpc_kernel::PayloadBinprotReader,
    };

    use super::*;
    use crate::account::AccountSecretKey;

    const OUR_FEE: u64 = 4_000_000;

    fn key(n: u32) -> v2::NonZeroCurvePoint {
        AccountSecretKey::deterministic(n.into())
            .public_key()
            .into()
    }

    /// A captured Mesa block without its commands: the template of the
    /// synthetic chains below. Their ancestry and contents are synthetic;
    /// only the log's bookkeeping is under test.
    fn template() -> v2::MinaBlockBlockStableV2 {
        let bytes = include_bytes!("../../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin");
        let mut block = GetBestTipV2::response_payload(&mut bytes.as_slice())
            .unwrap()
            .unwrap()
            .data;
        let diff = &mut block.body.staged_ledger_diff.diff;
        diff.0.commands = List::new();
        diff.0.completed_works = List::new();
        diff.1 = None;
        block
    }

    /// Captured completed works, whose prover and fee the tests set.
    fn works() -> Vec<v2::TransactionSnarkWorkTStableV2> {
        let bytes =
            include_bytes!("../../../p2p-messages/tests/files/mesa/get_completed_snarks_v1.bin");
        GetCompletedSnarksV1::response_payload(&mut bytes.as_slice())
            .unwrap()
            .unwrap()
            .0
            .into_iter()
            .collect()
    }

    struct Chains {
        template: v2::MinaBlockBlockStableV2,
        works: Vec<v2::TransactionSnarkWorkTStableV2>,
    }

    impl Chains {
        fn new() -> Self {
            Self {
                template: template(),
                works: works(),
            }
        }

        /// A block at `height` on `parent`; `fork` makes a sibling with
        /// another hash. It buys the `bought` works: (prover, fee, index of
        /// the captured work).
        fn block(
            &self,
            parent: Option<&AppliedBlock>,
            height: u32,
            fork: u64,
            bought: &[(v2::NonZeroCurvePoint, u64, usize)],
        ) -> AppliedBlock {
            let mut block = self.template.clone();
            let state = &mut block.header.protocol_state;
            state.body.consensus_state.blockchain_length = height.into();
            state.body.blockchain_state.timestamp = v2::BlockTimeTimeStableV1(
                (1_800_000_000_000 + u64::from(height) * 90_000 + fork).into(),
            );
            if let Some(parent) = parent {
                state.previous_state_hash = parent.hash().clone();
            }
            block.body.staged_ledger_diff.diff.0.completed_works = bought
                .iter()
                .map(|(prover, fee, index)| {
                    let mut work = self.works[*index % self.works.len()].clone();
                    work.prover = prover.clone();
                    work.fee = v2::CurrencyFeeStableV1((*fee).into());
                    work
                })
                .collect();
            AppliedBlock {
                block: BlockWithHash::try_new(Arc::new(block)).unwrap(),
                just_emitted_a_proof: false,
            }
        }

        /// Blocks `from..=to` on `parent`, buying from `ours` at heights in
        /// `ours_at` (one bundle, `OUR_FEE`) and from another key elsewhere.
        fn chain(
            &self,
            parent: Option<&AppliedBlock>,
            from: u32,
            to: u32,
            fork: u64,
            ours: &v2::NonZeroCurvePoint,
            ours_at: &[u32],
        ) -> Vec<AppliedBlock> {
            let mut chain: Vec<AppliedBlock> = Vec::new();
            for height in from..=to {
                let bought = if ours_at.contains(&height) {
                    vec![(ours.clone(), OUR_FEE, height as usize)]
                } else {
                    vec![(key(9), 10_000_000, height as usize)]
                };
                let parent = chain.last().or(parent);
                chain.push(self.block(parent, height, fork, &bought));
            }
            chain
        }
    }

    fn watching(ours: &v2::NonZeroCurvePoint) -> IncludedWorkLog {
        let mut log = IncludedWorkLog::default();
        log.watch(&ours.clone().into(), 1_000);
        log
    }

    fn update(log: &mut IncludedWorkLog, chain: &[AppliedBlock], retired: &[AppliedBlock]) -> bool {
        log.on_best_chain(
            chain,
            retired.iter().map(|b| (b.hash(), b.height())),
            |_| false,
        )
    }

    fn statuses(log: &IncludedWorkLog) -> Vec<(u32, IncludedWorkStatus)> {
        log.entries.iter().map(|e| (e.height, e.status)).collect()
    }

    #[test]
    fn our_works_are_logged_and_others_ignored() {
        let chains = Chains::new();
        let ours = key(1);
        let other = key(2);
        let root = chains.block(None, 10, 0, &[(ours.clone(), OUR_FEE, 0)]);
        let ours_and_other = chains.block(
            Some(&root),
            11,
            0,
            &[
                (ours.clone(), OUR_FEE, 1),
                (other.clone(), 10_000_000, 2),
                (ours.clone(), 5_000_000, 3),
            ],
        );
        let other_only = chains.block(Some(&ours_and_other), 12, 0, &[(other, 1, 4)]);
        let chain = vec![root.clone(), ours_and_other.clone(), other_only];

        // No key yet: nothing to look for.
        let mut log = IncludedWorkLog::default();
        assert!(!update(&mut log, &chain, &[]));
        assert!(log.entries.is_empty());

        let mut log = watching(&ours);
        assert!(update(&mut log, &chain, &[]));
        assert_eq!(
            statuses(&log),
            vec![
                (10, IncludedWorkStatus::Final),
                (11, IncludedWorkStatus::Pending)
            ]
        );
        let entry = &log.entries[1];
        assert_eq!(&entry.state_hash, ours_and_other.hash());
        assert_eq!(&entry.producer, ours_and_other.producer());
        assert_eq!(entry.prover, ours);
        assert_eq!(entry.bundles, 2);
        assert_eq!(entry.fees, OUR_FEE + 5_000_000);
        assert_eq!(entry.time_ms, 1_800_000_000_000 + 11 * 90_000);
        // The root's work is final, the rest pending.
        assert_eq!(log.final_totals.blocks, 1);
        assert_eq!(log.final_totals.fees, OUR_FEE);
        assert_eq!(log.pending_totals().bundles, 2);

        // The same chain again changes nothing.
        let revision = log.revision;
        assert!(!update(&mut log, &chain, &[]));
        assert_eq!(log.revision, revision);
        // The view is newest first.
        let view = log.view();
        assert_eq!(view.entries[0].height, 11);
        assert_eq!(view.pending_totals.fees, OUR_FEE + 5_000_000);
    }

    #[test]
    fn a_reorg_drops_an_entry_and_can_bring_it_back() {
        let chains = Chains::new();
        let ours = key(1);
        let a = chains.chain(None, 10, 14, 0, &ours, &[13]);
        let mut log = watching(&ours);
        update(&mut log, &a, &[]);
        assert_eq!(statuses(&log), vec![(13, IncludedWorkStatus::Pending)]);

        // Another chain from height 13: ours is dropped, not deleted.
        let mut b = a[..3].to_vec();
        b.extend(chains.chain(Some(&a[2]), 13, 15, 1, &ours, &[]));
        assert!(update(&mut log, &b, &[]));
        assert_eq!(statuses(&log), vec![(13, IncludedWorkStatus::Dropped)]);
        assert_eq!(log.pending_totals().bundles, 0);

        // Back to the first chain: pending again.
        assert!(update(&mut log, &a, &[]));
        assert_eq!(statuses(&log), vec![(13, IncludedWorkStatus::Pending)]);

        // The other chain wins for good: the root passes height 13.
        let mut b_on = b.clone();
        b_on.extend(chains.chain(Some(b.last().unwrap()), 16, 20, 1, &ours, &[]));
        let (retired, frontier) = b_on.split_at(5);
        update(&mut log, frontier, retired);
        assert_eq!(frontier[0].height(), 15);
        assert_eq!(statuses(&log), vec![(13, IncludedWorkStatus::Dropped)]);
        assert_eq!(log.final_totals, IncludedWorkTotals::default());
    }

    #[test]
    fn entries_become_final_behind_the_root() {
        let chains = Chains::new();
        let ours = key(1);
        let chain = chains.chain(None, 10, 30, 0, &ours, &[11, 12, 20]);
        let mut log = watching(&ours);
        update(&mut log, &chain[..5], &[]);
        assert_eq!(log.pending_totals().blocks, 2);

        // The root moves to 13: 11 and 12 are behind it, retired roots.
        update(&mut log, &chain[3..15], &chain[..3]);
        assert_eq!(
            statuses(&log),
            vec![
                (11, IncludedWorkStatus::Final),
                (12, IncludedWorkStatus::Final),
                (20, IncludedWorkStatus::Pending)
            ]
        );
        assert_eq!(log.final_totals.blocks, 2);
        assert_eq!(log.final_totals.bundles, 2);
        assert_eq!(log.final_totals.fees, 2 * OUR_FEE);
        assert_eq!(log.pending_totals().fees, OUR_FEE);

        // The root at 20 itself: final; counted once.
        update(&mut log, &chain[10..], &chain[..10]);
        update(&mut log, &chain[10..], &chain[..10]);
        assert_eq!(log.final_totals.blocks, 3);
        assert_eq!(log.pending_totals(), IncludedWorkTotals::default());
    }

    #[test]
    fn a_pending_entry_the_node_lost_sight_of_is_unknown() {
        let chains = Chains::new();
        let ours = key(1);
        let chain = chains.chain(None, 10, 40, 0, &ours, &[12]);
        let mut log = watching(&ours);
        update(&mut log, &chain[..5], &[]);
        // A restart far behind: a new frontier, no retired roots.
        update(&mut log, &chain[20..], &[]);
        assert_eq!(statuses(&log), vec![(12, IncludedWorkStatus::Unknown)]);
        assert_eq!(log.final_totals, IncludedWorkTotals::default());
        assert_eq!(log.pending_totals(), IncludedWorkTotals::default());
    }

    #[test]
    fn zkapp_bundles_are_counted_when_known() {
        let chains = Chains::new();
        let ours = key(1);
        let works = works();
        let job = |i: usize| SnarkJobId::from(&works[i % works.len()].proofs);
        let block = chains.block(
            None,
            10,
            0,
            &[
                (ours.clone(), OUR_FEE, 0),
                (ours.clone(), OUR_FEE, 1),
                (ours.clone(), OUR_FEE, 2),
            ],
        );
        let mut log = watching(&ours);
        // Job 0 our worker offered as a zkApp bundle, job 1 the pool knows.
        log.note_zkapp_job(&job(0));
        let pool_job = job(1);
        log.on_best_chain(std::slice::from_ref(&block), [], |id| *id == pool_job);
        assert_eq!(log.entries[0].bundles, 3);
        assert_eq!(log.entries[0].zkapp_bundles, 2);
    }

    #[test]
    fn the_log_is_bounded_and_keeps_its_totals() {
        let chains = Chains::new();
        let ours = key(1);
        let count = MAX_LOG_BLOCKS as u32 + 20;
        let all = (1..=count).collect::<Vec<_>>();
        let chain = chains.chain(None, 1, count, 0, &ours, &all);
        let mut log = watching(&ours);
        let (retired, frontier) = chain.split_at(chain.len() - 291);
        update(&mut log, frontier, retired);
        // Only the root and its descendants are in a best chain: 291 blocks.
        assert_eq!(log.entries.len(), 291);
        update(&mut log, &chain, &[]);
        assert_eq!(log.entries.len(), MAX_LOG_BLOCKS);
        assert_eq!(log.entries.front().unwrap().height, 21);
        assert_eq!(log.entries.back().unwrap().height, count);
        // The root's work counted as final, and so it stays.
        assert_eq!(log.final_totals.blocks, 2);

        let mut many = IncludedWorkLog::default();
        for n in 0..(MAX_WATCHED_KEYS as u32 + 3) {
            many.watch(&key(n).into(), 1);
        }
        assert_eq!(many.keys.len(), MAX_WATCHED_KEYS);
        assert_eq!(many.keys.last().unwrap(), &key(MAX_WATCHED_KEYS as u32 + 2));
        for i in 0..(MAX_ZKAPP_JOBS + 5) {
            let mut job = SnarkJobId::from(&works()[0].proofs);
            job.source.first_pass_ledger =
                v2::LedgerHash::from_fp(mina_curves::pasta::Fp::from(i as u64));
            many.note_zkapp_job(&job);
        }
        assert_eq!(many.zkapp_jobs.len(), MAX_ZKAPP_JOBS);
    }

    #[test]
    fn the_log_reloads_from_its_json() {
        let chains = Chains::new();
        let ours = key(1);
        let chain = chains.chain(None, 10, 20, 0, &ours, &[11, 15]);
        let mut log = watching(&ours);
        update(&mut log, &chain[2..], &chain[..2]);
        log.note_zkapp_job(&SnarkJobId::from(&works()[0].proofs));
        let json = serde_json::to_vec(&log).unwrap();
        let reloaded: IncludedWorkLog = serde_json::from_slice(&json).unwrap();
        assert_eq!(reloaded, log);
        assert_eq!(statuses(&reloaded), statuses(&log));
    }

    fn offer(prover: &v2::NonZeroCurvePoint, fee: u64, index: usize) -> Snark {
        let works = works();
        Snark {
            snarker: prover.clone(),
            fee: v2::CurrencyFeeStableV1(fee.into()),
            proofs: works[index % works.len()].proofs.clone().into(),
        }
    }

    #[test]
    fn the_market_lists_the_newest_purchases_and_the_pool_by_prover() {
        let chains = Chains::new();
        let ours = key(1);
        let mut chain = chains.chain(None, 1, 8, 0, &ours, &[3, 7]);
        // Two provers in one block, then a block that buys nothing.
        let mixed = chains.block(
            chain.last(),
            9,
            0,
            &[
                (ours.clone(), OUR_FEE, 0),
                (key(2), 10_000_000, 1),
                (key(2), 12_000_000, 2),
            ],
        );
        chain.push(mixed);
        let empty = chains.block(chain.last(), 10, 0, &[]);
        chain.push(empty);
        let pool = [
            offer(&key(2), 10_000_000, 0),
            offer(&ours, 5_000_000, 1),
            offer(&ours, OUR_FEE, 2),
        ];

        let market = SnarkMarket::new(&chain, &pool, std::slice::from_ref(&ours));
        assert_eq!(market.blocks, 10);
        assert_eq!(market.buying_blocks, 9);
        assert_eq!(market.bundles, 11);
        assert_eq!(market.purchases.len(), MARKET_PURCHASES);
        let newest = &market.purchases[0];
        assert_eq!((newest.height, newest.bundles, newest.provers), (9, 3, 2));
        assert_eq!((newest.min_fee, newest.max_fee), (OUR_FEE, 12_000_000));
        assert_eq!(newest.state_hash, *chain[8].hash());
        assert_eq!(market.purchases[1].height, 8);
        assert_eq!(market.purchases[2].min_fee, OUR_FEE);
        assert_eq!(market.offers.len(), 2);
        assert_eq!(market.offers[0].prover, ours);
        assert_eq!(
            (
                market.offers[0].bundles,
                market.offers[0].min_fee,
                market.offers[0].max_fee
            ),
            (2, OUR_FEE, 5_000_000)
        );
        assert_eq!(market.offers[1].prover, key(2));
        assert_eq!(
            (market.other_provers, market.highest_other_fee),
            (1, 10_000_000)
        );
    }

    #[test]
    fn the_offers_are_bounded_and_keep_our_key() {
        // 1,000 offers from 500 other provers, two each, and ours, the dearest.
        let proofs = mina_core::snark::SnarkProofs::new(works()[0].proofs.clone());
        let snark = |prover: &v2::NonZeroCurvePoint, fee: u64| Snark {
            snarker: prover.clone(),
            fee: v2::CurrencyFeeStableV1(fee.into()),
            proofs: proofs.clone(),
        };
        let ours = key(1_000);
        let provers = (0..500).map(key).collect::<Vec<_>>();
        let low = |i: usize| 1_000_000 * (i as u64 + 2);
        let mut pool = Vec::new();
        // Interleaved, so a prover's two bundles are far apart in the pool.
        for round in 0..2 {
            for (i, prover) in provers.iter().enumerate().rev() {
                pool.push(snark(prover, low(i) * (round + 1)));
            }
        }
        pool.push(snark(&ours, 2_000_000_000));
        assert_eq!(pool.len(), 1_001);

        let market = SnarkMarket::new(&[], &pool, std::slice::from_ref(&ours));
        assert_eq!(market.offers.len(), MARKET_OFFERS + 1);
        assert_eq!(market.other_provers, 500);
        assert_eq!(market.highest_other_fee, low(499) * 2);
        // The cheapest others in order, each with both its bundles.
        for (i, offer) in market.offers.iter().take(MARKET_OFFERS).enumerate() {
            assert_eq!(offer.prover, provers[i]);
            assert_eq!(
                (offer.bundles, offer.min_fee, offer.max_fee),
                (2, low(i), low(i) * 2)
            );
        }
        // Ours, though no cheaper than any, is listed: last, by fee.
        let last = market.offers.last().unwrap();
        assert_eq!((&last.prover, last.bundles), (&ours, 1));
        // Without our key, only the cap.
        let theirs = SnarkMarket::new(&[], &pool, &[]);
        assert_eq!(theirs.offers.len(), MARKET_OFFERS);
        assert_eq!(theirs.other_provers, 501);
        assert_eq!(theirs.highest_other_fee, 2_000_000_000);
    }

    #[test]
    fn the_purchases_look_back_a_fixed_window() {
        let chains = Chains::new();
        let ours = key(1);
        let length = MARKET_BLOCKS as u32 + 5;
        let chain = chains.chain(None, 1, length, 0, &ours, &[]);
        let market = SnarkMarket::new(&chain, std::iter::empty(), &[]);
        assert_eq!(market.blocks, MARKET_BLOCKS as u32);
        assert_eq!(market.buying_blocks, MARKET_BLOCKS as u32);
        assert_eq!(market.bundles, MARKET_BLOCKS as u64);
        assert_eq!(market.purchases[0].height, length);
        assert_eq!(market.purchases.len(), MARKET_PURCHASES);
    }

    #[test]
    fn the_view_adds_the_tip_and_the_market() {
        let chains = Chains::new();
        let ours = key(1);
        let chain = chains.chain(None, 10, 20, 0, &ours, &[11, 15]);
        let mut log = watching(&ours);
        update(&mut log, &chain[2..], &chain[..2]);
        let view = log.view();
        assert_eq!((view.best_tip_height, view.market.as_ref()), (None, None));
        let view = view.with_market(&chain[2..], &[offer(&ours, OUR_FEE, 0)]);
        assert_eq!(view.best_tip_height, Some(20));
        let market = view.market.unwrap();
        assert_eq!((market.blocks, market.buying_blocks), (9, 9));
        assert_eq!(market.offers.len(), 1);
        // An empty chain: no tip, and nothing bought.
        let empty = log.view().with_market(&[], std::iter::empty());
        assert_eq!(empty.best_tip_height, None);
        assert_eq!(empty.market, Some(SnarkMarket::default()));
    }

    #[test]
    fn an_older_answer_without_the_market_still_reads() {
        let chains = Chains::new();
        let ours = key(1);
        let chain = chains.chain(None, 10, 20, 0, &ours, &[11, 15]);
        let mut log = watching(&ours);
        update(&mut log, &chain[2..], &chain[..2]);
        let mut json = serde_json::to_value(log.view()).unwrap();
        let object = json.as_object_mut().unwrap();
        object.remove("best_tip_height");
        object.remove("market");
        let read: RpcIncludedWork = serde_json::from_value(json).unwrap();
        assert_eq!((read.best_tip_height, read.market), (None, None));
        assert_eq!(read.entries, log.view().entries);
        assert!(!read.entries.is_empty());
    }
}
