//! Staged ledger dumps: what a node needs to rebuild the staged ledger of a
//! block without replaying the chain before it. They let a replay start
//! anywhere on the chain.
//!
//! The format is the one a node writes with `MINA_DUMP_RECONSTRUCT` set
//! (`<data dir>/debug/failed_reconstruct_ctx.binprot`, from
//! `dump_reconstruct_to_file` in `crates/node/src/ledger/ledger_service.rs`):
//! the parts of OCaml's `get_staged_ledger_aux_and_pending_coinbases_at_hash`
//! answer, and the snarked ledger they apply to. `mina-block-replay` writes
//! the same format with `--write-dump`, so either kind can start a replay.
//!
//! A dump doesn't say which block it belongs to: the replay finds the block
//! whose staged ledger hash the rebuilt staged ledger has.

use std::{collections::HashMap, path::Path};

use anyhow::Context;
use ledger::{
    scan_state::{
        pending_coinbase::PendingCoinbase, scan_state::ScanState,
        transaction_logic::local_state::LocalState,
    },
    staged_ledger::staged_ledger::StagedLedger,
    verifier::Verifier,
    Account, AccountIndex, Address, BaseLedger, Mask,
};
use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    binprot::{
        self,
        macros::{BinProtRead, BinProtWrite},
        BinProtRead, BinProtWrite,
    },
    list::List,
    v2,
};
use rayon::prelude::*;

/// Field for field the `ReconstructContext` the node writes.
#[derive(BinProtRead, BinProtWrite)]
pub struct StagedLedgerDump {
    /// The snarked ledger's accounts, by index.
    pub accounts: Vec<v2::MinaBaseAccountBinableArgStableV2>,
    pub scan_state: v2::TransactionSnarkScanStateStableV2,
    pub pending_coinbase: v2::MinaBasePendingCoinbaseStableV2,
    /// The staged ledger's ledger hash, once the scan state's transactions are
    /// applied to the snarked ledger: the `ledger_hash` of the block's
    /// `staged_ledger_hash`.
    pub staged_ledger_hash: v2::LedgerHash,
    /// The protocol states of the blocks whose transactions are in the scan
    /// state, which applying them needs.
    pub states: List<v2::MinaStateProtocolStateValueStableV2>,
}

/// The node's historical name for the identical dump wire format.
pub type ReconstructContext = StagedLedgerDump;

