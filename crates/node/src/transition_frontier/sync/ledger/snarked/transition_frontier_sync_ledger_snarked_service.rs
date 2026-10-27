use mina_p2p_messages::v2::{LedgerHash, MinaBaseAccountBinableArgStableV2};

use crate::ledger::LedgerAddress;

pub trait TransitionFrontierSyncLedgerSnarkedService: redux::Service {
    /// Whether the complete ledger is already committed under this hash.
    fn contains_snarked_ledger(&self, hash: &LedgerHash) -> bool;

    /// For the given ledger, compute the merkle root hash, forcing
    /// all pending hashes to be computed too.
    fn compute_snarked_ledger_hashes(&self, snarked_ledger_hash: &LedgerHash)
        -> Result<(), String>;

    /// Creates a new copy of the ledger stored under the first found `origin` hash
    /// and stores it under the `target` hash. If `overwrite` is false,
    /// only copy the ledger if the target doesn't exist already.
    fn copy_snarked_ledger_contents_for_sync(
        &self,
        origin: Vec<LedgerHash>,
        target: LedgerHash,
        overwrite: bool,
    ) -> Result<bool, String>;

    /// For the given ledger, get the hashes `depth` levels below the `parent`
    /// address, left to right.
    fn child_hashes_get(
        &self,
        snarked_ledger_hash: LedgerHash,
        parent: &LedgerAddress,
        depth: usize,
    ) -> Result<Vec<LedgerHash>, String>;

    /// For the given ledger, sets all accounts in `accounts` under
    /// the subtree starting at the `parent` address, if they hash to
    /// `expected_hash`. The result is the hash they give that subtree.
    fn accounts_set(
        &self,
        snarked_ledger_hash: LedgerHash,
        parent: &LedgerAddress,
        expected_hash: &LedgerHash,
        accounts: Vec<MinaBaseAccountBinableArgStableV2>,
    ) -> Result<LedgerHash, String>;
}
