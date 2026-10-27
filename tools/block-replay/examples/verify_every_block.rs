//! Verify every block in a directory of precomputed blocks, in parallel
//! (`mina_block_replay::verify`): its block proof against the blockchain
//! verifier key, every transaction SNARK of its staged ledger diff against the
//! transaction verifier key, and every zkApp proof against the key its account
//! update names; and the delta block chain proof and the body reference of
//! each header, with the node's checks. With the replay, which checks every
//! staged ledger hash and signature, this validates every block of the chain.
//!
//!   cargo run --release -p mina-block-replay --example verify_every_block -- \
//!       --blocks /path/to/blocks --chain /path/to/chain.txt \
//!       --threads 16 --tsv results.tsv
//!
//! Only blocks carrying the network's Mesa genesis state hash (`--genesis` to
//! choose another) are verified. Blocks of another chain, like the Berkeley
//! blocks the public bucket holds at the first Mesa heights, are listed apart
//! and are not failures.
//!
//! The keys are the node's own, `BlockVerifier::make()` and
//! `TransactionVerifier::make()` for `--network`, unless `--block-key` or
//! `--tx-key` name a key in the OCaml JSON form or the index JSON mina-tree
//! embeds. Like the node, `make()` reads and, when it doesn't match, rewrites
//! the verifier index cache under `$HOME/.mina/circuit-blobs`. zkApp keys come
//! from the account updates of the blocks that set them, so a run over part
//! of a chain may lack some. `--zkapp-keys-genesis` and `--zkapp-keys-dump`
//! add the keys the accounts of a ledger hold: of a prebuilt genesis config
//! (its genesis and staking epoch ledgers), and of a node's root dump (its
//! snarked ledger, as `mina-block-replay --dump` reads it). A proof is still
//! checked only against a key of the hash its account update names.
//!
//! It prints the totals, every failure with its height and hash, and the
//! runtime, and exits with status 1 if a block fails, a file can't be read, or
//! a block of `--chain` is missing.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::Context;
use clap::{Parser, ValueEnum};
use ledger::{
    proofs::{
        verifiers::{BlockVerifier, TransactionVerifier},
        VerifierIndex,
    },
    verifier::get_srs,
    Account, BaseLedger, VerificationKey,
};
use mina_block_replay::{
    dump::ReconstructContext,
    verify::{
        check_files, hex, list_blocks, mesa_genesis, zkapp_keys_of_accounts, BlockResult, Checks,
        Keys, Options, Outcome, Summary, ZkappResult, ZKAPP_PROOFS_CHECKED,
    },
};
use mina_curves::pasta::{Fp, Fq};
use mina_node::transition_frontier::genesis::PrebuiltGenesisConfig;

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Check {
    /// Block proofs.
    Block,
    /// Transaction SNARKs of completed works.
    Transactions,
    /// zkApp proofs of account updates.
    Zkapps,
    /// The delta block chain proof and the body reference, as the node checks
    /// them on a received block.
    Header,
}

#[derive(Parser)]
struct Args {
    /// Network whose embedded keys and Mesa genesis apply.
    #[arg(long, default_value = "mainnet")]
    network: String,
    /// Directory with the precomputed blocks, `<height>-<state hash>.json[.zst]`
    /// or `<network>-<height>-<state hash>.json`.
    #[arg(long)]
    blocks: PathBuf,
    /// Verify only blocks carrying this genesis state hash. Default: the
    /// network's Mesa genesis.
    #[arg(long)]
    genesis: Option<String>,
    /// `<height> <state hash>` per line: every one of these blocks must be in
    /// the directory and verify.
    #[arg(long)]
    chain: Option<PathBuf>,
    /// Which proofs to verify.
    #[arg(long, value_enum, value_delimiter = ',', default_values_t = [Check::Block, Check::Transactions, Check::Zkapps, Check::Header])]
    checks: Vec<Check>,
    /// Blockchain verification key instead of the embedded one.
    #[arg(long)]
    block_key: Option<PathBuf>,
    /// Transaction verification key instead of the embedded one.
    #[arg(long)]
    tx_key: Option<PathBuf>,
    /// Prebuilt genesis config, as `ledger-tool` writes it: the zkApp keys of
    /// its genesis and staking epoch ledgers' accounts serve proofs whose key
    /// no block of the run sets.
    #[arg(long)]
    zkapp_keys_genesis: Vec<PathBuf>,
    /// A node's root dump (`MINA_DUMP_RECONSTRUCT`): the zkApp keys of its
    /// snarked ledger's accounts, likewise.
    #[arg(long)]
    zkapp_keys_dump: Vec<PathBuf>,
    /// Skip blocks below this height.
    #[arg(long)]
    from: Option<u32>,
    /// Skip blocks above this height.
    #[arg(long)]
    to: Option<u32>,
    /// Worker threads.
    #[arg(long, default_value_t = 8)]
    threads: usize,
    /// Verify transaction proofs one by one rather than a block's at once.
    #[arg(long)]
    one_by_one: bool,
    /// Write one line per block here: height, hash, outcome, timings, counts.
    #[arg(long)]
    tsv: Option<PathBuf>,
    /// Where failed verifications are dumped (`<dir>/debug`).
    #[arg(long, default_value = "/tmp/verify-every-block")]
    work_dir: PathBuf,
}

