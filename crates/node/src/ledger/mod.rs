pub mod read;
pub mod write;

mod ledger_config;
pub use ledger_config::*;
use mina_p2p_messages::bigint::InvalidBigInt;

mod ledger_event;
pub use ledger_event::*;

mod ledger_actions;
pub use ledger_actions::*;

mod ledger_state;
pub use ledger_state::*;

mod ledger_reducer;

mod delegate_index;
mod epoch_delegatees;

mod ledger_service;
pub use ledger_service::*;

pub mod ledger_manager;

pub use ledger::{AccountIndex as LedgerAccountIndex, Address as LedgerAddress};
pub use ledger_manager::LedgerManager;

use ledger::TreeVersion;
use mina_p2p_messages::v2;

// FIXME(tizoc): both networks use the same value, but this will break if that changes
pub const LEDGER_DEPTH: usize =
    crate::core::network::mainnet::CONSTRAINT_CONSTANTS.ledger_depth as usize;

lazy_static::lazy_static! {
    /// Array size needs to be changed when the tree's depth change
    static ref LEDGER_HASH_EMPTIES: [v2::LedgerHash; LEDGER_DEPTH + 1] = {
        use ledger::TreeVersion;

        std::array::from_fn(|i| {
            let hash = ledger::V2::empty_hash_at_height(LEDGER_DEPTH.saturating_sub(i));
            v2::MinaBaseLedgerHash0StableV1(hash.into()).into()
        })
    };
}

pub fn ledger_empty_hash_at_depth(depth: usize) -> v2::LedgerHash {
    LEDGER_HASH_EMPTIES.get(depth).unwrap().clone()
}

/// Given the hash of the subtree containing all accounts of height `subtree_height`
/// compute the hash of a tree of size `LEDGER_DEPTH` if all other nodes were
/// empty.
pub fn complete_height_tree_with_empties(
    content_hash: &v2::LedgerHash,
    subtree_height: usize,
) -> Result<v2::LedgerHash, InvalidBigInt> {
    assert!(LEDGER_DEPTH >= subtree_height);
    let content_hash = content_hash.0.to_field()?;

    let computed_hash = (subtree_height..LEDGER_DEPTH).fold(content_hash, |prev_hash, height| {
        let depth = LEDGER_DEPTH.saturating_sub(height);
        let empty_right = ledger_empty_hash_at_depth(depth).0.to_field().unwrap(); // We know empties are valid
        ledger::V2::hash_node(height, prev_hash, empty_right)
    });

    Ok(v2::LedgerHash::from_fp(computed_hash))
}

/// Returns the minimum tree height required for storing `num_accounts` accounts:
/// `Int.ceil_log2 num_accounts`, so 0 for a single account, whose subtree is
/// its leaf. It sizes both the `Num_accounts` answer and its check.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/syncable_ledger/syncable_ledger.ml#L281-L292>
pub fn tree_height_for_num_accounts(num_accounts: u64) -> usize {
    // A peer's answer can claim any count; above 2^63 the height is 64.
    num_accounts
        .checked_next_power_of_two()
        .map_or(64, |n| n.ilog2() as usize)
}

/// Given the hash of the subtree containing `num_accounts` accounts
/// compute the hash of a tree of size `LEDGER_DEPTH` if all other nodes were
/// empty.
///
/// NOTE: For out of range sizes, en empty tree hash is returned.
pub fn complete_num_accounts_tree_with_empties(
    contents_hash: &v2::LedgerHash,
    num_accounts: u64,
) -> Result<v2::LedgerHash, InvalidBigInt> {
    // Note, we assume there is always at least one account
    if num_accounts == 0 {
        return Ok(ledger_empty_hash_at_depth(0));
    }

    let subtree_height = tree_height_for_num_accounts(num_accounts);

    // This would not be a valid number of accounts because it doesn't fit the tree
    if subtree_height > LEDGER_DEPTH {
        Ok(ledger_empty_hash_at_depth(0))
    } else {
        complete_height_tree_with_empties(contents_hash, subtree_height)
    }
}

