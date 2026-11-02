//! Bounds on how deeply recursive wire values may nest.
//!
//! The recursive fields read through [`read_nested_call`] or
//! [`read_nested_tree`]: a value nested deeper than the bound is invalid
//! input, refused while it is read.
//!
//! - A zkApp command's account updates nest at most as deep as there are
//!   updates, and OCaml refuses a command with more than 16 segments of one
//!   or two updates each (`Zkapp_command.valid_size`), in the pool and in
//!   gossiped blocks: at most 32. [`MAX_CALL_NESTING`] is 40. Each level costs
//!   tens of KiB of stack to convert and check, so the bound also keeps a
//!   command within what the threads that handle commands have room for.
//! - A sparse ledger has one level per level of the ledger, 35, and a
//!   pending coinbase tree one per level of its depth, 5.
//!   [`MAX_TREE_NESTING`] is 64.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/mina_base/zkapp_command.ml#L1340-L1395>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transition_handler/block_sink.ml#L193-L221>

use std::cell::Cell;

use binprot::BinProtRead;

/// The deepest a zkApp command's account updates may nest.
pub const MAX_CALL_NESTING: usize = 40;

/// The deepest a sparse ledger or pending coinbase tree may nest.
pub const MAX_TREE_NESTING: usize = 64;

thread_local! {
    /// Recursive fields being read on this thread. The two kinds of value
    /// are never nested in each other, so they share the count.
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

#[derive(Debug, thiserror::Error)]
#[error("value nests deeper than {0} levels")]
pub struct NestedTooDeep(pub usize);

/// Reads a zkApp command's nested calls, one level deeper than its parent.
pub fn read_nested_call<T, R>(r: &mut R) -> Result<T, binprot::Error>
where
    T: BinProtRead,
    R: std::io::Read + ?Sized,
{
    read_nested(r, MAX_CALL_NESTING)
}

/// Reads a subtree of a sparse ledger or pending coinbase tree.
pub fn read_nested_tree<T, R>(r: &mut R) -> Result<T, binprot::Error>
where
    T: BinProtRead,
    R: std::io::Read + ?Sized,
{
    read_nested(r, MAX_TREE_NESTING)
}

fn read_nested<T, R>(r: &mut R, max: usize) -> Result<T, binprot::Error>
where
    T: BinProtRead,
    R: std::io::Read + ?Sized,
{
    struct Level;
    impl Drop for Level {
        fn drop(&mut self) {
            DEPTH.with(|depth| depth.set(depth.get() - 1));
        }
    }

    let depth = DEPTH.with(|depth| {
        depth.set(depth.get() + 1);
        depth.get()
    });
    let _level = Level;
    if depth > max {
        return Err(binprot::Error::CustomError(Box::new(NestedTooDeep(max))));
    }
    T::binprot_read(r)
}

#[cfg(test)]
mod tests {
    use binprot::{BinProtRead, BinProtWrite};

    use super::*;
    use crate::v2::{
        MinaBasePendingCoinbaseMerkleTreeVersionedStableV2Tree as Tree, PendingCoinbaseHash,
    };

    fn hash() -> PendingCoinbaseHash {
        PendingCoinbaseHash::from_fp(mina_curves::pasta::Fp::from(0u64))
    }

    /// A tree that is a path of `depth` nodes, written by hand: a node is its
    /// tag, its hash and its two children.
    fn path(depth: usize) -> Vec<u8> {
        let mut leaf = Vec::new();
        Tree::Hash(hash()).binprot_write(&mut leaf).unwrap();
        let mut bytes = Vec::new();
        for _ in 0..depth {
            bytes.push(2);
            hash().binprot_write(&mut bytes).unwrap();
            bytes.extend_from_slice(&leaf);
        }
        bytes.extend_from_slice(&leaf);
        bytes
    }

    #[test]
    fn a_tree_within_the_bound_reads() {
        let bytes = path(MAX_TREE_NESTING);
        let tree = Tree::binprot_read(&mut &bytes[..]).unwrap();
        let mut written = Vec::new();
        tree.binprot_write(&mut written).unwrap();
        assert_eq!(written, bytes);
    }

    /// Past the bound, at any depth, reading fails instead of recursing; the
    /// depth is back to zero after, so the next read is unaffected.
    #[test]
    fn a_tree_nested_past_the_bound_is_refused() {
        for depth in [MAX_TREE_NESTING + 1, 100_000] {
            let err = Tree::binprot_read(&mut &path(depth)[..]).unwrap_err();
            assert!(
                matches!(&err, binprot::Error::CustomError(e) if e.is::<NestedTooDeep>()),
                "{depth}: {err:?}"
            );
            assert_eq!(DEPTH.with(Cell::get), 0);
        }
        assert!(Tree::binprot_read(&mut &path(3)[..]).is_ok());
    }
}
