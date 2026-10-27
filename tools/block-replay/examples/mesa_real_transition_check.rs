//! Build the block prover's input of a real Mesa mainnet (or, with `--network
//! devnet`, devnet) transition, a block and its parent, and check it against
//! the Mesa block step, all but its VRF:
//!
//!   MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs \
//!     cargo run --release -p mina-block-replay --example mesa_real_transition_check -- \
//!       --genesis mesa-mainnet.bin --chain blocks/chain.txt --blocks blocks \
//!       --height 548149 --out real-548149.bin
//!
//! The input is the one the block's producer proved, as far as it can be
//! rebuilt: the parent's state and OCaml's proof of it, from the precomputed
//! blocks; the block's state and snark transition; the ledger proof it emits,
//! the pending coinbase update and witness, from replaying the chain to the
//! parent and applying the block; and the stake proof, the winner's account
//! in the staking epoch ledger. The producer's private key is unknown, so the genesis winner's
//! stands in, and the block step then fails where it checks the key against
//! the winner's delegate (`eval_and_check_public_key`), and wherever the VRF
//! output it computes with that key differs from the block's: the threshold
//! and the new state's hash. Every other constraint must hold, the recursive
//! verification of OCaml's proof of the parent among them. The block step
//! lists every failing row (`constraint_failures`).
//!
//! `--check <input.bin> --key-one` instead checks an existing input with the
//! private key 1 as the stand-in: P4.6a's offline blocks, whose real key is
//! the genesis winner's, show which rows a wrong key alone breaks.
//!
//! The input is written to `--out` without the stand-in key, as the node
//! dumps a failed input (the private key 1).
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/vrf_lib/integrated.ml#L111-L122>

use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};
use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use clap::Parser;
use ledger::{
    proofs::{
        block::{BlockParams, ProtocolState},
        generate_block_proof,
        provers::BlockProver,
        transaction::ProofError,
        verifiers::{BlockVerifier, TransactionVerifier},
    },
    scan_state::{currency::Slot, transaction_logic::protocol_state::protocol_state_view},
    sparse_ledger::SparseLedger,
    staged_ledger::{
        diff::Diff,
        staged_ledger::{SkipVerification, StagedLedger},
    },
    verifier::Verifier,
    AccountId, BaseLedger, TokenId,
};
use mina_block_replay::read_block;
use mina_core::constants::constraint_constants;
use mina_node::{account::AccountSecretKey, transition_frontier::genesis::PrebuiltGenesisConfig};
use mina_p2p_messages::{
    bigint::BigInt,
    binprot::{BinProtRead, BinProtWrite},
    v2,
};

#[derive(Parser)]
struct Args {
    /// Prebuilt genesis config, as written by `ledger-tool`.
    #[arg(long)]
    genesis: Option<PathBuf>,
    /// `<height> <state hash>` per line, oldest first.
    #[arg(long)]
    chain: Option<PathBuf>,
    /// Directory with the precomputed blocks.
    #[arg(long)]
    blocks: Option<PathBuf>,
    /// The block whose transition to check, after the first Mesa block.
    #[arg(long, default_value_t = 548149)]
    height: u32,
    /// Where to write the input, in binprot.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Check this input instead of building one.
    #[arg(long)]
    check: Option<PathBuf>,
    /// With `--check`: the private key 1 stands in for the input's.
    #[arg(long)]
    key_one: bool,
    /// The network: its constraint constants and circuits.
    #[arg(long, default_value = "mainnet")]
    network: String,
}

/// A precomputed block's protocol state proof: the uri-safe base64 of its
/// binprot.
fn read_proof(dir: &Path, height: u32, hash: &str) -> anyhow::Result<v2::MinaBaseProofStableV2> {
    let path = dir.join(format!("{height}-{hash}.json.zst"));
    let mut bytes = Vec::new();
    zstd::Decoder::new(File::open(&path).with_context(|| path.display().to_string())?)?
        .read_to_end(&mut bytes)?;
    // Found in the bytes: the JSON need not be UTF-8 (raw sok digests), and
    // base64 has no quotes.
    const KEY: &[u8] = b"\"protocol_state_proof\":\"";
    let start = bytes
        .windows(KEY.len())
        .position(|w| w == KEY)
        .context("no protocol_state_proof")?
        + KEY.len();
    let len = bytes[start..]
        .iter()
        .position(|&b| b == b'"')
        .context("unterminated protocol_state_proof")?;
    let proof = URL_SAFE.decode(&bytes[start..start + len])?;
    Ok(v2::MinaBaseProofStableV2::binprot_read(
        &mut proof.as_slice(),
    )?)
}

