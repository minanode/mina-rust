//! Verify the SNARK work in precomputed blocks, the completed works of their
//! staged ledger diffs, against transaction verifier keys, and check that
//! tampered work is rejected.
//!
//! Proofs are checked with `ledger::proofs::verification::verify_transaction`,
//! the function the node uses for snark pool work and for completed works in
//! blocks it applies: one by one, or with `--batch` a block's proofs at once,
//! as the node does, and one by one only if that fails. One by one costs about
//! 20 times more per proof.
//!
//!   cargo run --release -p mina-block-replay --example verify_snark_work -- \
//!       --chain chain.txt --blocks blocks --batch --tamper 48
//!
//! Every proof's sok digest is also checked against the digest of its work's
//! fee and prover. The tamper checks change one statement field or one proof
//! field at a time, give a proof another work's statement, and put one
//! tampered proof in a batch of valid ones; each must be rejected.
//!
//! `--key` takes a verification key in the OCaml JSON form (as the golden
//! files `mainnet_transaction_snark_vk.json` hold) or the index JSON mina-tree
//! embeds, and may be repeated. Without it, the node's own
//! `TransactionVerifier::make()` is used, which loads the embedded index for
//! `--network`.

use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use clap::Parser;
use ledger::{
    proofs::{verification::verify_transaction, verifiers::TransactionVerifier, VerifierIndex},
    scan_state::{
        currency::{Fee, Signed},
        scan_state::transaction_snark::{SokDigest, SokMessage, Statement},
    },
    verifier::get_srs,
};
use mina_block_replay::read_block;
use mina_curves::pasta::{Fp, Fq};
use mina_p2p_messages::{
    bigint::BigInt,
    number::UInt64,
    v2::{
        LedgerProofProdStableV2, LimbVectorConstantHex64StableV1, TransactionSnarkProofStableV2,
        TransactionSnarkWorkTStableV2, TransactionSnarkWorkTStableV2Proofs,
    },
};
use rayon::prelude::*;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "mainnet")]
    network: String,
    /// `<height> <state hash>` per line, oldest first.
    #[arg(long)]
    chain: PathBuf,
    /// Directory with the precomputed blocks.
    #[arg(long)]
    blocks: PathBuf,
    /// Verification keys to check every proof against, in order. The first
    /// one is used for the tamper checks. Default: the embedded index.
    #[arg(long)]
    key: Vec<PathBuf>,
    /// Use every n-th block of the chain.
    #[arg(long, default_value_t = 1)]
    every: usize,
    /// Skip blocks below this height.
    #[arg(long)]
    from: Option<u32>,
    /// Skip blocks above this height.
    #[arg(long)]
    to: Option<u32>,
    /// Stop collecting blocks once this many proofs have been seen.
    #[arg(long)]
    max_proofs: Option<usize>,
    /// Worker threads.
    #[arg(long, default_value_t = 8)]
    threads: usize,
    /// Run the tamper checks on this many proofs, from different blocks.
    #[arg(long, default_value_t = 0)]
    tamper: usize,
    /// Verify each block's proofs as one batch, as the node does, and one by
    /// one only when the batch fails.
    #[arg(long)]
    batch: bool,
    /// Where failed verifications are dumped (`<dir>/debug`).
    #[arg(long, default_value = "/tmp/verify-snark-work")]
    work_dir: PathBuf,
}

/// One transaction proof of a completed work.
#[derive(Clone)]
struct Item {
    height: u32,
    work: usize,
    part: usize,
    statement: Statement<SokDigest>,
    proof: TransactionSnarkProofStableV2,
}

impl Item {
    fn kind(&self) -> String {
        let branch = &self
            .proof
            .0
            .statement
            .proof_state
            .deferred_values
            .branch_data;
        format!(
            "{:?}/d{}",
            branch.proofs_verified,
            branch.domain_log2.as_u8()
        )
    }

    fn label(&self) -> String {
        format!("{}#{}.{}", self.height, self.work, self.part)
    }
}

fn verify_one(
    statement: &Statement<SokDigest>,
    proof: &TransactionSnarkProofStableV2,
    index: &VerifierIndex<Fq>,
) -> bool {
    let srs = get_srs::<Fp>();
    verify_transaction(std::iter::once((statement, proof)), index, &srs)
}

