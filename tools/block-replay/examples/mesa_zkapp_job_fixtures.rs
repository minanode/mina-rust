//! Replay Mesa mainnet from genesis, or devnet from a staged ledger dump, as
//! `mina-block-replay` does, and write the first zkApp command base jobs of
//! the wanted shapes that the chain's blocks add to the scan state, for the
//! zkApp prover tests in `crates/ledger/src/proofs/zkapp.rs`.
//!
//! A shape is the authorization of each account update in the order the
//! zkApp segments take them, fee payer first: `S` signature, `N` none, `P`
//! proof. `--want signed=SSN` writes the first command with a fee payer, one
//! signed and one unauthorised account update to
//! `mesa_<network>_zkapp_signed_base_job.bin`; `--want signed=SSN@577969`
//! takes the first one in block 577969.
//!
//! Each fixture is the `(statement, transaction witness)` pair in binprot
//! that a snark worker gets for `Transition` work, as in
//! `mesa_base_job_fixtures`. The jobs are OCaml's own: the replay checks each
//! block's staged ledger hash, whose scan state part hashes the binprot of
//! every base job, sparse ledgers included (`ScanState::hash`). The witness
//! takes the protocol state body of the job's state hash, the block before
//! the one that added it, as OCaml's `single_spec_of_job` does.
//!
//! The replay goes on until a block's completed work proves each written job,
//! and writes that work's sok message and OCaml's proof of the job to
//! `mesa_<network>_zkapp_<name>_ocaml_work.bin`: the statement a Rust proof of
//! the job must have, and an OCaml proof of it.
//!
//!   cargo run --release -p mina-block-replay --example mesa_zkapp_job_fixtures -- \
//!       --genesis mesa-mainnet.bin --chain blocks/chain.txt --blocks blocks \
//!       --out crates/ledger/src/proofs/data --want proved=SSSSPS --want signed=SSN
//!
//!   cargo run --release -p mina-block-replay --example mesa_zkapp_job_fixtures -- \
//!       --network devnet --dump root.binprot --root 577343 --chain chain.txt \
//!       --blocks blocks --out crates/ledger/src/proofs/data --want devnet_signed=SSN@577969

use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
    time::Instant,
};

use anyhow::{bail, Context};
use clap::Parser;
use ledger::{
    proofs::block::ProtocolState,
    scan_state::{
        currency::Slot,
        scan_state::transaction_snark::{Statement, TransactionWithWitness},
        transaction_logic::{
            protocol_state::protocol_state_view, zkapp_command::Control, Transaction,
            TransactionStatus, UserCommand,
        },
    },
    staged_ledger::{
        diff::Diff,
        staged_ledger::{SkipVerification, StagedLedger},
    },
    verifier::Verifier,
    AccountId,
};
use mina_block_replay::{dump::StagedLedgerDump, read_block};
use mina_curves::pasta::Fp;
use mina_node::transition_frontier::genesis::PrebuiltGenesisConfig;
use mina_p2p_messages::{
    binprot::BinProtWrite,
    v2::{self, MinaBaseStagedLedgerHashStableV1, MinaStateProtocolStateValueStableV2},
};

#[derive(Parser)]
struct Args {
    /// `mainnet` or `devnet`.
    #[arg(long, default_value = "mainnet")]
    network: String,
    /// Prebuilt genesis config, as written by `ledger-tool`.
    #[arg(long, conflicts_with = "dump")]
    genesis: Option<PathBuf>,
    /// A staged ledger dump of the block `--root`, to start after it.
    #[arg(long, requires = "root")]
    dump: Option<PathBuf>,
    /// The height of the dump's block.
    #[arg(long)]
    root: Option<u32>,
    /// `<height> <state hash>` per line, oldest first.
    #[arg(long)]
    chain: PathBuf,
    /// Directory with the precomputed blocks.
    #[arg(long)]
    blocks: PathBuf,
    /// Where to write the fixtures.
    #[arg(long)]
    out: PathBuf,
    /// Stop after this height.
    #[arg(long)]
    to: Option<u32>,
    /// `NAME=SHAPE` or `NAME=SHAPE@HEIGHT`, as in the module doc.
    #[arg(long)]
    want: Vec<String>,
    /// Report every proof-authorised account update whose zkApp's
    /// verification key has a wrap domain other than 2^13 (a zkApp whose
    /// proofs verify proofs), and count the keys by domain.
    #[arg(long)]
    report_domains: bool,
    /// Save every zkApp base job added at or after this height.
    #[arg(long)]
    capture_from: Option<u32>,
}

