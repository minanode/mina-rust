//! Reads every block of a replay chain, without applying any, and checks
//! that it parses, that every zkApp account update has the digest OCaml wrote
//! next to it (as `read_block` does), that each protocol state hashes to the
//! state hash in the block's name, and that each block's parent is the block
//! before it in the chain file. It is a quick check of blocks a replay can't
//! reach yet, for example when the chain doesn't start at genesis.
//!
//!   cargo run --release -p mina-block-replay --example check_chain -- \
//!       --network devnet --chain replay-devnet/chain.txt --blocks replay-devnet

use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use anyhow::{bail, Context};
use clap::Parser;
use mina_block_replay::read_block;
use mina_p2p_messages::v2::MinaBaseUserCommandStableV2;
use rayon::prelude::*;

#[derive(Parser)]
struct Args {
    /// Network whose signature prefixes the account update digests use.
    #[arg(long, default_value = "mainnet")]
    network: String,
    /// `<height> <state hash>` per line, oldest first.
    #[arg(long)]
    chain: PathBuf,
    /// Directory with the precomputed blocks.
    #[arg(long)]
    blocks: PathBuf,
}

/// What one block holds, and the hashes the chain check needs.
struct Read {
    height: u32,
    hash: String,
    parent: String,
    zkapp_commands: usize,
    signed_commands: usize,
    completed_works: usize,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    mina_core::NetworkConfig::init(&args.network).map_err(anyhow::Error::msg)?;
    let mut chain = Vec::new();
    for line in BufReader::new(File::open(&args.chain)?).lines() {
        let line = line?;
        let mut fields = line.split_whitespace();
        if let (Some(height), Some(hash)) = (fields.next(), fields.next()) {
            chain.push((height.parse::<u32>()?, hash.to_owned()));
        }
    }

    let started = Instant::now();
    let done = AtomicUsize::new(0);
    let reads = chain
        .par_iter()
        .map(|(height, hash)| {
            let block = read_block(&args.blocks, *height, hash)?;
            let state_hash = block.protocol_state.try_hash()?.to_string();
            if state_hash != *hash {
                bail!("block {height}: protocol state hashes to {state_hash}, not {hash}");
            }
            let diff = &block.staged_ledger_diff.diff;
            let parts = std::iter::once((&diff.0.commands, diff.0.completed_works.len())).chain(
                diff.1
                    .iter()
                    .map(|part| (&part.commands, part.completed_works.len())),
            );
            let (mut zkapp_commands, mut signed_commands, mut completed_works) = (0, 0, 0);
            for (commands, works) in parts {
                completed_works += works;
                for command in commands.iter() {
                    match command.data {
                        MinaBaseUserCommandStableV2::ZkappCommand(_) => zkapp_commands += 1,
                        MinaBaseUserCommandStableV2::SignedCommand(_) => signed_commands += 1,
                    }
                }
            }
            let count = done.fetch_add(1, Ordering::Relaxed) + 1;
            if count.is_multiple_of(2500) {
                println!("{count} blocks read in {:.0?}", started.elapsed());
            }
            Ok(Read {
                height: *height,
                hash: hash.clone(),
                parent: block.protocol_state.previous_state_hash.to_string(),
                zkapp_commands,
                signed_commands,
                completed_works,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    for pair in reads.windows(2) {
        let (before, block) = (&pair[0], &pair[1]);
        if block.height != before.height + 1 || block.parent != before.hash {
            bail!(
                "block {} {}: parent {} is not the line before it, {} {}",
                block.height,
                block.hash,
                block.parent,
                before.height,
                before.hash
            );
        }
    }
    let first = reads.first().context("an empty chain file")?;
    let last = reads.last().context("an empty chain file")?;
    println!(
        "read {} blocks, {} {} (parent {}) to {} {}, in {:.0?}: {} zkApp commands, every account update digest matching; {} signed commands; {} completed works",
        reads.len(),
        first.height,
        first.hash,
        first.parent,
        last.height,
        last.hash,
        started.elapsed(),
        reads.iter().map(|r| r.zkapp_commands).sum::<usize>(),
        reads.iter().map(|r| r.signed_commands).sum::<usize>(),
        reads.iter().map(|r| r.completed_works).sum::<usize>(),
    );
    Ok(())
}
