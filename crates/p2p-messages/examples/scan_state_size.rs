//! Break down the binprot size of a staged ledger aux answer: the parts a
//! node persists (`parts-<state_hash>.bin`) and serves to peers.
//!
//! It prints the bytes per part of the scan state, by job kind and status,
//! and the size after dropping completed (`Done`) merge jobs, which OCaml's
//! binable drops (`with_leaner_trees`) and the scan state hash ignores.
//!
//! ```text
//! cargo run --release -p mina-p2p-messages --example scan_state_size -- \
//!     parts.bin [--write-lean lean.bin]
//! ```

use std::collections::BTreeMap;

use binprot::{BinProtRead, BinProtWrite};
use mina_p2p_messages::{
    list::List,
    v2::{
        LedgerHash, LedgerProofProdStableV2, MinaBasePendingCoinbaseStableV2,
        MinaStateProtocolStateValueStableV2, ParallelScanJobStatusStableV1,
        TransactionSnarkScanStateStableV2, TransactionSnarkScanStateStableV2ScanStateTreesA,
        TransactionSnarkScanStateStableV2ScanStateTreesABaseT1 as Base,
        TransactionSnarkScanStateStableV2ScanStateTreesAMergeT1 as Merge,
        TransactionSnarkScanStateTransactionWithWitnessStableV2,
    },
};
use sha2::{Digest, Sha256};

type Parts = (
    TransactionSnarkScanStateStableV2,
    LedgerHash,
    MinaBasePendingCoinbaseStableV2,
    List<MinaStateProtocolStateValueStableV2>,
);

fn size<T: BinProtWrite>(value: &T) -> usize {
    let mut buf = Vec::new();
    value.binprot_write(&mut buf).unwrap();
    buf.len()
}

#[derive(Default)]
struct Tally {
    count: usize,
    bytes: usize,
}

impl Tally {
    fn add(&mut self, bytes: usize) {
        self.count += 1;
        self.bytes += bytes;
    }
}

#[derive(Default)]
struct Stats {
    kinds: BTreeMap<&'static str, Tally>,
    proofs: Tally,
    proof_bodies: Tally,
    distinct_proofs: BTreeMap<[u8; 32], usize>,
    witness_first: Tally,
    witness_second: Tally,
    transaction: Tally,
}

impl Stats {
    fn proof(&mut self, proof: &LedgerProofProdStableV2) {
        let mut buf = Vec::new();
        proof.binprot_write(&mut buf).unwrap();
        self.proofs.add(buf.len());
        self.proof_bodies.add(size(&proof.0.proof));
        let digest: [u8; 32] = Sha256::digest(&buf).into();
        *self.distinct_proofs.entry(digest).or_default() += buf.len();
    }

    fn base_job(&mut self, job: &TransactionSnarkScanStateTransactionWithWitnessStableV2) {
        self.witness_first.add(size(&job.first_pass_ledger_witness));
        self.witness_second
            .add(size(&job.second_pass_ledger_witness));
        self.transaction.add(size(&job.transaction_with_status));
    }
}

fn status(s: &ParallelScanJobStatusStableV1) -> &'static str {
    match s {
        ParallelScanJobStatusStableV1::Todo => "Todo",
        ParallelScanJobStatusStableV1::Done => "Done",
    }
}

fn walk(tree: &TransactionSnarkScanStateStableV2ScanStateTreesA, stats: &mut Stats) {
    match tree {
        TransactionSnarkScanStateStableV2ScanStateTreesA::Leaf(leaves) => {
            for (_, job) in leaves {
                let bytes = size(job);
                match job {
                    Base::Empty => stats.kinds.entry("base Empty").or_default().add(bytes),
                    Base::Full(full) => {
                        let kind = match status(&full.status) {
                            "Todo" => "base Full Todo",
                            _ => "base Full Done",
                        };
                        stats.kinds.entry(kind).or_default().add(bytes);
                        stats.base_job(&full.job);
                    }
                }
            }
        }
        TransactionSnarkScanStateStableV2ScanStateTreesA::Node {
            value, sub_tree, ..
        } => {
            for (_, job) in value {
                let bytes = size(job);
                match job {
                    Merge::Empty => stats.kinds.entry("merge Empty").or_default().add(bytes),
                    Merge::Part(proof) => {
                        stats.kinds.entry("merge Part").or_default().add(bytes);
                        stats.proof(proof);
                    }
                    Merge::Full(full) => {
                        let kind = match status(&full.status) {
                            "Todo" => "merge Full Todo",
                            _ => "merge Full Done",
                        };
                        stats.kinds.entry(kind).or_default().add(bytes);
                        stats.proof(&full.left);
                        stats.proof(&full.right);
                    }
                }
            }
            walk(sub_tree, stats);
        }
    }
}

fn mb(bytes: usize) -> String {
    format!("{:.2} MB", bytes as f64 / 1e6)
}

fn report(name: &str, parts: &Parts) {
    let (scan_state, _, pending_coinbase, needed_blocks) = parts;
    let mut stats = Stats::default();
    let trees = &scan_state.scan_state.trees;
    walk(&trees.0, &mut stats);
    trees.1.iter().for_each(|tree| walk(tree, &mut stats));
    let tree_bytes = size(&trees.0) + trees.1.iter().map(size).sum::<usize>();

    println!("== {name}: {}", mb(size(parts)));
    println!("  trees: {} ({} trees)", mb(tree_bytes), trees.1.len() + 1);
    for (kind, tally) in &stats.kinds {
        println!("    {kind:16} {:5} jobs {}", tally.count, mb(tally.bytes));
    }
    let acc = &scan_state.scan_state.acc;
    println!("  acc: {}", mb(size(acc)));
    println!(
        "  previous incomplete zkapp updates: {}",
        mb(size(&scan_state.previous_incomplete_zkapp_updates))
    );
    println!("  pending coinbase: {}", mb(size(pending_coinbase)));
    println!(
        "  needed blocks: {} ({} states)",
        mb(size(needed_blocks)),
        needed_blocks.len()
    );
    println!(
        "  proofs in trees: {} ({} proofs, {} distinct, proof bodies {})",
        mb(stats.proofs.bytes),
        stats.proofs.count,
        stats.distinct_proofs.len(),
        mb(stats.proof_bodies.bytes)
    );
    println!(
        "  base jobs: transactions {}, first pass witness {}, second pass witness {}",
        mb(stats.transaction.bytes),
        mb(stats.witness_first.bytes),
        mb(stats.witness_second.bytes)
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: scan_state_size PARTS [--write-lean OUT]");
    let write_lean = match (args.next().as_deref(), args.next()) {
        (Some("--write-lean"), Some(out)) => Some(out),
        _ => None,
    };
    let bytes = std::fs::read(&path).unwrap();
    let parts = Parts::binprot_read(&mut bytes.as_slice()).unwrap();
    assert_eq!(size(&parts), bytes.len(), "re-encoding changed the size");
    report("as stored", &parts);

    let mut leaner = parts.clone();
    let trees = &mut leaner.0.scan_state.trees;
    trees.0 = trees.0.with_leaner_trees();
    trees
        .1
        .iter_mut()
        .for_each(|tree| *tree = tree.with_leaner_trees());
    report("without Done merge jobs", &leaner);

    if let Some(out) = write_lean {
        let mut buf = Vec::new();
        leaner.binprot_write(&mut buf).unwrap();
        std::fs::write(out, buf).unwrap();
    }
}
