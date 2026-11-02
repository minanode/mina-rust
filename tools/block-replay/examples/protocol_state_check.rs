//! Offline protocol-state checking of every parent/child in a block directory.
//! Like OCaml's protocol_state_replay, this takes the staged hash and emitted
//! statement from recorded blocks. Full ledger replay independently checks both.
use std::{collections::BTreeMap, path::PathBuf, time::Instant};

use anyhow::{bail, Context};
use clap::Parser;
use mina_block_replay::{parse_block, read_file};
use mina_core::consensus::ConsensusConstants;
use mina_node::transition_frontier::protocol_state::check_protocol_state;
use mina_p2p_messages::v2;
use rayon::prelude::*;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "mainnet")]
    network: String,
    /// Directory of recorded blocks; may be repeated to supply missing parents.
    #[arg(long, required = true)]
    blocks: Vec<PathBuf>,
    /// Prebuilt genesis config, to check the first post-genesis block too.
    #[arg(long)]
    genesis: Option<PathBuf>,
    #[arg(long, default_value_t = 8)]
    threads: usize,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    mina_core::NetworkConfig::init(&args.network).map_err(anyhow::Error::msg)?;
    rayon::ThreadPoolBuilder::new()
        .num_threads(args.threads)
        .build_global()?;
    let started = Instant::now();
    let mut paths = Vec::new();
    for dir in &args.blocks {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if (name.ends_with(".json") || name.ends_with(".json.zst")) && name.contains("-3N") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    let states = paths
        .par_iter()
        .map(|path| -> anyhow::Result<_> {
            let block =
                parse_block(&read_file(path)?).with_context(|| path.display().to_string())?;
            let hash = v2::StateHash::from(block.protocol_state.try_hash()?);
            let body =
                ledger::staged_ledger::validate_block::block_body_hash(&block.staged_ledger_diff)
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?;
            Ok((hash, (block.protocol_state, body)))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
    let genesis = args
        .genesis
        .as_ref()
        .map(|path| -> anyhow::Result<_> {
            let (_, loaded) =
                mina_node::transition_frontier::genesis::PrebuiltGenesisConfig::load_from_bytes(
                    &std::fs::read(path)?,
                )?;
            let (_, state, hash) = loaded.protocol_states(
                loaded.genesis_total_currency.clone(),
                loaded.staking_epoch_total_currency.clone(),
                loaded.next_epoch_total_currency.clone(),
            )?;
            Ok((hash, state))
        })
        .transpose()?;
    let mut checked = 0;
    let mut rejected = 0;
    let mut missing_parent = 0;
    let mut seed_update = 0;
    let mut epoch_crossing = 0;
    let mut emitted = 0;
    for (hash, (state, body)) in &states {
        let parent = states
            .get(&state.previous_state_hash)
            .map(|(state, _)| state)
            .or_else(|| {
                genesis
                    .as_ref()
                    .filter(|(hash, _)| hash == &state.previous_state_hash)
                    .map(|(_, state)| state)
            });
        let Some(parent) = parent else {
            missing_parent += 1;
            eprintln!(
                "missing parent for height {} {hash}: {}",
                state.body.consensus_state.blockchain_length.as_u32(),
                state.previous_state_hash
            );
            continue;
        };
        let constants = ConsensusConstants::create(
            mina_core::constants::constraint_constants(),
            &parent.body.constants,
        );
        let statement = &state.body.blockchain_state.ledger_proof_statement;
        let proof = (statement != &parent.body.blockchain_state.ledger_proof_statement)
            .then_some(statement);
        let cs = &state.body.consensus_state;
        let slot = cs.curr_global_slot_since_hard_fork.slot_number.as_u32();
        seed_update +=
            usize::from(slot % constants.slots_per_epoch < constants.slots_per_epoch / 3 * 2);
        epoch_crossing += usize::from(cs.epoch_count != parent.body.consensus_state.epoch_count);
        emitted += usize::from(proof.is_some());
        checked += 1;
        if let Err(error) = check_protocol_state(
            &constants,
            parent,
            &state.previous_state_hash,
            state,
            &state.body.blockchain_state.staged_ledger_hash,
            body,
            proof,
        ) {
            rejected += 1;
            eprintln!(
                "rejected height {} {hash}: {error}",
                cs.blockchain_length.as_u32()
            );
        }
    }
    println!("files={} unique_blocks={} checked={} rejected={} missing_parent={} seed_update={} epoch_crossing={} emitted_statement={} elapsed_s={:.3}",
        paths.len(), states.len(), checked, rejected, missing_parent, seed_update, epoch_crossing, emitted, started.elapsed().as_secs_f64());
    if checked == 0 || rejected != 0 {
        bail!("protocol-state replay failed");
    }
    Ok(())
}
