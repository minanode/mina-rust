//! Compare the precomputed blocks we write with the public bucket's, field by
//! field, and print a summary of the differences by kind.
//!
//! `round-trip` reads every block of the chain and writes it back:
//!
//!   cargo run --release -p mina-block-replay --example precomputed_blocks -- \
//!       round-trip --chain chain.txt --blocks blocks
//!
//! `node` applies the chain from genesis as the node does, and compares the
//! precomputed block the node's archive service would write for each block,
//! which needs the node's accounts accessed, accounts created and tokens used:
//!
//!   cargo run --release -p mina-block-replay --example precomputed_blocks -- \
//!       node --genesis mesa-mainnet.bin --chain chain.txt --blocks blocks \
//!       [--to 551260]
//!
//! The node writes the time it writes a block as `scheduled_time`, as OCaml
//! does, so that field is taken from the bucket's block.

use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Instant,
};

use clap::{Parser, Subcommand};
use mina_block_replay::{
    compare::{self, Replay, Summary},
    read_block_bytes,
};
use mina_node::ledger::write::LedgerHashes;
use mina_p2p_messages::v2::PrecomputedBlockData;
use rayon::prelude::*;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    mode: Mode,
    /// `<height> <state hash>` per line, oldest first.
    #[arg(long, global = true)]
    chain: Option<PathBuf>,
    /// Directory with the precomputed blocks.
    #[arg(long, global = true)]
    blocks: Option<PathBuf>,
    /// Stop after this height.
    #[arg(long, global = true)]
    to: Option<u32>,
    /// Print every difference, not only the summary.
    #[arg(long, global = true)]
    verbose: bool,
    /// Network whose signature prefixes, and so account update digests,
    /// apply.
    #[arg(long, global = true, default_value = "mainnet")]
    network: String,
}

#[derive(Subcommand)]
enum Mode {
    /// Read every block and write it back.
    RoundTrip {
        /// Threads.
        #[arg(long, default_value_t = 8)]
        jobs: usize,
    },
    /// Apply the chain from genesis and compare what the node writes.
    Node {
        /// Prebuilt genesis config, as written by `ledger-tool`.
        #[arg(long)]
        genesis: PathBuf,
    },
}

fn chain(path: &Path, to: Option<u32>) -> anyhow::Result<Vec<(u32, String)>> {
    let mut chain = Vec::new();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        let mut fields = line.split_whitespace();
        if let (Some(height), Some(hash)) = (fields.next(), fields.next()) {
            let height: u32 = height.parse()?;
            if to.is_some_and(|to| height > to) {
                break;
            }
            chain.push((height, hash.to_owned()));
        }
    }
    Ok(chain)
}

fn print_differences(name: &str, result: &anyhow::Result<Vec<compare::Difference>>) {
    match result {
        Ok(differences) => {
            for difference in differences {
                println!("{name} {difference}");
            }
        }
        Err(e) => println!("{name} error: {e:#}"),
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let chain = chain(args.chain.as_deref().expect("--chain"), args.to)?;
    let blocks = args.blocks.expect("--blocks");
    mina_core::NetworkConfig::init(&args.network).map_err(anyhow::Error::msg)?;
    let started = Instant::now();

    let summary = match args.mode {
        Mode::RoundTrip { jobs } => {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build()?;
            let done = Mutex::new(0usize);
            pool.install(|| {
                chain
                    .par_iter()
                    .fold(Summary::default, |mut summary, (height, hash)| {
                        let name = format!("{height}-{hash}");
                        let result = read_block_bytes(&blocks, *height, hash)
                            .and_then(|bytes| compare::round_trip(&bytes));
                        if args.verbose {
                            print_differences(&name, &result);
                        }
                        summary.add(&name, result);
                        let mut done = done.lock().unwrap();
                        *done += 1;
                        if done.is_multiple_of(1000) {
                            eprintln!("{} blocks in {:.0?}", *done, started.elapsed());
                        }
                        summary
                    })
                    .reduce(Summary::default, |mut a, b| {
                        a.merge(b);
                        a
                    })
            })
        }
        Mode::Node { genesis } => {
            let mut replay = Replay::from_genesis(&genesis)?;
            let mut summary = Summary::default();
            for (i, (height, hash)) in chain.iter().enumerate() {
                let name = format!("{height}-{hash}");
                let bytes = read_block_bytes(&blocks, *height, hash)?;
                let bucket = compare::read(&bytes)?;
                // A block that fails to apply ends the replay: the ones after
                // it can't be applied either.
                let ours = replay
                    .apply(&bucket.data)
                    .map_err(|e| anyhow::anyhow!("block {name}: {e:#}"))?;
                let result = PrecomputedBlockData::from(ours)
                    .to_ocaml_json(&LedgerHashes)
                    .map_err(anyhow::Error::from)
                    .and_then(|ours| compare::compare(&bytes, &ours));
                if args.verbose {
                    print_differences(&name, &result);
                }
                summary.add(&name, result);
                if (i + 1).is_multiple_of(250) {
                    eprintln!("{height}: {} blocks in {:.0?}", i + 1, started.elapsed());
                }
            }
            summary
        }
    };

    print!("{summary}");
    println!("in {:.0?}", started.elapsed());
    if !summary.is_clean() {
        std::process::exit(1);
    }
    Ok(())
}
