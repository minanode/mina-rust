mod transition_frontier_sync_ledger_staged_state;
use mina_curves::pasta::Fp;
use mina_p2p_messages::bigint::InvalidBigInt;
pub use transition_frontier_sync_ledger_staged_state::*;

mod transition_frontier_sync_ledger_staged_actions;
pub use transition_frontier_sync_ledger_staged_actions::*;

mod transition_frontier_sync_ledger_staged_reducer;

mod transition_frontier_sync_ledger_staged_service;
pub use transition_frontier_sync_ledger_staged_service::*;

use std::sync::Arc;

use ledger::{
    scan_state::{
        pending_coinbase::{InvalidPendingCoinbase, PendingCoinbase},
        scan_state::ScanState,
    },
    staged_ledger::hash::StagedLedgerHash,
};
use mina_p2p_messages::{
    list::List,
    v2::{self, MinaBaseStagedLedgerHashStableV1},
};
use serde::{Deserialize, Serialize};

use crate::p2p::channels::rpc::StagedLedgerAuxAndPendingCoinbases;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum PeerStagedLedgerPartsFetchError {
    Timeout,
    Disconnected,
    DataUnavailable,
}

/// Parts RPCs allow 180 s by default, much longer than ledger queries.
/// Bound lost outcomes without racing an ordinary heavy parts transfer.
pub const STAGED_LEDGER_PARTS_STALE_AFTER: std::time::Duration =
    std::time::Duration::from_secs(5 * 60);

/// Staged ledger parts from a peer that hash to the target's staged ledger
/// hash, in the ledger's own types. The validation converts a peer's parts to
/// hash them and keeps what it converted; the staged ledger rebuild takes it,
/// so the parts are converted once, not again on the ledger thread, and the
/// wire form is dropped once checked instead of being kept until the rebuild
/// ends (R38). The scan state's jobs, behind `Arc`s, are then shared with the
/// rebuilt staged ledger, and their hashes are cached. Serialized as the wire
/// parts.
#[derive(Clone)]
pub struct StagedLedgerAuxAndPendingCoinbasesValid {
    pub scan_state: ScanState,
    pub staged_ledger_hash: v2::LedgerHash,
    /// Its Merkle root is computed, as the conversion to the wire type needs.
    pub pending_coinbase: PendingCoinbase,
    pub needed_blocks: List<v2::MinaStateProtocolStateValueStableV2>,
}

impl StagedLedgerAuxAndPendingCoinbasesValid {
    /// Converts a peer's parts, and hashes them: the staged ledger hash they
    /// make with their ledger hash.
    fn convert(
        parts: &StagedLedgerAuxAndPendingCoinbases,
    ) -> Result<(Self, MinaBaseStagedLedgerHashStableV1), String> {
        let scan_state: ScanState = (&parts.scan_state)
            .try_into()
            .map_err(|error: InvalidBigInt| format!("scan state: {error}"))?;
        let mut pending_coinbase: PendingCoinbase = (&parts.pending_coinbase)
            .try_into()
            .map_err(|error: InvalidPendingCoinbase| error.to_string())?;
        let ledger_hash: Fp = parts
            .staged_ledger_hash
            .to_field()
            .map_err(|error: InvalidBigInt| format!("staged ledger hash: {error}"))?;
        let hash = StagedLedgerHash::of_aux_ledger_and_coinbase_hash(
            scan_state.hash(),
            ledger_hash,
            &mut pending_coinbase,
        );
        let parts = Self {
            scan_state,
            staged_ledger_hash: parts.staged_ledger_hash.clone(),
            pending_coinbase,
            needed_blocks: parts.needed_blocks.clone(),
        };
        Ok((parts, (&hash).into()))
    }

    /// How many pieces a peer streams them in, as
    /// `calc_total_pieces_to_transfer` counts the wire parts: three, and one
    /// per scan state tree.
    /// Whether `needed_blocks` are exactly the protocol states the scan
    /// state's transactions need. The staged ledger hash doesn't cover them,
    /// so the hash check alone can't tell that some are missing. OCaml
    /// refuses a peer's parts unless there is one state for each, and no
    /// others.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_snark_scan_state/transaction_snark_scan_state.ml#L1506-L1533>
    pub fn has_required_protocol_states(&self) -> bool {
        let received = self
            .needed_blocks
            .iter()
            .map(|state| state.try_hash()?.to_field())
            .collect::<Result<Vec<Fp>, InvalidBigInt>>();
        received.is_ok_and(|received| {
            holds_exactly(&self.scan_state.required_state_hashes(), &received)
        })
    }