pub fn hash_node_at_depth(
    depth: usize,
    left: mina_curves::pasta::Fp,
    right: mina_curves::pasta::Fp,
) -> mina_curves::pasta::Fp {
    let height = LEDGER_DEPTH.saturating_sub(depth).saturating_sub(1);
    ledger::V2::hash_node(height, left, right)
}

/// Hash of the node at `depth` whose descendants some levels below are
/// `hashes`, left to right. Their number must be a power of two, at least 2.
pub fn hash_subtree_at_depth(
    depth: usize,
    mut hashes: Vec<mina_curves::pasta::Fp>,
) -> Option<mina_curves::pasta::Fp> {
    if hashes.len() < 2 || !hashes.len().is_power_of_two() {
        return None;
    }
    let levels = hashes.len().trailing_zeros() as usize;
    for level in (0..levels).rev() {
        hashes = hashes
            .chunks(2)
            .map(|pair| hash_node_at_depth(depth + level, pair[0], pair[1]))
            .collect();
    }
    hashes.pop()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_subtree_at_depth() {
        let empty = |depth: usize| ledger_empty_hash_at_depth(depth).to_field().unwrap();
        for (depth, levels) in [(0, 1), (16, 6), (28, 1), (27, 8)] {
            let hashes = vec![empty(depth + levels); 1 << levels];
            assert_eq!(hash_subtree_at_depth(depth, hashes), Some(empty(depth)));
        }
        assert_eq!(hash_subtree_at_depth(3, vec![empty(4)]), None);
        assert_eq!(hash_subtree_at_depth(3, vec![empty(5); 3]), None);
    }

    // Both hashes are OCaml 4.0.2's `mina ledger hash` of the same 1000
    // accounts: with the dev profile, whose ledgers have depth 10, that is the
    // subtree of height 10 holding every account; with the mainnet profile it
    // is the root at depth 35. mina-tree's `test_ledger_root_1000_accounts`
    // builds those accounts.
    const CONTENTS_HASH_HEIGHT_10: &str = "jxkpJJb18Gd4x4cphdnUG3KMNU1Ykm7UMjMKgVwfRBzvp3FHXsW";
    const ROOT_HASH_1000_ACCOUNTS: &str = "jxyeaYyGz8REJMbG9QXuooAEndFVF4yaSNbD6DPC2bmkAbTW8cv";

    #[test]
    fn test_complete_with_empties() {
        let subtree_height = 10;
        let expected_hash: v2::LedgerHash = ROOT_HASH_1000_ACCOUNTS.parse().unwrap();
        let contents_hash = CONTENTS_HASH_HEIGHT_10.parse().unwrap();

        let actual_hash =
            complete_height_tree_with_empties(&contents_hash, subtree_height).unwrap();

        assert_eq!(expected_hash, actual_hash);
    }

    #[test]
    fn test_complete_with_empties_with_num_accounts() {
        let num_accounts = 1000;
        assert_eq!(tree_height_for_num_accounts(num_accounts), 10);
        let expected_hash: v2::LedgerHash = ROOT_HASH_1000_ACCOUNTS.parse().unwrap();
        let contents_hash = CONTENTS_HASH_HEIGHT_10.parse().unwrap();

        let actual_hash =
            complete_num_accounts_tree_with_empties(&contents_hash, num_accounts).unwrap();

        assert_eq!(expected_hash, actual_hash);
    }

    /// OCaml sizes the `Num_accounts` subtree with `Int.ceil_log2`.
    #[test]
    fn test_tree_height_for_num_accounts() {
        for (num_accounts, height) in [
            (1, 0),
            (2, 1),
            (3, 2),
            (4, 2),
            (5, 3),
            (1000, 10),
            (1024, 10),
            (1025, 11),
            (1 << 35, 35),
            (u64::MAX, 64),
        ] {
            assert_eq!(
                tree_height_for_num_accounts(num_accounts),
                height,
                "{num_accounts} accounts"
            );
        }
    }

    /// OCaml 4.0.2's `mina ledger hash` of an empty ledger, mainnet profile.
    #[test]
    fn test_ledger_empty_hash_at_depth() {
        assert_eq!(
            ledger_empty_hash_at_depth(0).to_string(),
            "jwkaDMeSMeL94hwJg6EYtSRD6gxB1BHpXnGKnZ1VW6jthKzgcef"
        );
    }
}
