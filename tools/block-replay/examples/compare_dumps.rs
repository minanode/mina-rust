//! Compares two staged ledger dumps (`mina_block_replay::dump`) part by part,
//! for example one a node wrote with `MINA_DUMP_RECONSTRUCT` and one
//! `mina-block-replay --write-dump` wrote for the same block:
//!
//!   cargo run --release -p mina-block-replay --example compare_dumps -- A B
//!
//! Prints, for each part, whether the two are the same, and where the first
//! difference is. The protocol states are compared as sets, since a node's
//! dump lists them in the order its peer sent them. When the scan states
//! differ, it compares their jobs too: base jobs field by field, and merge
//! jobs by kind and proofs.

use std::collections::{BTreeMap, BTreeSet};

use ledger::scan_state::scan_state::{JobValue, JobValueBase, JobValueMerge, ScanState};
use mina_block_replay::dump::sorted_indexes;
use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    list::List,
    v2,
};

/// A dump's parts, as the bytes they were read from.
struct Parts {
    names: [&'static str; 4],
    bytes: [Vec<u8>; 4],
    states: List<v2::MinaStateProtocolStateValueStableV2>,
}

fn split(bytes: &[u8]) -> anyhow::Result<Parts> {
    let mut rest = bytes;
    let mut part = |read: &dyn Fn(&mut &[u8]) -> anyhow::Result<()>| {
        let before = rest;
        read(&mut rest)?;
        Ok::<_, anyhow::Error>(before[..before.len() - rest.len()].to_vec())
    };
    let accounts = part(&|r| {
        Vec::<v2::MinaBaseAccountBinableArgStableV2>::binprot_read(r)?;
        Ok(())
    })?;
    let scan_state = part(&|r| {
        v2::TransactionSnarkScanStateStableV2::binprot_read(r)?;
        Ok(())
    })?;
    let pending_coinbase = part(&|r| {
        v2::MinaBasePendingCoinbaseStableV2::binprot_read(r)?;
        Ok(())
    })?;
    let staged_ledger_hash = part(&|r| {
        v2::LedgerHash::binprot_read(r)?;
        Ok(())
    })?;
    let states = List::<v2::MinaStateProtocolStateValueStableV2>::binprot_read(&mut rest)?;
    Ok(Parts {
        names: [
            "accounts",
            "scan state",
            "pending coinbase",
            "staged ledger hash",
        ],
        bytes: [accounts, scan_state, pending_coinbase, staged_ledger_hash],
        states,
    })
}

/// A pending coinbase's bytes with its stack indexes sorted.
fn sorted_pending_coinbase(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let pending_coinbase = v2::MinaBasePendingCoinbaseStableV2::binprot_read(&mut &bytes[..])?;
    let mut sorted = Vec::new();
    sorted_indexes(pending_coinbase).binprot_write(&mut sorted)?;
    Ok(sorted)
}

fn bytes(value: &impl BinProtWrite) -> Vec<u8> {
    let mut bytes = Vec::new();
    value.binprot_write(&mut bytes).unwrap();
    bytes
}

/// The fields of a base job, as in `TransactionWithWitness`.
const FIELDS: [&str; 7] = [
    "transaction with status",
    "state hash",
    "statement",
    "init stack",
    "first pass ledger witness",
    "second pass ledger witness",
    "block global slot",
];

/// A full base job: its fields' bytes, and the number of accounts in its two
/// ledger witnesses.
type BaseJob = ([Vec<u8>; 7], [usize; 2]);

/// A merge job: its kind, and its proofs' bytes.
type MergeJob = (&'static str, Vec<u8>);

/// The full base jobs of a scan state, and its merge jobs, by tree (the
/// latest first) and position in the tree.
#[allow(clippy::type_complexity)]
fn jobs(
    scan_state: &[u8],
) -> anyhow::Result<(
    BTreeMap<(usize, usize), BaseJob>,
    BTreeMap<(usize, usize), MergeJob>,
)> {
    let scan_state = v2::TransactionSnarkScanStateStableV2::binprot_read(&mut &scan_state[..])?;
    let scan_state = ScanState::try_from(&scan_state)?;
    let mut jobs = BTreeMap::new();
    let mut merges = BTreeMap::new();
    for (tree, values) in scan_state.view().enumerate() {
        for value in values {
            let proof =
                |proof: &std::sync::Arc<_>| bytes(&v2::LedgerProofProdStableV2::from(&**proof));
            let record = match value.job {
                JobValue::Leaf(JobValueBase::Full(record)) => record,
                JobValue::Leaf(JobValueBase::Empty) => continue,
                JobValue::Node(JobValueMerge::Empty) => {
                    merges.insert((tree, value.index()), ("empty", Vec::new()));
                    continue;
                }
                JobValue::Node(JobValueMerge::Part(left)) => {
                    merges.insert((tree, value.index()), ("part", proof(left)));
                    continue;
                }
                JobValue::Node(JobValueMerge::Full(record)) => {
                    let mut both = proof(&record.left);
                    both.extend(proof(&record.right));
                    merges.insert((tree, value.index()), ("full", both));
                    continue;
                }
            };
            let job =
                v2::TransactionSnarkScanStateTransactionWithWitnessStableV2::from(&*record.job);
            let fields = [
                bytes(&job.transaction_with_status),
                bytes(&job.state_hash),
                bytes(&job.statement),
                bytes(&job.init_stack),
                bytes(&job.first_pass_ledger_witness),
                bytes(&job.second_pass_ledger_witness),
                bytes(&job.block_global_slot),
            ];
            let accounts = [
                job.first_pass_ledger_witness.indexes.len(),
                job.second_pass_ledger_witness.indexes.len(),
            ];
            jobs.insert((tree, value.index()), (fields, accounts));
        }
    }
    Ok((jobs, merges))
}

/// Compares the jobs of two scan states: base jobs field by field, merge
/// jobs by kind and proofs.
fn compare_jobs(a: &[u8], b: &[u8]) -> anyhow::Result<()> {
    let ((a, a_merges), (b, b_merges)) = (jobs(a)?, jobs(b)?);
    for kind in ["empty", "part", "full"] {
        let of_kind = |merges: &BTreeMap<_, MergeJob>| {
            merges
                .iter()
                .filter(|(_, (k, _))| *k == kind)
                .map(|(key, (_, proofs))| (*key, proofs.len()))
                .collect::<BTreeMap<_, _>>()
        };
        let (x, y) = (of_kind(&a_merges), of_kind(&b_merges));
        let same_proofs = x
            .keys()
            .filter(|key| y.contains_key(*key) && a_merges[*key].1 == b_merges[*key].1)
            .count();
        println!(
            "  {kind} merge jobs: {} and {}, {} with the same proofs, {} and {} bytes of proofs",
            x.len(),
            y.len(),
            same_proofs,
            x.values().sum::<usize>(),
            y.values().sum::<usize>()
        );
    }
    let both: Vec<_> = a.keys().filter(|key| b.contains_key(key)).collect();
    println!(
        "  base jobs: {} and {}, {} at the same positions",
        a.len(),
        b.len(),
        both.len()
    );
    for (field, name) in FIELDS.iter().enumerate() {
        let (mut differ, mut a_bytes, mut b_bytes) = (0, 0, 0);
        for key in &both {
            let (x, y) = (&a[*key].0[field], &b[*key].0[field]);
            differ += usize::from(x != y);
            a_bytes += x.len();
            b_bytes += y.len();
        }
        println!("  {name}: differs in {differ}, {a_bytes} and {b_bytes} bytes");
    }
    for (witness, name) in ["first", "second"].iter().enumerate() {
        let count = |jobs: &BTreeMap<_, BaseJob>| -> usize {
            both.iter().map(|key| jobs[*key].1[witness]).sum()
        };
        println!(
            "  {name} pass ledger witnesses: {} and {} accounts",
            count(&a),
            count(&b)
        );
    }
    Ok(())
}

fn state_bytes(states: &List<v2::MinaStateProtocolStateValueStableV2>) -> BTreeSet<Vec<u8>> {
    states
        .iter()
        .map(|state| {
            let mut bytes = Vec::new();
            state.binprot_write(&mut bytes).unwrap();
            bytes
        })
        .collect()
}

fn main() -> anyhow::Result<()> {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let [a, b] = &paths[..] else {
        anyhow::bail!("usage: compare_dumps A B");
    };
    let (a_bytes, b_bytes) = (std::fs::read(a)?, std::fs::read(b)?);
    let (a, b) = (split(&a_bytes)?, split(&b_bytes)?);
    let mut same = true;
    for (i, name) in a.names.iter().enumerate() {
        let (x, y) = (&a.bytes[i], &b.bytes[i]);
        if i == 2 && x != y && sorted_pending_coinbase(x)? == sorted_pending_coinbase(y)? {
            println!(
                "{name}: same but for the order of the stack indexes, {} bytes",
                x.len()
            );
            continue;
        }
        match x.iter().zip(y).position(|(x, y)| x != y) {
            None if x.len() == y.len() => println!("{name}: same, {} bytes", x.len()),
            first => {
                same = false;
                let differing = x.iter().zip(y).filter(|(x, y)| x != y).count();
                println!(
                    "{name}: differ, {} and {} bytes, first at byte {} of the part, {differing} bytes differ",
                    x.len(),
                    y.len(),
                    first.unwrap_or(x.len().min(y.len()))
                );
                if i == 1 {
                    compare_jobs(x, y)?;
                }
            }
        }
    }
    let (x, y) = (state_bytes(&a.states), state_bytes(&b.states));
    if x == y {
        let order = if a.states.iter().eq(b.states.iter()) {
            "in the same order"
        } else {
            "in another order"
        };
        println!("protocol states: same {}, {order}", x.len());
    } else {
        same = false;
        println!(
            "protocol states: differ, {} and {}, {} in both",
            x.len(),
            y.len(),
            x.intersection(&y).count()
        );
    }
    if same {
        println!("the dumps hold the same staged ledger");
    } else {
        std::process::exit(1);
    }
    Ok(())
}
