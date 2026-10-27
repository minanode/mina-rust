//! Write the block prover's input for the Mesa mainnet genesis block, the base
//! case of the blockchain step, as the node builds it to prove its genesis
//! block (`genesis_block_prover_input`): the input of the block step tests in
//! `crates/ledger/src/proofs/block.rs`.
//!
//! The genesis state comes from the prebuilt genesis config. Before writing,
//! its hash is checked against the parent of the first Mesa block, the first
//! line of `--chain`, read from `--blocks`, so the fixture is the real genesis
//! transition of Mesa mainnet: the negative-one state to the genesis state.
//!
//!   cargo run --release -p mina-block-replay --example mesa_genesis_block_input -- \
//!       --genesis mesa-mainnet.bin --chain blocks/chain.txt --blocks blocks \
//!       --out crates/ledger/src/proofs/data/mesa_mainnet_genesis_block_input.bin

use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
};

use anyhow::{bail, Context};
use clap::Parser;
use mina_block_replay::read_block;
use mina_node::transition_frontier::genesis::{genesis_block_prover_input, PrebuiltGenesisConfig};
use mina_p2p_messages::binprot::BinProtWrite;

#[derive(Parser)]
struct Args {
    /// Prebuilt genesis config, as written by `ledger-tool`.
    #[arg(long)]
    genesis: PathBuf,
    /// `<height> <state hash>` per line, oldest first: the first line is the
    /// first block after genesis.
    #[arg(long)]
    chain: PathBuf,
    /// Directory with the precomputed blocks.
    #[arg(long)]
    blocks: PathBuf,
    /// Where to write the input, in binprot.
    #[arg(long)]
    out: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    mina_core::NetworkConfig::init("mainnet").map_err(anyhow::Error::msg)?;

    let (_masks, loaded) = PrebuiltGenesisConfig::load_from_bytes(&std::fs::read(&args.genesis)?)?;
    let (negative_one, genesis, genesis_hash) = loaded.protocol_states(
        loaded.genesis_total_currency.clone(),
        loaded.staking_epoch_total_currency.clone(),
        loaded.next_epoch_total_currency.clone(),
    )?;

    let first = BufReader::new(File::open(&args.chain)?)
        .lines()
        .next()
        .context("empty chain")??;
    let mut fields = first.split_whitespace();
    let (Some(height), Some(hash)) = (fields.next(), fields.next()) else {
        bail!("bad chain line: {first}");
    };
    let block = read_block(&args.blocks, height.parse()?, hash)?;
    let parent = &block.protocol_state.previous_state_hash;
    if parent != &genesis_hash {
        bail!("genesis {genesis_hash} is not the parent of block {height} {hash}: {parent}");
    }

    let input = genesis_block_prover_input(
        &negative_one,
        &genesis,
        &loaded.genesis_producer_stake_proof,
    );
    let mut bytes = Vec::new();
    input.binprot_write(&mut bytes)?;
    std::fs::write(&args.out, &bytes)?;
    println!(
        "genesis {genesis_hash} (height {}, global slot {}), the parent of block {height} {hash}: \
         {} bytes to {}",
        genesis.body.consensus_state.blockchain_length.as_u32(),
        genesis
            .body
            .consensus_state
            .global_slot_since_genesis
            .as_u32(),
        bytes.len(),
        args.out.display()
    );
    Ok(())
}