struct Wanted {
    name: String,
    shape: String,
    height: Option<u32>,
    /// Once written: the job's statement, until a block's completed work
    /// proves it.
    written: Option<Statement<()>>,
    proved_in: Option<u32>,
}

/// The authorization of each account update, fee payer first, in the order
/// `Zkapp_command.group_by_zkapp_command_rev` takes them.
fn shape(job: &TransactionWithWitness) -> Option<String> {
    let Transaction::Command(UserCommand::ZkAppCommand(command)) =
        &job.transaction_with_status.data
    else {
        return None;
    };
    let mut shape = String::from("S");
    command.account_updates.fold((), |(), update| {
        shape.push(match update.authorization {
            Control::Signature(_) => 'S',
            Control::NoneGiven => 'N',
            Control::Proof(_) => 'P',
        });
    });
    Some(shape)
}

/// With `--report-domains`: each proof-authorised account update of a zkApp
/// command job, by the max proofs verified and wrap domain of its account's
/// verification key, read from the job's first pass sparse ledger.
fn report_domains(job: &TransactionWithWitness, height: u32, domains: &mut BTreeMap<String, u64>) {
    let Transaction::Command(UserCommand::ZkAppCommand(command)) =
        &job.transaction_with_status.data
    else {
        return;
    };
    command.account_updates.fold((), |(), update| {
        if !matches!(update.authorization, Control::Proof(_)) {
            return;
        }
        let id = AccountId::create(update.body.public_key.clone(), update.body.token_id.clone());
        let account = job.first_pass_ledger_witness.get_account(&id);
        let key = match account
            .zkapp
            .as_ref()
            .and_then(|z| z.verification_key.as_ref())
        {
            Some(vk) => {
                let vk = vk.vk();
                format!(
                    "max {:?}, domain {:?}",
                    vk.max_proofs_verified, vk.actual_wrap_domain_size
                )
            }
            None => "no key in the first pass ledger".to_owned(),
        };
        if !key.ends_with("domain N0") {
            println!(
                "domain: block {height}, {} ({}): {key}",
                id.public_key.into_address(),
                v2::TokenIdKeyHash::from(id.token_id.clone())
            );
        }
        *domains.entry(key).or_default() += 1;
    });
}

/// The `Transition` work a snark worker gets for a base job: its statement,
/// and its transaction witness with the protocol state body of the job's state
/// hash, here the parent of the block that added it.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transaction_snark_scan_state/transaction_snark_scan_state.ml#L1333-L1362>
fn transition_spec(
    job: &TransactionWithWitness,
    protocol_state_body: &v2::MinaStateProtocolStateBodyValueStableV2,
) -> (
    v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement,
    v2::TransactionWitnessStableV2,
) {
    let base: v2::TransactionSnarkScanStateTransactionWithWitnessStableV2 = job.into();
    let witness = v2::TransactionWitnessStableV2 {
        transaction: base.transaction_with_status.data,
        first_pass_ledger: base.first_pass_ledger_witness,
        second_pass_ledger: base.second_pass_ledger_witness,
        protocol_state_body: protocol_state_body.clone(),
        init_stack: base.init_stack,
        status: base.transaction_with_status.status,
        block_global_slot: base.block_global_slot,
    };
    (base.statement.0, witness)
}

