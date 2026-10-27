//! Produce and prove a chain of Mesa mainnet (or devnet) blocks offline, with
//! the node's block producer code and the Rust prover, and check every proof
//! with the network's blockchain verifier, the live Mesa key:
//!
//!   MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs \
//!     cargo run --release -p mina-block-replay --example mesa_offline_chain -- \
//!       --genesis mesa-mainnet.bin --chain blocks/chain.txt --blocks blocks \
//!       --count 25 --out offline-chain
//!
//! With `--network devnet` it does the same from the Mesa devnet genesis
//! (`mesa-devnet.bin`), with the devnet circuits (`4.0.2devnet`, from
//! `MINA_PROFILE=devnet tools/mesa-circuits/generate.sh`) and the devnet key,
//! and also checks that each proof fails with the other network's key.
//!
//! A block of the real Mesa mainnet chain can't be proved again: the block
//! step checks the VRF in the circuit, with the producer's private key
//! (`eval_and_check_public_key`), so the prover must hold the key of the
//! block's stake winner's delegate. The one key in the Mesa staking ledgers
//! whose private key is public, the genesis winner's, has 0.000001 MINA, about
//! 7.7e-16 of the stake, and so never wins a slot.
//!
//! This chain forks from the Mesa mainnet genesis as little as that allows:
//!
//! - the genesis is Mesa mainnet's, but in its staking epoch ledger the genesis
//!   winner's account (index 0) holds three times the total currency, so that
//!   the genesis winner wins about two slots in three. The staking epoch
//!   ledger's hash and total currency change, and with them the genesis
//!   state hash; the genesis ledger, the next epoch ledger, the seeds and the
//!   fork constants are Mesa mainnet's. The genesis block is the base case,
//!   proved as the node proves its genesis block
//!   (`genesis_block_prover_input`);
//! - block `k` carries the user commands of the `k`-th real Mesa block (from
//!   `--chain` and `--blocks`), applied to the genesis ledger in the same
//!   order, at the first slot from the real block's on that the genesis
//!   winner wins. Its coinbase and fees go to the genesis winner;
//! - each block is built as the node's block producer builds it: the staged
//!   ledger diff as the ledger service creates it
//!   (`staged_ledger_diff_create`), then `build_unproven_block` and
//!   `block_prover_input`, then proved, step and wrap, on the previous block's
//!   proof. No snark work is bought, so no ledger proof is emitted while the
//!   scan state has room, as in the first real Mesa blocks.
//!
//! The proofs verify with the live key, which does not depend on the genesis
//! ledgers: it fixes the circuit and the fork constants. OCaml nodes would
//! reject the chain at its genesis state hash
//! (`validate_genesis_protocol_state`), not at its proofs.
//!
//! Written to `--out`: `inputs/<height>.bin`, each block's prover input in
//! binprot (the private key in it is the genesis winner's, which is public),
//! as `prove_block --input` reads it; `blocks.json`, the genesis and every
//! block with its proof as OCaml's JSON has them (`prove_block --verify` reads
//! it); and `chain.txt`, a line per block.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L173-L436>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/vrf_lib/integrated.ml#L111-L122>