/// The proofs of one completed work, after checking that the statement's sok
/// digest is the digest of the work's fee and prover.
fn work_items(
    height: u32,
    index: usize,
    work: &TransactionSnarkWorkTStableV2,
    sok_mismatches: &AtomicUsize,
) -> anyhow::Result<Vec<Item>> {
    let message = SokMessage::create((&work.fee).into(), (&work.prover).try_into()?);
    let digest = message.digest();
    let proofs: Vec<&LedgerProofProdStableV2> = match &work.proofs {
        TransactionSnarkWorkTStableV2Proofs::One(p) => vec![p],
        TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => vec![p, q],
    };
    let mut items = Vec::new();
    for (part, proof) in proofs.into_iter().enumerate() {
        let statement = Statement::<SokDigest>::try_from(&proof.0.statement)?;
        if statement.sok_digest != digest {
            sok_mismatches.fetch_add(1, Ordering::Relaxed);
        }
        items.push(Item {
            height,
            work: index,
            part,
            statement,
            proof: (*proof.0.proof.get()?).clone(),
        });
    }
    Ok(items)
}

fn read_items(
    args: &Args,
    height: u32,
    hash: &str,
    sok_mismatches: &AtomicUsize,
) -> anyhow::Result<Vec<Item>> {
    let block = read_block(&args.blocks, height, hash)?;
    let diff = &block.staged_ledger_diff.diff;
    let works = diff
        .0
        .completed_works
        .iter()
        .chain(diff.1.iter().flat_map(|d| d.completed_works.iter()));
    let mut items = Vec::new();
    for (i, work) in works.enumerate() {
        items.extend(work_items(height, i, work, sok_mismatches)?);
    }
    Ok(items)
}

fn load_key(path: &PathBuf) -> anyhow::Result<VerifierIndex<Fq>> {
    let json = std::fs::read_to_string(path)?;
    mina_web_verifier::verifier_index_auto(&json).map_err(anyhow::Error::msg)
}

#[derive(Default)]
struct Tally {
    ok: usize,
    failed: usize,
    time: Duration,
    batch_time: Duration,
    by_kind: std::collections::BTreeMap<String, (usize, usize)>,
    failures: Vec<String>,
}

fn add_one(limb: &LimbVectorConstantHex64StableV1) -> LimbVectorConstantHex64StableV1 {
    LimbVectorConstantHex64StableV1(UInt64::from(limb.0.as_u64().wrapping_add(1)))
}

fn bigint_plus_one(x: &BigInt) -> BigInt {
    let f: Fp = x.to_field().unwrap_or_default();
    BigInt::from(f + Fp::from(1u64))
}