/// The zkApp keys of a prebuilt genesis config's genesis and staking epoch
/// ledgers, and how many accounts they have.
fn genesis_zkapp_keys(path: &Path) -> anyhow::Result<(usize, HashMap<Fp, VerificationKey>)> {
    let (masks, _) = PrebuiltGenesisConfig::load_from_bytes(&std::fs::read(path)?)?;
    let mut accounts = 0;
    let mut keys = HashMap::new();
    for mask in &masks {
        let list = mask.to_list();
        accounts += list.len();
        keys.extend(zkapp_keys_of_accounts(&list));
    }
    Ok((accounts, keys))
}

/// The zkApp keys of a root dump's snarked ledger, and how many accounts it
/// has.
fn dump_zkapp_keys(path: &Path) -> anyhow::Result<(usize, HashMap<Fp, VerificationKey>)> {
    let accounts = ReconstructContext::read(path)?
        .accounts
        .iter()
        .map(Account::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((accounts.len(), zkapp_keys_of_accounts(&accounts)))
}

fn load_key(path: &PathBuf) -> anyhow::Result<Arc<VerifierIndex<Fq>>> {
    let json = std::fs::read_to_string(path)?;
    Ok(Arc::new(
        mina_web_verifier::verifier_index_auto(&json).map_err(anyhow::Error::msg)?,
    ))
}

const TSV_HEADER: &str = "height\thash\tresult\tblock_proof\tblock_ms\tdelta_chain\t\
                          body_reference\tworks\tproofs\tproofs_ok\ttx_ms\tzkapp_proofs\tzkapp_ok\t\
                          zkapp_ms\tdetail";

fn yes_no(check: Option<bool>) -> String {
    check.map_or("-".to_string(), |ok| ok.to_string())
}

fn tsv_line(r: &BlockResult) -> String {
    let rest = match &r.outcome {
        Outcome::Checked(c) => {
            let failed = c.failures(r.height, &r.hash);
            format!(
                "{}\t{}\t{:.1}\t{}\t{}\t{}\t{}\t{}\t{:.1}\t{}\t{}\t{:.1}\t{}",
                if failed.is_empty() { "ok" } else { "FAILED" },
                yes_no(c.block_proof),
                c.block_time.as_secs_f64() * 1e3,
                yes_no(c.delta_chain),
                yes_no(c.body_reference),
                c.works,
                c.proofs.len(),
                c.proofs
                    .iter()
                    .filter(|p| p.verified && p.sok_matches)
                    .count(),
                c.tx_time.as_secs_f64() * 1e3,
                c.zkapp_proofs.len(),
                c.zkapp_proofs
                    .iter()
                    .filter(|p| p.result == ZkappResult::Verified)
                    .count(),
                c.zkapp_time.as_secs_f64() * 1e3,
                failed.join("; ")
            )
        }
        Outcome::OtherChain {
            genesis_state_hash,
            protocol_version,
        } => format!(
            "other-chain{}genesis {genesis_state_hash}, protocol {protocol_version}",
            "\t".repeat(12)
        ),
        Outcome::Unreadable(e) => format!("unreadable{}{e}", "\t".repeat(12)),
    };
    format!("{}\t{}\t{rest}", r.height, r.hash)
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    mina_core::NetworkConfig::init(&args.network).map_err(anyhow::Error::msg)?;
    mina_core::set_work_dir(args.work_dir.clone());
    rayon::ThreadPoolBuilder::new()
        .num_threads(args.threads)
        .build_global()?;
    let genesis = match &args.genesis {
        Some(g) => g.clone(),
        None => mesa_genesis(&args.network)
            .ok_or_else(|| anyhow::anyhow!("no Mesa genesis known for {}", args.network))?
            .to_string(),
    };
    let checks = Checks {
        block: args.checks.contains(&Check::Block),
        transactions: args.checks.contains(&Check::Transactions),
        zkapps: args.checks.contains(&Check::Zkapps),
        header: args.checks.contains(&Check::Header),
    };

    let started = Instant::now();
    let mut ledger_zkapp = HashMap::new();
    // Which of the ledgers hold each key, by the order they were given.
    let mut key_sources: HashMap<Fp, Vec<usize>> = HashMap::new();
    let sources = (args.zkapp_keys_genesis.iter().map(|p| (p, true)))
        .chain(args.zkapp_keys_dump.iter().map(|p| (p, false)));
    for (source, (path, genesis)) in sources.enumerate() {
        let (accounts, keys) = if genesis {
            genesis_zkapp_keys(path)
        } else {
            dump_zkapp_keys(path)
        }
        .with_context(|| format!("zkApp keys from {}", path.display()))?;
        let new = keys
            .keys()
            .filter(|hash| !ledger_zkapp.contains_key(*hash))
            .count();
        println!(
            "zkApp keys from {}: {accounts} accounts, {} distinct keys, {new} not in the ledgers \
             before; {:.1?}",
            path.display(),
            keys.len(),
            started.elapsed()
        );
        for hash in keys.keys() {
            key_sources.entry(*hash).or_default().push(source);
        }
        ledger_zkapp.extend(keys);
    }
    let source_names: Vec<String> = (args.zkapp_keys_genesis.iter())
        .chain(&args.zkapp_keys_dump)
        .map(|p| {
            p.file_name().map_or(p.display().to_string(), |n| {
                n.to_string_lossy().into_owned()
            })
        })
        .collect();
    let keys = Keys {
        block: match &args.block_key {
            Some(path) => load_key(path)?,
            None => BlockVerifier::make().into(),
        },
        transaction: match &args.tx_key {
            Some(path) => load_key(path)?,
            None => TransactionVerifier::make().into(),
        },
        ledger_zkapp: Arc::new(ledger_zkapp),
    };
    let _ = get_srs::<Fp>();
    let name = |key: &Option<PathBuf>| {
        key.as_ref()
            .map_or(format!("embedded {}", args.network), |p| {
                p.display().to_string()
            })
    };
    println!(
        "keys: block {}, transaction {}; ready with the SRS in {:.1?}",
        name(&args.block_key),
        name(&args.tx_key),
        started.elapsed()
    );

    let files: Vec<_> = list_blocks(&args.blocks)?
        .into_iter()
        .filter(|f| {
            args.from.is_none_or(|from| f.height >= from) && args.to.is_none_or(|to| f.height <= to)
        })
        .collect();
    println!(
        "{} block files in {}, heights {} to {}; {} threads; checks {checks:?}{}",
        files.len(),
        args.blocks.display(),
        files.first().map_or(0, |f| f.height),
        files.last().map_or(0, |f| f.height),
        args.threads,
        if args.one_by_one {
            ", transaction proofs one by one"
        } else {
            ", transaction proofs in a batch per block"
        }
    );

    let options = Options {
        genesis_state_hash: genesis.clone(),
        batch: !args.one_by_one,
        checks,
    };
    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let proofs = AtomicUsize::new(0);
    let zkapp_proofs = AtomicUsize::new(0);
    let finished = AtomicBool::new(false);
    let started = Instant::now();
    let visit = |r: &BlockResult| {
        match &r.outcome {
            Outcome::Checked(c) => {
                proofs.fetch_add(c.proofs.len(), Ordering::Relaxed);
                zkapp_proofs.fetch_add(c.zkapp_proofs.len(), Ordering::Relaxed);
                let reasons = c.failures(r.height, &r.hash);
                // zkApp proofs are verified after every block has been read.
                let reasons: Vec<_> = reasons
                    .into_iter()
                    .filter(|reason| !reason.ends_with("was not verified"))
                    .collect();
                if !reasons.is_empty() {
                    failed.fetch_add(1, Ordering::Relaxed);
                    println!("FAILED {} {}: {}", r.height, r.hash, reasons.join("; "));
                }
            }
            Outcome::Unreadable(e) => {
                failed.fetch_add(1, Ordering::Relaxed);
                println!("UNREADABLE {} {}: {e}", r.height, r.hash);
            }
            Outcome::OtherChain { .. } => {}
        }
        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(500) {
            println!(
                "  {n}/{} blocks, {} transaction proofs, {} failed blocks, {:.0?}",
                files.len(),
                proofs.load(Ordering::Relaxed),
                failed.load(Ordering::Relaxed),
                started.elapsed()
            );
        }
    };
    // zkApp proofs are checked once every block has been read, which can take
    // hours: report them every two minutes.
    let results = std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut last = 0;
            let mut tick = Instant::now();
            while !finished.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                let checked = ZKAPP_PROOFS_CHECKED.load(Ordering::Relaxed);
                if tick.elapsed() >= Duration::from_secs(120) && checked != last {
                    println!(
                        "  zkApp proofs: {checked}/{} checked, {:.0?}",
                        zkapp_proofs.load(Ordering::Relaxed),
                        started.elapsed()
                    );
                    (last, tick) = (checked, Instant::now());
                }
            }
        });
        let results = check_files(&files, &keys, &options, visit);
        finished.store(true, Ordering::Relaxed);
        results
    });
    let elapsed = started.elapsed();

    if let Some(path) = &args.tsv {
        let mut file = BufWriter::new(File::create(path)?);
        writeln!(file, "{TSV_HEADER}")?;
        for r in &results {
            writeln!(file, "{}", tsv_line(r))?;
        }
    }

    let summary = Summary::new(&genesis, &results);
    println!("\n{summary}");

    // The keys from ledgers that served proofs: how many proofs, from which
    // height, and which ledgers hold them.
    let mut from_ledgers: HashMap<Fp, (usize, u32)> = HashMap::new();
    for r in &results {
        if let Outcome::Checked(c) = &r.outcome {
            for p in c.zkapp_proofs.iter().filter(|p| p.key_from_ledger) {
                let entry = from_ledgers.entry(p.vk_hash).or_insert((0, r.height));
                entry.0 += 1;
                entry.1 = entry.1.min(r.height);
            }
        }
    }
    let mut from_ledgers: Vec<_> = from_ledgers.into_iter().collect();
    from_ledgers.sort_by_key(|(_, (n, first))| (std::cmp::Reverse(*n), *first));
    for (hash, (n, first)) in &from_ledgers {
        let held_by: Vec<&str> = key_sources
            .get(hash)
            .into_iter()
            .flatten()
            .map(|&i| source_names[i].as_str())
            .collect();
        println!(
            "zkApp key {} from ledgers: {n} proofs, from {first}; in {}",
            hex(*hash),
            held_by.join(", ")
        );
    }

    let mut missing = Vec::new();
    if let Some(chain) = &args.chain {
        let verified: HashSet<(u32, &str)> = results
            .iter()
            .filter(|r| match &r.outcome {
                Outcome::Checked(c) => c.failures(r.height, &r.hash).is_empty(),
                _ => false,
            })
            .map(|r| (r.height, r.hash.as_str()))
            .collect();
        let mut listed = 0;
        for line in BufReader::new(File::open(chain)?).lines() {
            let line = line?;
            let mut fields = line.split_whitespace();
            let (Some(height), Some(hash)) = (fields.next(), fields.next()) else {
                continue;
            };
            let height: u32 = height.parse()?;
            if args.from.is_some_and(|from| height < from) || args.to.is_some_and(|to| height > to)
            {
                continue;
            }
            listed += 1;
            if !verified.contains(&(height, hash)) {
                missing.push(format!("{height} {hash}"));
            }
        }
        println!(
            "chain {}: {listed} blocks listed, {} verified, {} missing or failed",
            chain.display(),
            listed - missing.len(),
            missing.len()
        );
        for m in missing.iter().take(20) {
            println!("  {m}");
        }
    }

    println!(
        "runtime: {elapsed:.1?} wall on {} threads; verification time summed over blocks: \
         {:.0?} block proofs, {:.0?} transaction proofs, {:.0?} zkApp proofs",
        args.threads, summary.block_time, summary.tx_time, summary.zkapp_time
    );
    if summary.ok() && missing.is_empty() {
        println!("RESULT: every block of the chain verified");
        Ok(())
    } else {
        println!("RESULT: failures");
        std::process::exit(1)
    }
}
