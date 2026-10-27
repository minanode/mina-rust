//! Replay Mesa mainnet from genesis, as `mina-block-replay` does, and write the
//! first base snark job of each non-zkApp kind that the chain's blocks add to
//! the scan state: a stake delegation, a failed payment, a failed delegation, a
//! fee transfer to one and to two receivers, and a coinbase without and with a
//! fee transfer. Each becomes the `(statement, transaction witness)` pair in
//! binprot that a snark worker gets for `Transition` work, the input of the
//! `mesa_tests` in `crates/ledger/src/proofs/transaction.rs`.
//!
//! The jobs are OCaml's own: the replay checks each block's staged ledger hash,
//! whose scan state part hashes the binprot of every base job, sparse ledgers
//! included (`ScanState::hash`). The witness takes the protocol state body of
//! the job's state hash, the block before the one that added it, as the node's
//! snark worker spec does (`external_snark_worker_impls.rs`) and OCaml's
//! `single_spec_of_job`. zkApp commands use the zkApp step circuits (P5.1)
//! and are left out; applied payments too, which have the P4.1 fixture.
//!
//!   cargo run --release -p mina-block-replay --example mesa_base_job_fixtures -- \
//!       --genesis mesa-mainnet.bin --chain blocks/chain.txt --blocks blocks \
//!       --out crates/ledger/src/proofs/data
//!
//! It stops once every kind is written, or at `--to`.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    ops::Deref,
    path::PathBuf,
    time::Instant,
};

use anyhow::bail;
use clap::Parser;
use ledger::{
    proofs::block::ProtocolState,
    scan_state::{
        currency::Slot,
        scan_state::transaction_snark::{OneOrTwo, TransactionWithWitness},
        transaction_logic::{
            protocol_state::protocol_state_view, signed_command::Body, Transaction,
            TransactionStatus, UserCommand,
        },
    },
    staged_ledger::{
        diff::Diff,
        staged_ledger::{SkipVerification, StagedLedger},
    },
    verifier::Verifier,
};
use mina_block_replay::read_block;
use mina_curves::pasta::Fp;
use mina_node::transition_frontier::genesis::PrebuiltGenesisConfig;
use mina_p2p_messages::{
    binprot::BinProtWrite,
    v2::{self, MinaBaseStagedLedgerHashStableV1, MinaStateProtocolStateValueStableV2},
};

/// The kinds written, each to `mesa_mainnet_<kind>_base_job.bin`.
const KINDS: &[&str] = &[
    "delegation",
    "failed_payment",
    "failed_delegation",
    "fee_transfer",
    "fee_transfer_two",
    "coinbase",
    "coinbase_fee_transfer",
];

#[derive(Parser)]
struct Args {
    /// Prebuilt genesis config, as written by `ledger-tool`.
    #[arg(long)]
    genesis: PathBuf,
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
}

/// The fixture name of a base job's kind, `None` for zkApp commands.
fn kind(job: &TransactionWithWitness) -> Option<String> {
    let kind = match &job.transaction_with_status.data {
        Transaction::Command(UserCommand::SignedCommand(command)) => match command.payload.body {
            Body::Payment(_) => "payment",
            Body::StakeDelegation(_) => "delegation",
        },
        Transaction::Command(UserCommand::ZkAppCommand(_)) => return None,
        Transaction::FeeTransfer(transfer) => match transfer.deref() {
            OneOrTwo::One(_) => "fee_transfer",
            OneOrTwo::Two(_) => "fee_transfer_two",
        },
        Transaction::Coinbase(coinbase) => match coinbase.fee_transfer {
            None => "coinbase",
            Some(_) => "coinbase_fee_transfer",
        },
    };
    Some(match job.transaction_with_status.status {
        TransactionStatus::Applied => kind.to_string(),
        TransactionStatus::Failed(_) => format!("failed_{kind}"),
    })
}