/// The staged ledger to start from, the block it belongs to and its hash.
fn start(
    args: &Args,
) -> anyhow::Result<(
    StagedLedger,
    MinaStateProtocolStateValueStableV2,
    v2::StateHash,
)> {
    let constraint_constants = mina_core::constants::constraint_constants();
    if let Some(genesis) = &args.genesis {
        let (mut masks, loaded) = PrebuiltGenesisConfig::load_from_bytes(&std::fs::read(genesis)?)?;
        let (_, state, hash) = loaded.protocol_states(
            loaded.genesis_total_currency.clone(),
            loaded.staking_epoch_total_currency.clone(),
            loaded.next_epoch_total_currency.clone(),
        )?;
        let staged_ledger = StagedLedger::create_exn(constraint_constants.clone(), masks.remove(0))
            .map_err(anyhow::Error::msg)?;
        return Ok((staged_ledger, state, hash));
    }
    let (Some(dump), Some(root)) = (&args.dump, args.root) else {
        bail!("give --genesis, or --dump with --root");
    };
    // As `mina-block-replay --dump --root` does.
    let dump = StagedLedgerDump::read(dump)?;
    let states = dump.states_by_hash()?;
    let snarked = dump.snarked_ledger()?;
    let mut staged_ledger = dump.staged_ledger(&snarked, &states)?;
    let chain = std::fs::read_to_string(&args.chain)?;
    let hash = chain
        .lines()
        .filter_map(|line| line.split_once(' '))
        .find(|(height, _)| height.parse() == Ok(root))
        .map(|(_, hash)| hash.trim().to_owned())
        .with_context(|| format!("the root {root} is not in the chain file"))?;
    let block = read_block(&args.blocks, root, &hash)?;
    let block_hash = block.protocol_state.try_hash()?;
    if block_hash.to_string() != hash {
        bail!("block {root}: protocol state hashes to {block_hash}, not {hash}");
    }
    if block
        .protocol_state
        .body
        .blockchain_state
        .staged_ledger_hash
        != MinaBaseStagedLedgerHashStableV1::from(&staged_ledger.hash())
    {
        bail!("the dump is not the staged ledger of {root} {hash}");
    }
    staged_ledger.commit_and_reparent_to_root();
    println!("root {root} {hash} rebuilt from the dump");
    Ok((staged_ledger, block.protocol_state, block_hash))
}

fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("build-info") {
        println!("Commit SHA: {}", mina_node::BuildEnv::get().git.commit_hash);
        return Ok(());
    }
    let args = Args::parse();
    mina_core::NetworkConfig::init(&args.network).map_err(anyhow::Error::msg)?;
    let constraint_constants = mina_core::constants::constraint_constants();

    let mut wanted = args
        .want
        .iter()
        .map(|want| {
            let (name, shape) = want
                .split_once('=')
                .with_context(|| format!("--want {want}: not NAME=SHAPE"))?;
            let (shape, height) = match shape.split_once('@') {
                Some((shape, height)) => (shape, Some(height.parse()?)),
                None => (shape, None),
            };
            Ok(Wanted {
                name: name.to_owned(),
                shape: shape.to_owned(),
                height,
                written: None,
                proved_in: None,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let (mut staged_ledger, mut pred_state, mut pred_hash) = start(&args)?;
    let started = Instant::now();
    let mut domains: BTreeMap<String, u64> = BTreeMap::new();
    for line in BufReader::new(File::open(&args.chain)?).lines() {
        let line = line?;
        let mut fields = line.split_whitespace();
        let (Some(height), Some(hash)) = (fields.next(), fields.next()) else {
            continue;
        };
        let height: u32 = height.parse()?;
        if args.root.is_some_and(|root| height <= root) {
            continue;
        }
        if args.to.is_some_and(|to| height > to) {
            break;
        }

        // As in `mina-block-replay`.
        let block = read_block(&args.blocks, height, hash)?;
        let state = &block.protocol_state;
        if state.previous_state_hash != pred_hash {
            bail!("block {height} {hash}: parent is not the previous block {pred_hash}");
        }
        let pred_view = protocol_state_view(&pred_state)?;
        let pred: ProtocolState = (&pred_state).try_into()?;
        let consensus = &state.body.consensus_state;
        let diff: Diff = (&block.staged_ledger_diff).try_into()?;

        // The block's completed works: OCaml's proofs of earlier jobs.
        for work in diff.completed_works() {
            for proof in work.proofs.iter() {
                let proved = proof.statement();
                for want in wanted.iter_mut().filter(|want| {
                    want.proved_in.is_none() && want.written.as_ref() == Some(&proved)
                }) {
                    let message = v2::MinaBaseSokMessageStableV1 {
                        fee: (&work.fee).into(),
                        prover: (&work.prover).into(),
                    };
                    let proof: v2::LedgerProofProdStableV2 = proof.into();
                    let mut bytes = vec![];
                    (message, proof).binprot_write(&mut bytes)?;
                    let path = args.out.join(format!(
                        "mesa_{}_zkapp_{}_ocaml_work.bin",
                        args.network, want.name
                    ));
                    std::fs::write(&path, &bytes)?;
                    println!(
                        "{}: proved on chain in block {height} ({hash}), fee {}, by {}: {} bytes to {}",
                        want.name,
                        work.fee.as_u64(),
                        work.prover.into_address(),
                        bytes.len(),
                        path.display()
                    );
                    want.proved_in = Some(height);
                }
            }
        }

        let result = staged_ledger
            .apply(
                Some(SkipVerification::All),
                constraint_constants,
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
        let found = MinaBaseStagedLedgerHashStableV1::from(&result.hash_after_applying);
        if found != state.body.blockchain_state.staged_ledger_hash {
            bail!("block {height} {hash}: staged ledger hash mismatch");
        }
        staged_ledger.commit_and_reparent_to_root();
        let block_hash = state.try_hash()?;
        if block_hash.to_string() != hash {
            bail!("block {height}: protocol state hashes to {block_hash}, not {hash}");
        }

        // The jobs this block added: those of its parent's state hash, in the
        // order of the block's transactions.
        let pred_fp: Fp = pred_hash.to_field()?;
        let scan_state = staged_ledger.scan_state();
        let jobs = scan_state
            .base_jobs_on_latest_tree()
            .chain((0..64).flat_map(|i| scan_state.base_jobs_on_earlier_tree(i)));
        let mut seen: HashMap<Vec<u8>, ()> = HashMap::new();
        for (job_index, job) in jobs.filter(|job| job.state_hash.0 == pred_fp).enumerate() {
            if args.report_domains {
                report_domains(&job, height, &mut domains);
            }
            let Some(job_shape) = shape(&job) else {
                continue;
            };
            if args.capture_from.is_some_and(|from| height >= from) {
                let mut bytes = vec![];
                transition_spec(&job, &pred_state.body).binprot_write(&mut bytes)?;
                let path = args
                    .out
                    .join(format!("historical-{height}-{job_index}-{job_shape}.bin"));
                std::fs::write(&path, bytes)?;
                println!(
                    "captured {job_shape} in block {height} to {}",
                    path.display()
                );
            }
            let Some(want) = wanted.iter_mut().find(|want| {
                want.written.is_none()
                    && want.shape == job_shape
                    && want.height.is_none_or(|h| h == height)
            }) else {
                continue;
            };
            let mut bytes = vec![];
            let spec = transition_spec(&job, &pred_state.body);
            spec.binprot_write(&mut bytes)?;
            if seen.insert(bytes.clone(), ()).is_some() {
                continue;
            }
            let path = args.out.join(format!(
                "mesa_{}_zkapp_{}_base_job.bin",
                args.network, want.name
            ));
            std::fs::write(&path, &bytes)?;
            let Transaction::Command(UserCommand::ZkAppCommand(command)) =
                &job.transaction_with_status.data
            else {
                unreachable!()
            };
            let status = match &job.transaction_with_status.status {
                TransactionStatus::Applied => "applied".to_owned(),
                TransactionStatus::Failed(failures) => format!("failed {failures:?}"),
            };
            println!(
                "{}: zkApp command {} ({} account updates, shape {job_shape}, {status}), \
                 fee payer {} nonce {}, in block {height} ({hash}), global slot {}, \
                 state hash {pred_hash}: {} bytes to {}",
                want.name,
                v2::MinaBaseUserCommandStableV2::ZkappCommand((&**command).into()).hash()?,
                job_shape.len() - 1,
                command.fee_payer.body.public_key.into_address(),
                command.fee_payer.body.nonce.as_u32(),
                job.block_global_slot.as_u32(),
                bytes.len(),
                path.display()
            );
            want.written = Some(job.statement.clone());
        }

        pred_state = block.protocol_state;
        pred_hash = block_hash;
        if !args.report_domains
            && args.capture_from.is_none()
            && wanted.iter().all(|want| want.proved_in.is_some())
        {
            break;
        }
    }

    println!("replayed to {pred_hash} in {:.0?}", started.elapsed());
    if args.report_domains {
        println!("proof-authorised account updates by their key's domains: {domains:?}");
    }
    for want in &wanted {
        match (&want.written, want.proved_in) {
            (None, _) => println!("{}: no job of shape {}", want.name, want.shape),
            (Some(_), None) => println!("{}: not proved on chain yet", want.name),
            (Some(_), Some(_)) => {}
        }
    }
    if wanted.iter().any(|want| want.written.is_none()) {
        bail!("some shapes were not found");
    }
    Ok(())
}
