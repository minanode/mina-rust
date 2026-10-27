//! Account Merkle paths in Mina's ledger tree.
//!
//! Leaves are account hashes. A node at height `h + 1` is
//! `poseidon(MinaMklTree{h}, [left, right])`, where `h` is the height of its
//! children (leaves are at height 0), and empty subtrees hash to the empty
//! account's hash merged with itself.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/ledger_hash.ml#L36-L39>
//! and <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/merkle_ledger/merkle_path.ml#L32-L42>

use ledger::Account;
use mina_curves::pasta::Fp;
use poseidon::hash::{hash_with_kimchi, params::get_merkle_param_for_height};

pub fn merge(height: usize, left: Fp, right: Fp) -> Fp {
    hash_with_kimchi(get_merkle_param_for_height(height), &[left, right])
}

/// Empty-subtree hashes by height, `0..=depth`.
pub fn empty_hashes(depth: usize) -> Vec<Fp> {
    let mut hashes = vec![Account::empty().hash()];
    for height in 0..depth {
        let below = hashes[height];
        hashes.push(merge(height, below, below));
    }
    hashes
}

/// Rows of a complete subtree, bottom first: `rows[0]` is `row` at `height`, the
/// last row is the subtree root. `row.len()` must be a power of two.
pub fn subtree_rows(row: Vec<Fp>, height: usize) -> Vec<Vec<Fp>> {
    assert!(row.len().is_power_of_two());
    let mut rows = vec![row];
    let mut h = height;
    while rows.last().unwrap().len() > 1 {
        let next = rows
            .last()
            .unwrap()
            .chunks(2)
            .map(|pair| merge(h, pair[0], pair[1]))
            .collect();
        rows.push(next);
        h += 1;
    }
    rows
}

/// The root implied by a leaf and its siblings, listed from the leaf up
/// (`siblings[h]` is the sibling at height `h`), as OCaml's
/// `Merkle_path.implied_root`. Bit `h` of `index` says whether the path node
/// at height `h` is a right child.
pub fn implied_root(leaf: Fp, index: u64, siblings: &[Fp]) -> Fp {
    siblings
        .iter()
        .enumerate()
        .fold(leaf, |node, (height, sibling)| {
            if (index >> height) & 1 == 0 {
                merge(height, node, *sibling)
            } else {
                merge(height, *sibling, node)
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implied_root_matches_subtree_rows() {
        let leaves: Vec<Fp> = (0..8u64).map(Fp::from).collect();
        let rows = subtree_rows(leaves.clone(), 0);
        let root = rows.last().unwrap()[0];
        for index in 0..8u64 {
            let siblings: Vec<Fp> = (0..3)
                .map(|h| rows[h][((index >> h) ^ 1) as usize])
                .collect();
            assert_eq!(implied_root(leaves[index as usize], index, &siblings), root);
        }
    }

    #[test]
    fn empty_hashes_are_consistent() {
        let e = empty_hashes(4);
        let rows = subtree_rows(vec![e[0]; 16], 0);
        assert_eq!(rows.last().unwrap()[0], e[4]);
    }
}