/// Ways to change a valid proof or its statement. Each must make it fail.
#[allow(clippy::type_complexity)]
fn tampers() -> Vec<(
    &'static str,
    fn(&mut Statement<SokDigest>, &mut TransactionSnarkProofStableV2),
)> {
    vec![
        ("statement: fee_excess_l + 1 nanomina", |s, _| {
            let f = &s.fee_excess.fee_excess_l;
            s.fee_excess.fee_excess_l =
                Signed::create(Fee::from_u64(f.magnitude.as_u64() + 1), f.sgn);
        }),
        ("statement: supply_increase negated or +1", |s, _| {
            s.supply_increase = if s.supply_increase.is_zero() {
                Signed::of_unsigned(ledger::scan_state::currency::Amount::from_u64(1))
            } else {
                s.supply_increase.negate()
            };
        }),
        ("statement: source first_pass_ledger + 1", |s, _| {
            s.source.first_pass_ledger += Fp::from(1u64);
        }),
        ("statement: target second_pass_ledger + 1", |s, _| {
            s.target.second_pass_ledger += Fp::from(1u64);
        }),
        ("statement: sok_digest (other fee/prover)", |s, _| {
            s.sok_digest.0[0] ^= 1;
        }),
        ("proof: ft_eval1 + 1", |_, p| {
            p.0.proof.ft_eval1 = bigint_plus_one(&p.0.proof.ft_eval1);
        }),
        ("proof: w_comm[0] and w_comm[1] swapped", |_, p| {
            p.0.proof.commitments.w_comm.0.swap(0, 1);
        }),
        ("proof: bulletproof z_1 + 1", |_, p| {
            p.0.proof.bulletproof.z_1 = bigint_plus_one(&p.0.proof.bulletproof.z_1);
        }),
        ("proof: deferred plonk alpha limb + 1", |_, p| {
            let alpha = &mut p.0.statement.proof_state.deferred_values.plonk.alpha;
            alpha.inner.0[0] = add_one(&alpha.inner.0[0]);
        }),
        ("proof: prev_evals ft_eval1 + 1", |_, p| {
            p.0.prev_evals.ft_eval1 = bigint_plus_one(&p.0.prev_evals.ft_eval1);
        }),
        ("proof: bulletproof challenge limb + 1", |_, p| {
            let c = &mut p
                .0
                .statement
                .proof_state
                .deferred_values
                .bulletproof_challenges
                .0[0];
            c.prechallenge.inner.0[0] = add_one(&c.prechallenge.inner.0[0]);
        }),
    ]
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    mina_core::NetworkConfig::init(&args.network).map_err(anyhow::Error::msg)?;
    mina_core::set_work_dir(args.work_dir.clone());
    rayon::ThreadPoolBuilder::new()
        .num_threads(args.threads)
        .build_global()?;

    // Keys.
    let started = Instant::now();
    let mut keys: Vec<(String, Arc<VerifierIndex<Fq>>)> = Vec::new();
    if args.key.is_empty() {
        let index: Arc<VerifierIndex<Fq>> = TransactionVerifier::make().into();
        keys.push((format!("embedded {}", args.network), index));
    }
    for path in &args.key {
        keys.push((path.display().to_string(), Arc::new(load_key(path)?)));
    }
    let _ = get_srs::<Fp>();
    println!("{} key(s) ready in {:.1?}", keys.len(), started.elapsed());
    if keys.len() > 1 {
        let first = serde_json::to_value(&*keys[0].1)?;
        for (name, key) in &keys[1..] {
            let other = serde_json::to_value(&**key)?;
            let differing: Vec<&String> = first
                .as_object()
                .into_iter()
                .flat_map(|o| o.iter())
                .filter(|(k, v)| other.get(k.as_str()) != Some(v))
                .map(|(k, _)| k)
                .collect();
            println!(
                "key {name} vs {}: differing fields {differing:?}",
                keys[0].0
            );
        }
    }

    // Blocks.
    let mut chain = Vec::new();
    for line in BufReader::new(File::open(&args.chain)?).lines() {
        let line = line?;
        let mut fields = line.split_whitespace();
        let (Some(height), Some(hash)) = (fields.next(), fields.next()) else {
            continue;
        };
        let height: u32 = height.parse()?;
        if args.from.is_some_and(|f| height < f) || args.to.is_some_and(|t| height > t) {
            continue;
        }
        chain.push((height, hash.to_string()));
    }
    let selected: Vec<(u32, String)> = chain.into_iter().step_by(args.every.max(1)).collect();
    println!(
        "{} blocks selected ({}..={})",
        selected.len(),
        selected.first().map_or(0, |b| b.0),
        selected.last().map_or(0, |b| b.0)
    );

    let sok_mismatches = AtomicUsize::new(0);
    let seen = AtomicUsize::new(0);
    let blocks_read = AtomicUsize::new(0);
    let blocks_without_work = AtomicUsize::new(0);
    let tallies: Vec<Mutex<Tally>> = keys.iter().map(|_| Mutex::new(Tally::default())).collect();
    let tamper_pool: Mutex<Vec<Item>> = Mutex::new(Vec::new());
    let started = Instant::now();

    selected.par_iter().try_for_each(|(height, hash)| {
        if args
            .max_proofs
            .is_some_and(|m| seen.load(Ordering::Relaxed) >= m)
        {
            return Ok(());
        }
        let items = read_items(&args, *height, hash, &sok_mismatches)?;
        blocks_read.fetch_add(1, Ordering::Relaxed);
        if items.is_empty() {
            blocks_without_work.fetch_add(1, Ordering::Relaxed);
        }
        let n = seen.fetch_add(items.len(), Ordering::Relaxed) + items.len();
        for (k, (_, key)) in keys.iter().enumerate() {
            // In batch mode a block's proofs are checked together, as the node
            // does, and one by one only if the batch fails.
            let batch_ok = args.batch && !items.is_empty() && {
                let t = Instant::now();
                let srs = get_srs::<Fp>();
                let ok =
                    verify_transaction(items.iter().map(|i| (&i.statement, &i.proof)), key, &srs);
                tallies[k].lock().unwrap().batch_time += t.elapsed();
                ok
            };
            let results: Vec<(bool, Duration)> = if batch_ok {
                items.iter().map(|_| (true, Duration::ZERO)).collect()
            } else {
                items
                    .par_iter()
                    .map(|item| {
                        let t = Instant::now();
                        let ok = verify_one(&item.statement, &item.proof, key);
                        (ok, t.elapsed())
                    })
                    .collect()
            };
            let mut guard = tallies[k].lock().unwrap();
            let tally = &mut *guard;
            for (item, (ok, time)) in items.iter().zip(results) {
                tally.time += time;
                let entry = tally.by_kind.entry(item.kind()).or_default();
                if ok {
                    tally.ok += 1;
                    entry.0 += 1;
                } else {
                    tally.failed += 1;
                    entry.1 += 1;
                    if tally.failures.len() < 20 {
                        tally.failures.push(item.label());
                    }
                }
            }
        }
        if args.tamper > 0 {
            let mut pool = tamper_pool.lock().unwrap();
            // One proof per block, a spread of kinds.
            if pool.len() < args.tamper {
                let have: Vec<String> = pool.iter().map(Item::kind).collect();
                let pick = items
                    .iter()
                    .find(|i| !have.contains(&i.kind()))
                    .or(items.first());
                if let Some(item) = pick {
                    pool.push(item.clone());
                }
            }
        }
        let read = blocks_read.load(Ordering::Relaxed);
        if read.is_multiple_of(100) {
            println!(
                "  {read} blocks, {n} proofs, {:.0?} elapsed",
                started.elapsed()
            );
        }
        anyhow::Ok(())
    })?;

    let elapsed = started.elapsed();
    println!(
        "\n{} blocks read ({} without completed work), {} proofs, in {:.1?}",
        blocks_read.load(Ordering::Relaxed),
        blocks_without_work.load(Ordering::Relaxed),
        seen.load(Ordering::Relaxed),
        elapsed
    );
    println!(
        "sok digest != digest(fee, prover): {}",
        sok_mismatches.load(Ordering::Relaxed)
    );
    for ((name, _), tally) in keys.iter().zip(&tallies) {
        let tally = tally.lock().unwrap();
        let total = tally.ok + tally.failed;
        println!(
            "key {name}: {} verified, {} failed, mean {:.1?} per proof one by one, \
             {:.1?} in batches",
            tally.ok,
            tally.failed,
            tally.time / total.max(1) as u32,
            tally.batch_time
        );
        for (kind, (ok, failed)) in &tally.by_kind {
            println!("  {kind}: {ok} verified, {failed} failed");
        }
        if !tally.failures.is_empty() {
            println!("  first failures: {:?}", tally.failures);
        }
    }

    // Tampering, against the first key.
    let pool = tamper_pool.into_inner().unwrap();
    if !pool.is_empty() {
        let (name, key) = &keys[0];
        let kinds: Vec<String> = pool
            .iter()
            .map(|i| format!("{} {}", i.label(), i.kind()))
            .collect();
        println!(
            "\ntamper checks against {name}, {} proofs: {kinds:?}",
            pool.len()
        );
        let originals: Vec<bool> = pool
            .par_iter()
            .map(|i| verify_one(&i.statement, &i.proof, key))
            .collect();
        println!(
            "  untampered: {}/{} verify",
            originals.iter().filter(|ok| **ok).count(),
            pool.len()
        );
        for (what, tamper) in tampers() {
            let results: Vec<bool> = pool
                .par_iter()
                .map(|item| {
                    let (mut s, mut p) = (item.statement.clone(), item.proof.clone());
                    tamper(&mut s, &mut p);
                    verify_one(&s, &p, key)
                })
                .collect();
            let accepted = results.iter().filter(|ok| **ok).count();
            println!(
                "  {what}: {} rejected, {accepted} accepted",
                results.len() - accepted
            );
        }
        // Another work's statement with this proof.
        let results: Vec<bool> = (0..pool.len())
            .into_par_iter()
            .map(|i| {
                let other = &pool[(i + 1) % pool.len()];
                verify_one(&other.statement, &pool[i].proof, key)
            })
            .collect();
        let accepted = results.iter().filter(|ok| **ok).count();
        println!(
            "  statement of another work: {} rejected, {accepted} accepted",
            results.len() - accepted
        );
        // A batch, as the node verifies a block's works: all valid, then one
        // tampered among them.
        let srs = get_srs::<Fp>();
        let batch = |items: &[(Statement<SokDigest>, TransactionSnarkProofStableV2)]| {
            verify_transaction(items.iter().map(|(s, p)| (s, p)), key, &srs)
        };
        let mut items: Vec<_> = pool
            .iter()
            .map(|i| (i.statement.clone(), i.proof.clone()))
            .collect();
        let t = Instant::now();
        let all_valid = batch(&items);
        let batch_time = t.elapsed();
        let last = items.len() - 1;
        items[last].0.sok_digest.0[0] ^= 1;
        let one_bad = batch(&items);
        println!(
            "  batch of {}: all valid -> {all_valid} ({batch_time:.1?}), one tampered -> {one_bad}",
            items.len()
        );
    }
    Ok(())
}