fn build(args: &Args) -> anyhow::Result<v2::ProverExtendBlockchainInputStableV2> {
    let (Some(genesis), Some(chain), Some(blocks)) = (&args.genesis, &args.chain, &args.blocks)
    else {
        bail!("give --genesis, --chain and --blocks, or --check");
    };
    let constants = constraint_constants();
    let (mut masks, loaded) = PrebuiltGenesisConfig::load_from_bytes(&std::fs::read(genesis)?)?;
    let staking_ledger = masks.pop().context("no staking epoch ledger")?;
    let (_, genesis_state, genesis_hash) = loaded.protocol_states(
        loaded.genesis_total_currency.clone(),
        loaded.staking_epoch_total_currency.clone(),
        loaded.next_epoch_total_currency.clone(),
    )?;
    let mut staged_ledger =
        StagedLedger::create_exn(constants.clone(), masks.remove(0)).map_err(anyhow::Error::msg)?;

    let mut parent: Option<(u32, String, v2::MinaStateProtocolStateValueStableV2)> = None;
    let mut pred_state = genesis_state;
    let mut pred_hash = genesis_hash;
    for line in BufReader::new(File::open(chain)?).lines() {
        let line = line?;
        let mut fields = line.split_whitespace();
        let (Some(height), Some(hash)) = (fields.next(), fields.next()) else {
            continue;
        };
        let height: u32 = height.parse()?;
        let block = read_block(blocks, height, hash)?;
        let state = block.protocol_state;
        if state.previous_state_hash != pred_hash {
            bail!("block {height} {hash} does not extend {pred_hash}");
        }

        // The pending coinbase before the block, as the producer's witness.
        staged_ledger.hash();
        let pending_coinbases =
            v2::MinaBasePendingCoinbaseStableV2::from(staged_ledger.pending_coinbase_collection());

        let pred_view = protocol_state_view(&pred_state)?;
        let pred: ProtocolState = (&pred_state).try_into()?;
        let consensus = &state.body.consensus_state;
        let diff: Diff = (&block.staged_ledger_diff).try_into()?;
        let result = staged_ledger
            .apply(
                Some(SkipVerification::All),
                constants,
                Slot::from_u32(consensus.global_slot_since_genesis.as_u32()),
                diff,
                (),
                &Verifier,
                &pred_view,
                pred.hashes(),
                (&consensus.coinbase_receiver).try_into()?,
                consensus.supercharge_coinbase,
            )
            .map_err(|e| anyhow::anyhow!("block {height} {hash}: {e:?}"))?;
        if v2::MinaBaseStagedLedgerHashStableV1::from(&result.hash_after_applying)
            != state.body.blockchain_state.staged_ledger_hash
        {
            bail!("block {height} {hash}: staged ledger hash mismatch");
        }
        staged_ledger.commit_and_reparent_to_root();

        if height == args.height {
            let (parent_height, parent_hash, parent_state) =
                parent.context("the first Mesa block's parent is the genesis block")?;
            let proof = read_proof(blocks, parent_height, &parent_hash)?;

            let winner = &consensus.block_stake_winner;
            let id = AccountId::new((winner).try_into()?, TokenId::default());
            let index = staking_ledger
                .index_of_account(id.clone())
                .context("the winner is not in the staking epoch ledger")?;
            let stake_ledger = SparseLedger::of_ledger_subset_exn(staking_ledger.clone(), &[id]);
            if v2::LedgerHash::from_fp(stake_ledger.clone().merkle_root())
                != parent_state
                    .body
                    .consensus_state
                    .staking_epoch_data
                    .ledger
                    .hash
            {
                bail!("block {height}: not in the parent's staking epoch");
            }
            // The ledger proof the block emits, OCaml's, merged from the
            // snark work of earlier blocks.
            let ledger_proof = result
                .ledger_proof
                .as_ref()
                .map(|(proof, ..)| std::sync::Arc::new(proof.into()));

            return Ok(v2::ProverExtendBlockchainInputStableV2 {
                chain: v2::BlockchainSnarkBlockchainStableV2 {
                    state: parent_state,
                    proof: proof.into(),
                },
                next_state: state.clone(),
                block: v2::MinaStateSnarkTransitionValueStableV2 {
                    blockchain_state: state.body.blockchain_state.clone(),
                    consensus_transition: consensus
                        .curr_global_slot_since_hard_fork
                        .slot_number
                        .clone(),
                    pending_coinbase_update: (&result.pending_coinbase_update.1).into(),
                },
                ledger_proof,
                prover_state: v2::ConsensusStakeProofStableV2 {
                    delegator: index.into(),
                    delegator_pk: winner.clone(),
                    coinbase_receiver_pk: consensus.coinbase_receiver.clone(),
                    ledger: (&stake_ledger).into(),
                    producer_private_key: v2::SignatureLibPrivateKeyStableV1(BigInt::one()),
                    producer_public_key: consensus.block_creator.clone(),
                },
                pending_coinbase: v2::MinaBasePendingCoinbaseWitnessStableV2 {
                    pending_coinbases,
                    is_new_stack: result.pending_coinbase_update.0,
                },
            });
        }
        parent = Some((height, hash.to_owned(), state.clone()));
        pred_hash = state.try_hash()?;
        pred_state = state;
    }
    bail!("block {} is not in the chain", args.height);
}