/// One line about the transaction, for the fixture's doc comment.
fn describe(job: &TransactionWithWitness) -> String {
    let mina = |nanomina: u64| {
        format!(
            "{}.{:09}",
            nanomina / 1_000_000_000,
            nanomina % 1_000_000_000
        )
    };
    let what = match &job.transaction_with_status.data {
        Transaction::Command(UserCommand::SignedCommand(command)) => {
            let common = &command.payload.common;
            let body = match &command.payload.body {
                Body::Payment(payment) => format!(
                    "payment of {} MINA to {}",
                    mina(payment.amount.as_u64()),
                    payment.receiver_pk.into_address()
                ),
                Body::StakeDelegation(delegation) => {
                    format!("delegation to {}", delegation.receiver_pk().into_address())
                }
            };
            format!(
                "{body} from {}, fee {}, nonce {}",
                common.fee_payer_pk.into_address(),
                mina(common.fee.as_u64()),
                common.nonce.as_u32()
            )
        }
        Transaction::Command(UserCommand::ZkAppCommand(_)) => "zkApp command".to_string(),
        Transaction::FeeTransfer(transfer) => {
            let single = |t: &ledger::scan_state::transaction_logic::SingleFeeTransfer| {
                format!(
                    "{} MINA to {}",
                    mina(t.fee.as_u64()),
                    t.receiver_pk.into_address()
                )
            };
            match transfer.deref() {
                OneOrTwo::One(t) => format!("fee transfer of {}", single(t)),
                OneOrTwo::Two((a, b)) => {
                    format!("fee transfer of {} and {}", single(a), single(b))
                }
            }
        }
        Transaction::Coinbase(coinbase) => {
            let transfer = match &coinbase.fee_transfer {
                None => String::new(),
                Some(t) => format!(
                    ", with a fee transfer of {} MINA to {}",
                    mina(t.fee.as_u64()),
                    t.receiver_pk.into_address()
                ),
            };
            format!(
                "coinbase of {} MINA to {}{transfer}",
                mina(coinbase.amount.as_u64()),
                coinbase.receiver.into_address()
            )
        }
    };
    match &job.transaction_with_status.status {
        TransactionStatus::Applied => what,
        TransactionStatus::Failed(failures) => {
            let failures: Vec<String> = failures.iter().flatten().map(|f| f.to_string()).collect();
            format!("{what}, failed with {}", failures.join(", "))
        }
    }
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

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    mina_core::NetworkConfig::init("mainnet").map_err(anyhow::Error::msg)?;
    let constraint_constants = mina_core::constants::constraint_constants();

    let (mut masks, loaded) =
        PrebuiltGenesisConfig::load_from_bytes(&std::fs::read(&args.genesis)?)?;
    let (_, genesis_state, genesis_hash) = loaded.protocol_states(
        loaded.genesis_total_currency.clone(),
        loaded.staking_epoch_total_currency.clone(),
        loaded.next_epoch_total_currency.clone(),
    )?;
    let mut staged_ledger = StagedLedger::create_exn(constraint_constants.clone(), masks.remove(0))
        .map_err(anyhow::Error::msg)?;
    let mut pred_state: MinaStateProtocolStateValueStableV2 = genesis_state;
    let mut pred_hash: v2::StateHash = genesis_hash;

    let started = Instant::now();
    let mut written: BTreeMap<String, u32> = BTreeMap::new();
    for line in BufReader::new(File::open(&args.chain)?).lines() {
        let line = line?;
        let mut fields = line.split_whitespace();
        let (Some(height), Some(hash)) = (fields.next(), fields.next()) else {
            continue;
        };
        let height: u32 = height.parse()?;
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

        // The jobs this block added: those of its parent's state hash.
        let pred_fp: Fp = pred_hash.to_field()?;
        let scan_state = staged_ledger.scan_state();
        let jobs = scan_state
            .base_jobs_on_latest_tree()
            .chain((0..64).flat_map(|i| scan_state.base_jobs_on_earlier_tree(i)));
        for job in jobs.filter(|job| job.state_hash.0 == pred_fp) {
            let Some(kind) = kind(&job) else { continue };
            if !KINDS.contains(&kind.as_str()) || written.contains_key(&kind) {
                continue;
            }
            let mut bytes = vec![];
            transition_spec(&job, &pred_state.body).binprot_write(&mut bytes)?;
            let path = args.out.join(format!("mesa_mainnet_{kind}_base_job.bin"));
            std::fs::write(&path, &bytes)?;
            println!(
                "{kind}: {}, in block {height} ({hash}), global slot {}: {} bytes to {}",
                describe(&job),
                job.block_global_slot.as_u32(),
                bytes.len(),
                path.display()
            );
            written.insert(kind, height);
        }

        pred_state = block.protocol_state;
        pred_hash = block_hash;
        if written.len() == KINDS.len() {
            break;
        }
    }

    println!("replayed to {pred_hash} in {:.0?}", started.elapsed());
    let missing: Vec<_> = KINDS
        .iter()
        .filter(|kind| !written.contains_key(**kind))
        .collect();
    if !missing.is_empty() {
        bail!("no base job of these kinds: {missing:?}");
    }
    Ok(())
}