    pub fn total_pieces_to_transfer(&self) -> u64 {
        3 + self.scan_state.scan_state.trees.len() as u64
    }

    /// The parts in the wire types, as a peer sends them.
    pub fn to_wire(&self) -> StagedLedgerAuxAndPendingCoinbases {
        StagedLedgerAuxAndPendingCoinbases {
            scan_state: (&self.scan_state).into(),
            staged_ledger_hash: self.staged_ledger_hash.clone(),
            pending_coinbase: (&self.pending_coinbase).into(),
            needed_blocks: self.needed_blocks.clone(),
        }
    }
}

impl std::fmt::Debug for StagedLedgerAuxAndPendingCoinbasesValid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedLedgerAuxAndPendingCoinbasesValid")
            .field("staged_ledger_hash", &self.staged_ledger_hash)
            .field("needed_blocks", &self.needed_blocks.len())
            .finish_non_exhaustive()
    }
}

impl Serialize for StagedLedgerAuxAndPendingCoinbasesValid {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_wire().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StagedLedgerAuxAndPendingCoinbasesValid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let parts = StagedLedgerAuxAndPendingCoinbases::deserialize(deserializer)?;
        Self::convert(&parts)
            .map(|(parts, _)| parts)
            .map_err(serde::de::Error::custom)
    }
}