/// Checks `input` against the block step, without proving, and prints what
/// fails.
fn check(input: &v2::ProverExtendBlockchainInputStableV2) -> anyhow::Result<()> {
    if std::env::var_os("MINA_CIRCUIT_BLOBS_BASE_DIR").is_none() {
        bail!("set MINA_CIRCUIT_BLOBS_BASE_DIR to the Mesa circuit blobs (tools/mesa-circuits)");
    }
    let provers = BlockProver::make(
        Some(BlockVerifier::make()),
        Some(TransactionVerifier::make()),
    )?;
    let result = generate_block_proof(BlockParams {
        input,
        block_step_prover: &provers.block_step_prover,
        block_wrap_prover: &provers.block_wrap_prover,
        tx_wrap_prover: &provers.tx_wrap_prover,
        only_verify_constraints: true,
        expected_step_proof: None,
        ocaml_wrap_witness: None,
    });
    let error = match result {
        Ok(_) => bail!("made a proof instead of checking constraints"),
        Err(error) => error,
    };
    match error.chain().find_map(|e| e.downcast_ref::<ProofError>()) {
        Some(ProofError::ConstraintsOk) => println!("block step: every constraint holds"),
        Some(ProofError::ConstraintsNotSatisfied(why)) => {
            println!("block step: not satisfied");
            for failure in why.split("; ") {
                println!("  {failure}");
            }
        }
        _ => bail!("{error:#}"),
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    mina_core::NetworkConfig::init(&args.network).map_err(anyhow::Error::msg)?;

    let mut input = match &args.check {
        Some(path) => v2::ProverExtendBlockchainInputStableV2::binprot_read(
            &mut std::fs::read(path)?.as_slice(),
        )?,
        None => {
            let input = build(&args)?;
            let mut bytes = Vec::new();
            input.binprot_write(&mut bytes)?;
            let out = args.out.as_ref().context("--out")?;
            std::fs::write(out, &bytes)?;
            println!(
                "block {} {}: input written to {}, {} bytes",
                args.height,
                v2::StateHash::from_fp(ProtocolState::try_from(&input.next_state)?.hashes().0),
                out.display(),
                bytes.len()
            );
            input
        }
    };
    if args.check.is_none() {
        input.prover_state.producer_private_key = AccountSecretKey::genesis_producer().into();
        println!("the genesis winner's private key stands in for the producer's");
    } else if args.key_one {
        input.prover_state.producer_private_key = v2::SignatureLibPrivateKeyStableV1(BigInt::one());
        println!("the private key 1 stands in for the input's");
    }
    check(&input)
}