impl StagedLedgerDump {
    /// Reconstruct using the protocol states carried by this dump.
    pub fn reconstruct(&self, snarked: &Mask) -> anyhow::Result<StagedLedger> {
        self.staged_ledger(snarked, &self.states_by_hash()?)
    }

    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("{}", path.display()))?;
        Self::binprot_read(&mut bytes.as_slice())
            .with_context(|| format!("{}: not a staged ledger dump", path.display()))
    }

    /// Writes the dump next to `path` first, so that an interrupted write
    /// leaves no partial dump behind.
    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        let partial = path.with_extension("partial");
        let mut bytes = Vec::new();
        self.binprot_write(&mut bytes)?;
        std::fs::write(&partial, bytes).with_context(|| format!("{}", partial.display()))?;
        std::fs::rename(&partial, path).with_context(|| format!("{}", path.display()))?;
        Ok(())
    }

    /// Takes the dump of a staged ledger: `snarked` is its snarked ledger,
    /// and `states` holds at least the protocol states its scan state needs.
    pub fn of_staged_ledger(
        staged_ledger: &StagedLedger,
        snarked: &Mask,
        states: &HashMap<Fp, v2::MinaStateProtocolStateValueStableV2>,
    ) -> anyhow::Result<Self> {
        let scan_state = staged_ledger.scan_state();
        let mut needed = scan_state
            .required_state_hashes()
            .into_iter()
            .map(|hash| {
                states
                    .get(&hash)
                    .cloned()
                    .with_context(|| format!("no protocol state {}", v2::StateHash::from_fp(hash)))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        // Oldest first, so that the same staged ledger always gives the same
        // bytes.
        needed.sort_by_key(|state| state.body.consensus_state.blockchain_length.as_u32());
        Ok(Self {
            accounts: snarked
                .to_list()
                .iter()
                .map(v2::MinaBaseAccountBinableArgStableV2::from)
                .collect(),
            scan_state: with_leaner_trees(scan_state.into()),
            pending_coinbase: sorted_indexes(staged_ledger.pending_coinbase_collection().into()),
            staged_ledger_hash: v2::LedgerHash::from_fp(staged_ledger.ledger().merkle_root()),
            states: needed.into_iter().collect(),
        })
    }

    /// The protocol states the dump holds, by state hash.
    pub fn states_by_hash(
        &self,
    ) -> anyhow::Result<HashMap<Fp, v2::MinaStateProtocolStateValueStableV2>> {
        self.states
            .iter()
            .map(|state| Ok((state.try_hash()?.to_field::<Fp>()?, state.clone())))
            .collect()
    }

    /// Builds the snarked ledger. The accounts are hashed in parallel, which
    /// is most of the work: about 40 s for 290,000 accounts on one thread.
    pub fn snarked_ledger(&self) -> anyhow::Result<Mask> {
        let depth = mina_core::constants::constraint_constants().ledger_depth as usize;
        let accounts = self
            .accounts
            .par_iter()
            .map(Account::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        let hashes: Vec<Fp> = accounts.par_iter().map(Account::hash).collect();
        let mut ledger = Mask::create_with_token_owners(depth);
        for account in accounts {
            ledger
                .get_or_create_account(account.id(), account)
                .map_err(|e| anyhow::anyhow!("adding an account: {e:?}"))?;
        }
        // After the accounts, which would clear them.
        for (index, hash) in hashes.into_iter().enumerate() {
            let leaf = Address::from_index(AccountIndex(index as u64), depth);
            ledger.set_cached_hash_unchecked(&leaf, hash);
        }
        Ok(ledger)
    }

    /// Rebuilds the staged ledger on a child of `snarked`, applying the scan
    /// state's transactions as a node does after syncing a root ledger, and
    /// checks it against the dump's staged ledger hash.
    pub fn staged_ledger(
        &self,
        snarked: &Mask,
        states: &HashMap<Fp, v2::MinaStateProtocolStateValueStableV2>,
    ) -> anyhow::Result<StagedLedger> {
        let scan_state: ScanState = (&self.scan_state).try_into()?;
        let pending_coinbase: PendingCoinbase = (&self.pending_coinbase).try_into()?;
        if let Some(missing) = scan_state
            .required_state_hashes()
            .into_iter()
            .find(|hash| !states.contains_key(hash))
        {
            anyhow::bail!(
                "the dump lacks the protocol state {}, which its scan state needs",
                v2::StateHash::from_fp(missing)
            );
        }
        StagedLedger::of_scan_state_pending_coinbases_and_snarked_ledger(
            (),
            mina_core::constants::constraint_constants(),
            Verifier,
            scan_state,
            snarked.make_child(),
            LocalState::empty(),
            self.staged_ledger_hash.to_field::<Fp>()?,
            pending_coinbase,
            // Only the scan state's transactions' states are looked up, and
            // they were all found above.
            |hash| states[&hash].clone(),
        )
        .map_err(|e| anyhow::anyhow!("rebuilding the staged ledger: {e}"))
    }
}

/// The scan state as OCaml writes it: with the merge jobs that are done
/// emptied, which it doesn't need any more and which its hash leaves out. The
/// conversion keeps them: at 563941 the replay's scan state held 2,307 full
/// merge jobs where a node's dump held 381, and 119 MB where it held 74 MB.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/parallel_scan/parallel_scan.ml#L835-L863>
pub fn with_leaner_trees(
    mut scan_state: v2::TransactionSnarkScanStateStableV2,
) -> v2::TransactionSnarkScanStateStableV2 {
    use v2::{
        ParallelScanJobStatusStableV1 as Status,
        TransactionSnarkScanStateStableV2ScanStateTreesA as Tree,
        TransactionSnarkScanStateStableV2ScanStateTreesAMergeT1 as Merge,
    };

    fn leaner(mut tree: &mut Tree) {
        while let Tree::Node {
            value, sub_tree, ..
        } = tree
        {
            for (_, job) in value.iter_mut() {
                if matches!(job, Merge::Full(full) if matches!(full.status, Status::Done)) {
                    *job = Merge::Empty;
                }
            }
            tree = sub_tree;
        }
    }

    let (latest, earlier) = &mut scan_state.scan_state.trees;
    leaner(latest);
    earlier.iter_mut().for_each(leaner);
    scan_state
}

/// The pending coinbase with its tree's stack indexes in the order of their
/// positions. The conversion lists them in the order of a hash map, which
/// differs from one run to the next; OCaml's list holds the same pairs.
pub fn sorted_indexes(
    mut pending_coinbase: v2::MinaBasePendingCoinbaseStableV2,
) -> v2::MinaBasePendingCoinbaseStableV2 {
    let mut indexes: Vec<_> = pending_coinbase.tree.indexes.iter().cloned().collect();
    indexes.sort_by_key(|(_, index)| index.as_u64());
    pending_coinbase.tree.indexes = indexes.into_iter().collect();
    pending_coinbase
}