/// Whether `received` has one entry for each of `required` and no others:
/// as many as are required, and each of them.
fn holds_exactly(required: &std::collections::HashSet<Fp>, received: &[Fp]) -> bool {
    received.len() == required.len() && {
        let received: std::collections::BTreeSet<&Fp> = received.iter().collect();
        required.iter().all(|hash| received.contains(hash))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum StagedLedgerAuxAndPendingCoinbasesValidated {
    Valid(Arc<StagedLedgerAuxAndPendingCoinbasesValid>),
    Invalid(Arc<StagedLedgerAuxAndPendingCoinbases>),
}

impl StagedLedgerAuxAndPendingCoinbasesValidated {
    pub fn validate(
        parts: &Arc<StagedLedgerAuxAndPendingCoinbases>,
        expected_hash: &MinaBaseStagedLedgerHashStableV1,
    ) -> Self {
        match Self::check(parts, expected_hash) {
            Ok(valid) => Self::Valid(Arc::new(valid)),
            Err(_) => Self::Invalid(parts.clone()),
        }
    }

    /// The parts converted, if they are valid for `expected_hash`, or why
    /// they are not. The reason is in this node's words, with at most
    /// numbers from the parts, so that it can be logged: a refusal of an
    /// honest peer's parts must show in the logs.
    pub fn check(
        parts: &StagedLedgerAuxAndPendingCoinbases,
        expected_hash: &MinaBaseStagedLedgerHashStableV1,
    ) -> Result<StagedLedgerAuxAndPendingCoinbasesValid, String> {
        let (valid, hash) = StagedLedgerAuxAndPendingCoinbasesValid::convert(parts)?;
        if &hash != expected_hash {
            return Err("they make another staged ledger hash".into());
        }
        if !valid.has_required_protocol_states() {
            return Err("their protocol states are not the ones their scan state needs".into());
        }
        valid
            .scan_state
            .check_shape()
            .map_err(|error| format!("scan state: {error}"))?;
        Ok(valid)
    }

    pub fn is_valid(&self) -> bool {
        matches!(self, Self::Valid(_))
    }
}

#[cfg(test)]
mod tests {
    use ledger::{staged_ledger::staged_ledger::StagedLedger, Account, BaseLedger, Mask};
    use mina_core::constants::constraint_constants;
    use mina_p2p_messages::binprot::BinProtWrite;

    use super::*;
    use crate::p2p::channels::streaming_rpc::staged_ledger_parts::calc_total_pieces_to_transfer;

    fn bytes<T: BinProtWrite>(value: &T) -> Vec<u8> {
        let mut bytes = Vec::new();
        value.binprot_write(&mut bytes).unwrap();
        bytes
    }

    /// `parts` are the peer's `wire` parts: the same scan state, ledger hash
    /// and protocol states, and a pending coinbase that makes the same hash
    /// (its stack index map is a `HashMap`, so the order of its wire list
    /// differs from conversion to conversion).
    fn assert_same_parts(
        parts: &StagedLedgerAuxAndPendingCoinbasesValid,
        wire: &Arc<StagedLedgerAuxAndPendingCoinbases>,
        hash: &MinaBaseStagedLedgerHashStableV1,
    ) {
        let back = Arc::new(parts.to_wire());
        assert_eq!(bytes(&back.scan_state), bytes(&wire.scan_state));
        assert_eq!(back.staged_ledger_hash, wire.staged_ledger_hash);
        assert_eq!(bytes(&back.needed_blocks), bytes(&wire.needed_blocks));
        assert!(StagedLedgerAuxAndPendingCoinbasesValidated::validate(&back, hash).is_valid());
    }

    /// A staged ledger's parts as a peer sends them, and its hash.
    fn parts() -> (
        Arc<StagedLedgerAuxAndPendingCoinbases>,
        MinaBaseStagedLedgerHashStableV1,
    ) {
        let mut ledger = Mask::create(35);
        for _ in 0..4 {
            let account = Account::rand();
            ledger.get_or_create_account(account.id(), account).unwrap();
        }
        let mut staged_ledger =
            StagedLedger::create_exn(constraint_constants().clone(), ledger.clone()).unwrap();
        let hash = MinaBaseStagedLedgerHashStableV1::from(&staged_ledger.hash());
        staged_ledger.pending_coinbase_collection_merkle_root();
        let parts = StagedLedgerAuxAndPendingCoinbases {
            scan_state: staged_ledger.scan_state().into(),
            staged_ledger_hash: hash.non_snark.ledger_hash.clone(),
            pending_coinbase: staged_ledger.pending_coinbase_collection().into(),
            needed_blocks: List::new(),
        };
        (Arc::new(parts), hash)
    }

    /// A peer's parts whose pending coinbase keeps the expected root but
    /// misses a stack index (which the staged ledger hash doesn't cover) are
    /// invalid.
    #[test]
    fn parts_with_a_pending_coinbase_missing_an_index_are_invalid() {
        let (wire, hash) = parts();
        let mut tampered = (*wire).clone();
        let indexes = &mut tampered.pending_coinbase.tree.indexes;
        *indexes = indexes.iter().skip(1).cloned().collect();
        assert!(
            !StagedLedgerAuxAndPendingCoinbasesValidated::validate(&Arc::new(tampered), &hash)
                .is_valid()
        );
    }

    /// A peer's parts whose pending coinbase is of another depth, or names a
    /// stack out of range, are refused by the conversion itself, before their
    /// hash is compared.
    #[test]
    fn parts_with_a_pending_coinbase_of_another_depth_or_position_are_invalid() {
        fn deeper(pending_coinbase: &mut v2::MinaBasePendingCoinbaseStableV2) {
            let depth = pending_coinbase.tree.depth.as_u64();
            pending_coinbase.tree.depth = (depth + 1).into();
        }
        fn past_the_stacks(pending_coinbase: &mut v2::MinaBasePendingCoinbaseStableV2) {
            let stacks = 1u64 << constraint_constants().pending_coinbase_depth;
            pending_coinbase.new_pos = v2::MinaBasePendingCoinbaseStackIdStableV1(stacks.into());
        }
        let tampers: [(fn(&mut v2::MinaBasePendingCoinbaseStableV2), &str); 2] =
            [(deeper, "depth"), (past_the_stacks, "out of range")];
        let (wire, hash) = parts();
        for (tamper, expected) in tampers {
            let mut tampered = (*wire).clone();
            tamper(&mut tampered.pending_coinbase);
            let error = StagedLedgerAuxAndPendingCoinbasesValid::convert(&tampered)
                .err()
                .expect("refused");
            assert!(error.contains(expected), "{error}");
            assert!(!StagedLedgerAuxAndPendingCoinbasesValidated::validate(
                &Arc::new(tampered),
                &hash
            )
            .is_valid());
        }
    }

    /// Parts that make the expected hash are kept converted, and convert
    /// back to the parts the peer sent, also through serde; others are
    /// invalid.
    #[test]
    fn parts_are_checked_and_kept_converted() {
        let (wire, hash) = parts();
        let StagedLedgerAuxAndPendingCoinbasesValidated::Valid(valid) =
            StagedLedgerAuxAndPendingCoinbasesValidated::validate(&wire, &hash)
        else {
            panic!("parts making the expected hash are invalid");
        };
        assert_same_parts(&valid, &wire, &hash);
        assert_eq!(
            valid.total_pieces_to_transfer(),
            calc_total_pieces_to_transfer(&wire)
        );

        let json = serde_json::to_string(&*valid).unwrap();
        let back: StagedLedgerAuxAndPendingCoinbasesValid = serde_json::from_str(&json).unwrap();
        assert_same_parts(&back, &wire, &hash);

        let (_, other_hash) = parts();
        assert!(
            !StagedLedgerAuxAndPendingCoinbasesValidated::validate(&wire, &other_hash).is_valid()
        );
    }

    /// Exactly the required states, in any order, and nothing else: OCaml
    /// counts them, then looks each required one up.
    #[test]
    fn needed_states_must_be_exactly_the_required_ones() {
        let [a, b, c] = [1u64, 2, 3].map(Fp::from);
        let required = std::collections::HashSet::from([a, b]);
        assert!(holds_exactly(&required, &[b, a]));
        assert!(!holds_exactly(&required, &[a]), "one missing");
        assert!(!holds_exactly(&required, &[a, b, c]), "one extra");
        assert!(!holds_exactly(&required, &[a, a]), "one twice, one missing");
        assert!(!holds_exactly(&required, &[a, c]), "one replaced");
        assert!(holds_exactly(&Default::default(), &[]));
    }

    /// Parts that make the expected hash but bring a state their scan state
    /// doesn't need are invalid, as OCaml refuses them.
    #[test]
    fn parts_with_unneeded_states_are_invalid() {
        use mina_p2p_messages::rpc_kernel::PayloadBinprotReader;
        let (wire, hash) = parts();
        let bytes =
            include_bytes!("../../../../../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin");
        let block = mina_p2p_messages::rpc::GetBestTipV2::response_payload(&mut bytes.as_slice())
            .unwrap()
            .unwrap()
            .data;
        let mut extra = (*wire).clone();
        extra
            .needed_blocks
            .push_back(block.header.protocol_state.clone());
        assert!(
            !StagedLedgerAuxAndPendingCoinbasesValidated::validate(&Arc::new(extra), &hash)
                .is_valid()
        );
    }

    /// The parts a web node gets from two relays that meet after `first`
    /// trees: the first relay's bases and its first `first` trees, and the
    /// second relay's scan state base and the other trees, last first. Both
    /// relays hold `full`, and send each tree without its completed merge
    /// jobs.
    fn union_of_two_streams(
        full: &StagedLedgerAuxAndPendingCoinbases,
        first: usize,
    ) -> StagedLedgerAuxAndPendingCoinbases {
        use crate::p2p::channels::streaming_rpc::staged_ledger_parts::{
            from_first_part, from_last_part, staged_ledger_parts_complete,
            take_staged_ledger_parts, StagedLedgerPartsFromLastProgress,
            StagedLedgerPartsReceiveProgress,
        };
        let time = redux::Timestamp::ZERO;
        let trees = full.scan_state.scan_state.trees.1.len() + 1;
        let mut from_first = StagedLedgerPartsReceiveProgress::BasePending { time };
        for part in (0..3 + first).map(|sent| from_first_part(full, sent).unwrap()) {
            if !from_first.is_part_pending() {
                assert!(from_first.set_next_pending(time));
            }
            assert!(from_first.update(time, part));
        }
        let mut from_last = StagedLedgerPartsFromLastProgress::new(time);
        for part in (0..1 + trees - first).map(|sent| from_last_part(full, sent).unwrap()) {
            if !from_last.is_part_pending() {
                assert!(from_last.set_next_pending(time));
            }
            assert!(from_last.update(time, part));
        }
        assert_eq!(
            staged_ledger_parts_complete(&from_first, &from_last),
            Some(true)
        );
        take_staged_ledger_parts(&mut from_first, &mut from_last).unwrap()
    }

    fn lean_trees(
        scan_state: &v2::TransactionSnarkScanStateStableV2,
    ) -> Vec<v2::TransactionSnarkScanStateStableV2ScanStateTreesA> {
        let trees = &scan_state.scan_state.trees;
        std::iter::once(&trees.0)
            .chain(trees.1.iter())
            .map(|tree| tree.with_leaner_trees())
            .collect()
    }

    /// Two relays that applied the blocks themselves hold completed (`Done`)
    /// merge jobs with their proofs, and send every tree without them, one
    /// from the first tree and one from the last. Wherever the two streams
    /// meet, the web node receives each tree once, in order and lean, and the
    /// union makes the staged ledger hash of the full scan state.
    #[test]
    fn lean_trees_from_two_relays_make_the_full_staged_ledger_hash() {
        use mina_p2p_messages::binprot::BinProtRead;
        use v2::{
            ParallelScanJobStatusStableV1 as Status,
            TransactionSnarkScanStateStableV2ScanStateTreesA as Tree,
            TransactionSnarkScanStateStableV2ScanStateTreesAMergeT1 as MergeJob,
            TransactionSnarkScanStateStableV2ScanStateTreesAMergeT1Full as Full,
        };

        let bytes =
            include_bytes!("../../../../../../ledger/src/verifier/fixtures/mainnet-551267-1.bin");
        let work = v2::TransactionSnarkWorkTStableV2::binprot_read(&mut bytes.as_slice()).unwrap();
        let proof = match work.proofs {
            v2::TransactionSnarkWorkTStableV2Proofs::One(proof) => proof,
            v2::TransactionSnarkWorkTStableV2Proofs::Two((proof, _)) => proof,
        };
        // Tree `i`: its root merge job done with sequence number `i + 1`,
        // and below it one job done and one waiting for its second proof,
        // which is pending work and stays. Below those, a job with both
        // proofs not merged yet, pending work too, marks the tree with
        // sequence number `100 + i`, so that lean trees still differ.
        let tree = |template: &Tree, i: u64| {
            let Tree::Node {
                depth,
                value,
                sub_tree,
            } = template
            else {
                panic!("a scan state tree has merge nodes")
            };
            let done = |seq_no: u64| {
                MergeJob::Full(Box::new(Full {
                    left: proof.clone(),
                    right: proof.clone(),
                    seq_no: v2::ParallelScanSequenceNumberStableV1(seq_no.into()),
                    status: Status::Done,
                }))
            };
            let mut value = value.clone();
            value[0].1 = done(i + 1);
            let Tree::Node {
                depth: sub_depth,
                value: sub_value,
                sub_tree: rest,
            } = &**sub_tree
            else {
                panic!("a scan state tree has two levels of merge nodes")
            };
            let mut sub_value = sub_value.clone();
            sub_value[0].1 = done(i + 1);
            sub_value[1].1 = MergeJob::Part(Box::new(proof.clone()));
            let Tree::Node {
                depth: rest_depth,
                value: rest_value,
                sub_tree: leaves,
            } = &**rest
            else {
                panic!("a scan state tree has three levels of merge nodes")
            };
            let mut rest_value = rest_value.clone();
            rest_value[0].1 = MergeJob::Full(Box::new(Full {
                left: proof.clone(),
                right: proof.clone(),
                seq_no: v2::ParallelScanSequenceNumberStableV1((100 + i).into()),
                status: Status::Todo,
            }));
            Tree::Node {
                depth: *depth,
                value,
                sub_tree: Box::new(Tree::Node {
                    depth: *sub_depth,
                    value: sub_value,
                    sub_tree: Box::new(Tree::Node {
                        depth: *rest_depth,
                        value: rest_value,
                        sub_tree: leaves.clone(),
                    }),
                }),
            }
        };

        const TREES: usize = 5;
        let (wire, _) = parts();
        let mut full = (*wire).clone();
        let trees = &mut full.scan_state.scan_state.trees;
        let template = trees.0.clone();
        trees.0 = tree(&template, 0);
        trees.1 = (1..TREES as u64).map(|i| tree(&template, i)).collect();
        let (_, hash) = StagedLedgerAuxAndPendingCoinbasesValid::convert(&full).unwrap();
        assert!(StagedLedgerAuxAndPendingCoinbasesValidated::validate(
            &Arc::new(full.clone()),
            &hash
        )
        .is_valid());
        let lean = lean_trees(&full.scan_state);
        assert!(bytes_of(&lean) < bytes_of(&full_trees(&full.scan_state)));
        // Each tree lean is smaller than full, and differs from the others.
        for (i, (lean_tree, full_tree)) in lean.iter().zip(full_trees(&full.scan_state)).enumerate()
        {
            let size = |tree| bytes_of(std::slice::from_ref(tree));
            assert!(size(lean_tree) < size(&full_tree), "tree {i}");
            assert!(!lean[..i].contains(lean_tree), "tree {i}");
        }

        for first in 0..TREES {
            let union = union_of_two_streams(&full, first);
            // The trees as received, not leaned again.
            assert_eq!(full_trees(&union.scan_state), lean, "meeting after {first}");
            assert!(
                StagedLedgerAuxAndPendingCoinbasesValidated::validate(&Arc::new(union), &hash)
                    .is_valid(),
                "meeting after {first}"
            );
        }
    }

    fn full_trees(
        scan_state: &v2::TransactionSnarkScanStateStableV2,
    ) -> Vec<v2::TransactionSnarkScanStateStableV2ScanStateTreesA> {
        let trees = &scan_state.scan_state.trees;
        std::iter::once(&trees.0)
            .chain(trees.1.iter())
            .cloned()
            .collect()
    }

    fn bytes_of(trees: &[v2::TransactionSnarkScanStateStableV2ScanStateTreesA]) -> usize {
        trees.iter().map(|tree| bytes(tree).len()).sum()
    }

    /// The same on a stored root, where the relays' trees are as a
    /// long-running node holds them. Needs a frontier store's parts file:
    ///
    /// ```text
    /// MINA_PERSIST_PARTS=<store>/<chain id>/frontier/parts-<root>.bin \
    ///     cargo test --release -p mina-node --lib stored_root_lean -- --ignored
    /// ```
    #[test]
    #[ignore = "needs a parts file in MINA_PERSIST_PARTS"]
    fn stored_root_lean_trees_from_two_relays_make_its_staged_ledger_hash() {
        use mina_p2p_messages::binprot::BinProtRead;

        let path = std::env::var("MINA_PERSIST_PARTS").expect("MINA_PERSIST_PARTS");
        let bytes = std::fs::read(&path).unwrap();
        let full = StagedLedgerAuxAndPendingCoinbases::binprot_read(&mut bytes.as_slice()).unwrap();
        let (sent, hash) = StagedLedgerAuxAndPendingCoinbasesValid::convert(&full).unwrap();
        let lean = lean_trees(&full.scan_state);
        assert_ne!(lean, full_trees(&full.scan_state), "no Done merge jobs");
        eprintln!(
            "{} trees, {} bytes full, {} bytes lean",
            lean.len(),
            bytes_of(&full_trees(&full.scan_state)),
            bytes_of(&lean)
        );
        let trees = lean.len();
        for first in [0, 1, trees / 2, trees - 1] {
            let union = union_of_two_streams(&full, first);
            assert_eq!(full_trees(&union.scan_state), lean, "meeting after {first}");
            let (received, union_hash) =
                StagedLedgerAuxAndPendingCoinbasesValid::convert(&union).unwrap();
            assert_eq!(union_hash, hash, "meeting after {first}");
            assert_eq!(
                received.has_required_protocol_states(),
                sent.has_required_protocol_states()
            );
        }
    }
}