use std::{
    collections::VecDeque,
    fs::File,
    io::{BufRead, BufReader, Write as _},
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

use anyhow::{bail, Context};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use clap::Parser;
use ledger::{
    proofs::{
        block::{BlockParams, ProtocolState},
        generate_block_proof,
        provers::BlockProver,
        verification::verify_block_proof,
        verifiers::{BlockVerifier, TransactionVerifier},
    },
    scan_state::{
        currency::Balance,
        transaction_logic::{protocol_state::protocol_state_view, valid, UserCommand, WithStatus},
    },
    sparse_ledger::SparseLedger,
    staged_ledger::{diff::Diff, staged_ledger::StagedLedger, validate_block::block_body_hash},
    AccountIndex, Address,
};
use mina_block_replay::read_block;
use mina_core::{
    block::{AppliedBlock, ArcBlockWithHash, BlockWithHash},
    consensus::ConsensusConstants,
    constants::{constraint_constants, PROTOCOL_VERSION},
};
use mina_curves::pasta::Fp;
use mina_node::{
    account::AccountSecretKey,
    block_producer::{
        block_prover_input, build_unproven_block, vrf_evaluator::VrfWonSlotWithHash,
        BlockProducerConfig, BlockProducerWonSlot,
    },
    transition_frontier::genesis::{genesis_block_prover_input, PrebuiltGenesisConfig},
};
use mina_p2p_messages::{binprot::BinProtWrite, v2};
use mina_vrf::{VrfEvaluationInput, VrfEvaluationOutput};
use serde_json::Value;

#[derive(Parser)]
struct Args {
    /// Prebuilt genesis config, as written by `ledger-tool`.
    #[arg(long)]
    genesis: PathBuf,
    /// `<height> <state hash>` per line, oldest first: the real chain whose
    /// user commands the blocks carry.
    #[arg(long)]
    chain: PathBuf,
    /// Directory with the precomputed blocks.
    #[arg(long)]
    blocks: PathBuf,
    /// How many blocks to produce after the genesis block.
    #[arg(long, default_value_t = 25)]
    count: usize,
    /// Where to write the inputs, the blocks and `chain.txt`.
    #[arg(long)]
    out: PathBuf,
    /// Build the blocks and their inputs without proving them, each on a
    /// dummy proof (the inputs are then not provable): a quick check of the
    /// rest.
    #[arg(long)]
    no_prove: bool,
    /// The network: its constraint constants, circuits and keys.
    #[arg(long, default_value = "mainnet")]
    network: String,
}

/// The genesis winner's balance in the staking epoch ledger, in multiples of
/// the ledger's total currency: with three, it holds three quarters of the
/// stake and wins a slot with probability `1 - (1 - 3/4)^(3/4)`, about 0.65.
const STAKE_MULTIPLE: u64 = 3;

/// `Pickles.Proof.to_yojson`, as in `prove_block`.
fn proof_to_yojson(proof: &v2::MinaBaseProofStableV2) -> Value {
    let sexp = rsexp::SexpOf::sexp_of(&proof.0);
    Value::String(STANDARD.encode(sexp.to_bytes_mach()))
}

/// Whether the network's blockchain verifier accepts `proof` for `state`, as
/// the node checks a block it receives and its own block proof.
fn verifies(
    state: &v2::MinaStateProtocolStateValueStableV2,
    proof: &v2::MinaBaseProofStableV2,
    verifier: &ledger::proofs::VerifierIndex<mina_curves::pasta::Fq>,
) -> bool {
    verify_block_proof(state, proof, verifier)
}

struct Prover {
    provers: Option<BlockProver>,
    verifier: BlockVerifier,
    /// The other network's blockchain verifier, which must reject every
    /// proof.
    other: (
        &'static str,
        ledger::proofs::VerifierIndex<mina_curves::pasta::Fq>,
    ),
}

impl Prover {
    /// Proves `input`, step and wrap, and checks the proof with the live key
    /// for the new state, and that it fails for the previous state, for the
    /// new state with another VRF output and with the other network's key.
    /// Returns the proof and the proving time.
    fn prove(
        &self,
        input: &v2::ProverExtendBlockchainInputStableV2,
    ) -> anyhow::Result<(v2::MinaBaseProofStableV2, f64)> {
        let Some(provers) = &self.provers else {
            return Ok((
                ledger::dummy::dummy_blockchain_proof().as_ref().clone(),
                0.0,
            ));
        };
        let started = Instant::now();
        let proof = generate_block_proof(BlockParams {
            input,
            block_step_prover: &provers.block_step_prover,
            block_wrap_prover: &provers.block_wrap_prover,
            tx_wrap_prover: &provers.tx_wrap_prover,
            only_verify_constraints: false,
            expected_step_proof: None,
            ocaml_wrap_witness: None,
        })?;
        let elapsed = started.elapsed().as_secs_f64();
        let proof = v2::MinaBaseProofStableV2((&proof).into());

        if !verifies(&input.next_state, &proof, &self.verifier) {
            bail!("the proof does not verify with the blockchain key");
        }
        if verifies(&input.next_state, &proof, &self.other.1) {
            bail!(
                "the proof verifies with the {} blockchain key too",
                self.other.0
            );
        }
        if verifies(&input.chain.state, &proof, &self.verifier) {
            bail!("the proof verifies for the previous state too");
        }
        let mut tampered = input.next_state.clone();
        let vrf = &mut tampered.body.consensus_state.last_vrf_output;
        vrf.0 .0[0] ^= 1;
        if verifies(&tampered, &proof, &self.verifier) {
            bail!("the proof verifies for a state with another VRF output");
        }
        Ok((proof, elapsed))
    }
}

/// The genesis config with the genesis winner's account in the staking epoch
/// ledger holding `STAKE_MULTIPLE` times the ledger's total currency, and the
/// total currency raised to match.
fn with_genesis_winner_stake(
    mut loaded: mina_node::transition_frontier::genesis::GenesisConfigLoaded,
) -> anyhow::Result<(
    mina_node::transition_frontier::genesis::GenesisConfigLoaded,
    u64,
)> {
    let mut ledger = SparseLedger::try_from(&loaded.genesis_producer_stake_proof)?;
    let real_root = ledger.merkle_root();
    if real_root != loaded.staking_epoch_ledger_hash.to_field::<Fp>()? {
        bail!("the genesis stake proof is not of the staking epoch ledger");
    }
    let addr = Address::from_index(
        AccountIndex(0),
        constraint_constants().ledger_depth as usize,
    );
    let mut account = ledger.get_exn(&addr);
    if account.public_key != AccountSecretKey::genesis_producer().public_key_compressed() {
        bail!("account 0 of the staking epoch ledger is not the genesis winner's");
    }
    let total = loaded.staking_epoch_total_currency.as_u64();
    let stake = total
        .checked_mul(STAKE_MULTIPLE)
        .context("stake overflows")?;
    let new_total = total
        .checked_sub(account.balance.as_u64())
        .and_then(|t| t.checked_add(stake))
        .context("total currency overflows")?;
    account.balance = Balance::from_u64(stake);
    ledger.set_exn(addr, account);

    loaded.staking_epoch_ledger_hash = v2::LedgerHash::from_fp(ledger.merkle_root());
    loaded.staking_epoch_total_currency = v2::CurrencyAmountStableV1(new_total.into());
    loaded.genesis_producer_stake_proof = (&ledger).into();
    Ok((loaded, stake))
}

/// The first slot from `from` on, in the parent's epoch, that the genesis
/// winner wins with its stake in the staking epoch ledger, as the node's VRF
/// evaluator finds it.
fn next_won_slot(
    parent: &ArcBlockWithHash,
    from: u32,
    stake: u64,
) -> anyhow::Result<(mina_vrf::VrfWonSlot, u32)> {
    let staking = &parent.consensus_state().staking_epoch_data;
    let slots_per_epoch = parent.constants().slots_per_epoch.as_u32();
    let epoch_end = (parent.global_slot() / slots_per_epoch + 1) * slots_per_epoch;
    let key = AccountSecretKey::genesis_producer();
    for slot in from..epoch_end {
        let output = mina_vrf::evaluate_vrf(VrfEvaluationInput {
            producer_key: key.clone().into(),
            global_slot: slot,
            epoch_seed: staking.seed.clone(),
            account_pub_key: key.public_key(),
            delegator_index: AccountIndex(0),
            delegated_stake: stake.into(),
            total_currency: staking.ledger.total_currency.as_u64().into(),
        })?;
        if let VrfEvaluationOutput::SlotWon(won) = output {
            return Ok((won, slot - from));
        }
    }
    bail!("no slot won from {from} to the end of the epoch");
}

fn block_json(block: &ArcBlockWithHash) -> anyhow::Result<Value> {
    let header = block.header();
    Ok(serde_json::json!({
        "state": serde_json::to_value(&header.protocol_state)?,
        "proof": proof_to_yojson(&header.protocol_state_proof),
    }))
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    mina_core::NetworkConfig::init(&args.network).map_err(anyhow::Error::msg)?;
    mina_core::set_work_dir(std::env::temp_dir().join("mina-offline-chain"));
    let other = match args.network.as_str() {
        "devnet" => "mainnet",
        _ => "devnet",
    };
    let constants = constraint_constants();
    std::fs::create_dir_all(args.out.join("inputs"))?;

    let started = Instant::now();
    let (mut masks, loaded) =
        PrebuiltGenesisConfig::load_from_bytes(&std::fs::read(&args.genesis)?)?;
    let real_staking_hash = loaded.staking_epoch_ledger_hash.clone();
    let (loaded, stake) = with_genesis_winner_stake(loaded)?;
    let (negative_one, genesis_state, genesis_hash) = loaded.protocol_states(
        loaded.genesis_total_currency.clone(),
        loaded.staking_epoch_total_currency.clone(),
        loaded.next_epoch_total_currency.clone(),
    )?;
    println!(
        "{} genesis {genesis_hash} loaded in {:.1?}: staking epoch ledger {} (Mesa's is {}), \
         the genesis winner's stake {stake} of {}",
        args.network,
        started.elapsed(),
        loaded.staking_epoch_ledger_hash,
        real_staking_hash,
        loaded.staking_epoch_total_currency.as_u64(),
    );

    let prover = Prover {
        provers: (!args.no_prove)
            .then(|| {
                if std::env::var_os("MINA_CIRCUIT_BLOBS_BASE_DIR").is_none() {
                    eprintln!("MINA_CIRCUIT_BLOBS_BASE_DIR is not set: the blobs may not be found");
                }
                let started = Instant::now();
                let provers = BlockProver::make(
                    Some(BlockVerifier::make()),
                    Some(TransactionVerifier::make()),
                )?;
                println!("provers loaded in {:.1?}", started.elapsed());
                Ok::<_, anyhow::Error>(provers)
            })
            .transpose()?,
        verifier: BlockVerifier::make(),
        other: (
            other,
            ledger::proofs::verifiers::block_verifier_index_of(other)?,
        ),
    };

    // The genesis block, the base case.
    let input = genesis_block_prover_input(
        &negative_one,
        &genesis_state,
        &loaded.genesis_producer_stake_proof,
    );
    let mut bytes = Vec::new();
    input.binprot_write(&mut bytes)?;
    let height = genesis_state
        .body
        .consensus_state
        .blockchain_length
        .as_u32();
    std::fs::write(args.out.join(format!("inputs/{height}.bin")), &bytes)?;
    let (proof, elapsed) = prover.prove(&input)?;
    println!("genesis {genesis_hash}, height {height}: proved and verified in {elapsed:.1} s");
    let genesis = BlockWithHash::try_new(Arc::new(v2::MinaBlockBlockStableV2 {
        header: v2::MinaBlockHeaderStableV2 {
            protocol_state: genesis_state.clone(),
            protocol_state_proof: Arc::new(proof),
            delta_block_chain_proof: (genesis_hash.clone(), std::iter::empty().collect()),
            current_protocol_version: PROTOCOL_VERSION,
            proposed_protocol_version_opt: None,
        },
        body: v2::StagedLedgerDiffBodyStableV1 {
            staged_ledger_diff:
                (&ledger::staged_ledger::diff::with_valid_signatures_and_proofs::Diff::empty())
                    .into(),
        },
    }))?;
    let genesis_timestamp = genesis.genesis_timestamp();

    let consensus_constants = ConsensusConstants::create(constants, &genesis_state.body.constants);
    let config = BlockProducerConfig::new(AccountSecretKey::genesis_producer().public_key().into());
    let coinbase_receiver = AccountSecretKey::genesis_producer().public_key_compressed();

    let mut staged_ledger =
        StagedLedger::create_exn(constants.clone(), masks.remove(0)).map_err(anyhow::Error::msg)?;
    // The genesis state's staged ledger hash has zero aux hashes
    // (`Staged_ledger_hash.genesis`), not the empty scan state's: compare the
    // ledger and the pending coinbase.
    let found = v2::MinaBaseStagedLedgerHashStableV1::from(&staged_ledger.hash());
    let expected = &genesis_state.body.blockchain_state.staged_ledger_hash;
    if found.non_snark.ledger_hash != expected.non_snark.ledger_hash
        || found.pending_coinbase_hash != expected.pending_coinbase_hash
    {
        bail!("the genesis staged ledger is not the genesis state's:\n  {found:?}\n  {expected:?}");
    }

    let mut blocks_json = vec![block_json(&genesis)?];
    let mut chain = vec![AppliedBlock {
        block: genesis,
        just_emitted_a_proof: false,
    }];
    let mut summary = File::create(args.out.join("chain.txt"))?;
    writeln!(
        summary,
        "# height state_hash slot real_height real_slot commands applied failed skipped coinbase prove_s"
    )?;

    let mut real_chain = BufReader::new(File::open(&args.chain)?).lines();
    let mut pending: VecDeque<valid::UserCommand> = VecDeque::new();
    for _ in 0..args.count {
        let line = real_chain
            .next()
            .context("the real chain is shorter than --count")??;
        let mut fields = line.split_whitespace();
        let (Some(real_height), Some(real_hash)) = (fields.next(), fields.next()) else {
            bail!("bad chain line: {line}");
        };
        let real = read_block(&args.blocks, real_height.parse()?, real_hash)?;
        let real_slot = real
            .protocol_state
            .body
            .consensus_state
            .curr_global_slot_since_hard_fork
            .slot_number
            .as_u32();
        let real_diff: Diff = (&real.staged_ledger_diff).try_into()?;
        pending.extend(
            real_diff
                .commands()
                .into_iter()
                .map(|WithStatus { data, .. }: WithStatus<UserCommand>| data.to_valid_unsafe()),
        );

        let parent = &chain.last().context("empty chain")?.block;
        let from = real_slot.max(parent.global_slot() + 1);
        let (won, lost) = next_won_slot(parent, from, stake)?;
        let won_slot = BlockProducerWonSlot::from_vrf_won_slot(
            &VrfWonSlotWithHash::new(won, loaded.staking_epoch_ledger_hash.clone()),
            genesis_timestamp,
        );
        let global_slot_since_genesis =
            won_slot.global_slot_since_genesis(parent.global_slot_diff());

        // The staged ledger diff, as the ledger service creates it for the
        // block producer (`staged_ledger_diff_create`): no snark work, and
        // `supercharge_coinbase` as the node passes it.
        let supercharge_coinbase = true;
        staged_ledger.hash();
        let pending_coinbase_witness =
            v2::MinaBasePendingCoinbaseStableV2::from(staged_ledger.pending_coinbase_collection());
        let parent_state = &parent.header().protocol_state;
        let view = protocol_state_view(parent_state)?;
        let offered = pending.len();
        let (pre_diff, invalid) = staged_ledger
            .create_diff(
                constants,
                (&global_slot_since_genesis).into(),
                Some(true),
                coinbase_receiver.clone(),
                (),
                &view,
                pending.drain(..).collect(),
                |_| None,
                supercharge_coinbase,
            )
            .map_err(|e| anyhow::anyhow!("create_diff: {e:?}"))?;
        let diff: v2::StagedLedgerDiffDiffStableV2 = (&pre_diff).into();
        let commands = pre_diff.commands();
        let applied = commands.iter().filter(|c| c.status.is_applied()).count();
        let coinbase = match &pre_diff.diff.0.coinbase {
            ledger::staged_ledger::diff::AtMostTwo::Zero => 0,
            ledger::staged_ledger::diff::AtMostTwo::One(_) => 1,
            ledger::staged_ledger::diff::AtMostTwo::Two(_) => 2,
        };
        let parent_proto: ProtocolState = parent_state.try_into()?;
        let (parent_hash, parent_body_hash) = parent_proto.hashes();
        let res = staged_ledger
            .apply_diff_unchecked(
                constants,
                (&global_slot_since_genesis).into(),
                pre_diff,
                (),
                &view,
                (parent_hash, parent_body_hash),
                coinbase_receiver.clone(),
                supercharge_coinbase,
            )
            .map_err(|e| anyhow::anyhow!("apply_diff_unchecked: {e:?}"))?;
        staged_ledger.commit_and_reparent_to_root();
        let diff_hash = block_body_hash(&diff).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let emitted_ledger_proof: Option<Arc<v2::LedgerProofProdStableV2>> = res
            .ledger_proof
            .map(|(proof, ..)| Arc::new((&proof).into()));
        let staged_ledger_hash = (&res.hash_after_applying).into();
        let pending_coinbase_update = (&res.pending_coinbase_update.1).into();
        let pending_coinbase_witness = v2::MinaBasePendingCoinbaseWitnessStableV2 {
            pending_coinbases: pending_coinbase_witness,
            is_new_stack: res.pending_coinbase_update.0,
        };

        let (block, block_hash) = build_unproven_block(
            &consensus_constants,
            &config,
            &won_slot,
            &chain,
            &diff,
            &diff_hash,
            &staged_ledger_hash,
            emitted_ledger_proof.as_deref(),
            &loaded.genesis_producer_stake_proof,
        )
        .map_err(anyhow::Error::msg)?;
        let input = block_prover_input(
            &won_slot,
            &chain,
            emitted_ledger_proof.clone(),
            &pending_coinbase_update,
            &pending_coinbase_witness,
            &loaded.genesis_producer_stake_proof,
            &block,
        )
        .context("block_prover_input")?;
        let height = block
            .protocol_state
            .body
            .consensus_state
            .blockchain_length
            .as_u32();
        let mut bytes = Vec::new();
        input.binprot_write(&mut bytes)?;
        std::fs::write(args.out.join(format!("inputs/{height}.bin")), &bytes)?;

        let (proof, elapsed) = prover
            .prove(&input)
            .with_context(|| format!("block {height} {block_hash}"))?;
        let block = block.with_hash_and_proof(block_hash.clone(), Arc::new(proof));
        let slot = won_slot.global_slot();
        println!(
            "block {height} {block_hash}: slot {slot} ({lost} lost from {from}), real block \
             {real_height} at slot {real_slot}; {} of {offered} commands ({applied} applied), \
             {} invalid, coinbase {coinbase}, ledger proof {}; {}",
            commands.len(),
            invalid.len(),
            emitted_ledger_proof.is_some(),
            if args.no_prove {
                "not proved".to_owned()
            } else {
                format!("proved and verified in {elapsed:.1} s")
            },
        );
        writeln!(
            summary,
            "{height} {block_hash} {slot} {real_height} {real_slot} {} {applied} {} {} {coinbase} {elapsed:.2}",
            commands.len(),
            commands.len() - applied,
            invalid.len(),
        )?;
        blocks_json.push(block_json(&block)?);
        chain.push(AppliedBlock {
            block,
            just_emitted_a_proof: emitted_ledger_proof.is_some(),
        });
    }

    let blocks = serde_json::to_string(&Value::Array(blocks_json))?;
    std::fs::write(args.out.join("blocks.json"), format!("{blocks}\n"))?;
    println!(
        "{} blocks after genesis {genesis_hash} in {:.0?}, written to {}",
        args.count,
        started.elapsed(),
        args.out.display()
    );
    Ok(())
}
