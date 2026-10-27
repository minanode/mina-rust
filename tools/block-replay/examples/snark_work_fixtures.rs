//! List the completed works of precomputed blocks, and write chosen ones as
//! binprot `TransactionSnarkWorkTStableV2` files (fee, proofs, prover), the
//! form the real-proof fixtures of `ledger::verifier::snark_work` take.
//!
//!   cargo run --release -p mina-block-replay --example snark_work_fixtures -- \
//!       --chain chain.txt --blocks blocks --list 553147
//!   cargo run --release -p mina-block-replay --example snark_work_fixtures -- \
//!       --chain chain.txt --blocks blocks --pick 553147:4 --out DIR
//!
//! Works are numbered as the block lists them, the first diff part's before
//! the second's. `--pick HEIGHT:WORK` writes `DIR/mainnet-HEIGHT-WORK.bin`.
//! Every listed or picked work's sok digest is checked against its fee and
//! prover.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
};

use anyhow::Context;
use clap::Parser;
use ledger::scan_state::scan_state::transaction_snark::{SokDigest, SokMessage, Statement};
use mina_block_replay::read_block;
use mina_p2p_messages::{
    binprot::BinProtWrite,
    v2::{
        LedgerProofProdStableV2, TransactionSnarkWorkTStableV2, TransactionSnarkWorkTStableV2Proofs,
    },
};

#[derive(Parser)]
struct Args {
    /// `<height> <state hash>` per line.
    #[arg(long)]
    chain: PathBuf,
    /// Directory with the precomputed blocks.
    #[arg(long)]
    blocks: PathBuf,
    /// List the completed works of the block at this height.
    #[arg(long)]
    list: Vec<u32>,
    /// Write this work, `HEIGHT:WORK`.
    #[arg(long)]
    pick: Vec<String>,
    /// Where `--pick` writes.
    #[arg(long, default_value = ".")]
    out: PathBuf,
}

fn proofs(work: &TransactionSnarkWorkTStableV2) -> Vec<&LedgerProofProdStableV2> {
    match &work.proofs {
        TransactionSnarkWorkTStableV2Proofs::One(p) => vec![p],
        TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => vec![p, q],
    }
}

fn describe(work: &TransactionSnarkWorkTStableV2) -> anyhow::Result<String> {
    let digest = SokMessage::create((&work.fee).into(), (&work.prover).try_into()?).digest();
    let mut kinds = Vec::new();
    for proof in proofs(work) {
        let statement = Statement::<SokDigest>::try_from(&proof.0.statement)?;
        anyhow::ensure!(statement.sok_digest == digest, "sok digest mismatch");
        let decoded = proof.0.proof.get()?;
        let branch = &decoded.0.statement.proof_state.deferred_values.branch_data;
        kinds.push(format!(
            "{:?}/d{}",
            branch.proofs_verified,
            branch.domain_log2.as_u8()
        ));
    }
    let mut bytes = Vec::new();
    work.binprot_write(&mut bytes)?;
    Ok(format!(
        "fee {} prover {} proofs [{}] {} bytes",
        ledger::scan_state::currency::Fee::from(&work.fee).as_u64(),
        work.prover,
        kinds.join(", "),
        bytes.len()
    ))
}

fn works(
    args: &Args,
    height: u32,
    hash: &str,
) -> anyhow::Result<Vec<TransactionSnarkWorkTStableV2>> {
    let block = read_block(&args.blocks, height, hash)?;
    let diff = &block.staged_ledger_diff.diff;
    Ok(diff
        .0
        .completed_works
        .iter()
        .chain(diff.1.iter().flat_map(|d| d.completed_works.iter()))
        .cloned()
        .collect())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    // Reading a block checks its zkApp account update digests, which depend
    // on the network.
    mina_core::NetworkConfig::init("mainnet").map_err(anyhow::Error::msg)?;
    let mut hashes = BTreeMap::new();
    for line in BufReader::new(File::open(&args.chain)?).lines() {
        let line = line?;
        let mut fields = line.split_whitespace();
        if let (Some(height), Some(hash)) = (fields.next(), fields.next()) {
            hashes.insert(height.parse::<u32>()?, hash.to_string());
        }
    }
    let hash_of = |height: u32| {
        hashes
            .get(&height)
            .with_context(|| format!("no block at {height} in the chain"))
    };

    for &height in &args.list {
        for (index, work) in works(&args, height, hash_of(height)?)?.iter().enumerate() {
            println!("{height}:{index} {}", describe(work)?);
        }
    }
    for pick in &args.pick {
        let (height, index) = pick
            .split_once(':')
            .with_context(|| format!("--pick {pick}: expected HEIGHT:WORK"))?;
        let (height, index): (u32, usize) = (height.parse()?, index.parse()?);
        let all = works(&args, height, hash_of(height)?)?;
        let work = all
            .get(index)
            .with_context(|| format!("block {height} has {} works", all.len()))?;
        let path = args.out.join(format!("mainnet-{height}-{index}.bin"));
        let mut bytes = Vec::new();
        work.binprot_write(&mut bytes)?;
        std::fs::write(&path, &bytes)?;
        println!("{height}:{index} {} -> {}", describe(work)?, path.display());
    }
    Ok(())
}
