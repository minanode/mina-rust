//! Replay Mesa mainnet from genesis, as `mina-block-replay` does, and write the
//! first two merge snark jobs that the scan state makes available: one whose
//! children are base proofs (a node on the level above the leaves), and one
//! whose children are merge proofs (a node higher up). Each becomes the
//! `(statement, left proof, right proof)` triple in binprot that a snark worker
//! gets for `Merge` work, the input of the merge tests in `mesa_tests` in
//! `crates/ledger/src/proofs/transaction.rs`.
//!
//! The two child proofs are OCaml's: the snark work that the chain's blocks
//! carry in their completed works. The replay checks each block's staged
//! ledger hash, whose scan state part hashes the binprot of every merge job,
//! child proofs and their statements included (`ScanState::hash`). The
//! statement is the merge of the children's statements, as OCaml's
//! `single_spec_of_job` makes it. The replay goes on until a block's completed
//! work proves each written merge job, and reports that block: OCaml's own
//! merge proof of the same statement.
//!
//!   cargo run --release -p mina-block-replay --example mesa_merge_job_fixtures -- \
//!       --genesis mesa-mainnet.bin --chain blocks/chain.txt --blocks blocks \
//!       --out crates/ledger/src/proofs/data
//!
//! It stops once both are written and proved on chain, or at `--to`. With
//! `--count` it goes on to `--to` or the end of the chain, and counts the merge
//! jobs that become available, and those among them whose fee excess is
//! negative: the merges a prover must get the sign of right.

use std::{
    collections::HashSet,
    fs::File,
    io::{BufRead, BufReader},
    ops::Deref,
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

use anyhow::bail;
use clap::Parser;
use ledger::{
    proofs::block::ProtocolState,
    scan_state::{
        currency::Slot,
        scan_state::{
            transaction_snark::{LedgerProof, OneOrTwo, Statement, TransactionWithWitness},
            JobValue, JobValueBase, JobValueMerge, JobValueWithIndex,
        },
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
use mina_node::transition_frontier::genesis::PrebuiltGenesisConfig;
use mina_p2p_messages::{
    binprot::BinProtWrite,
    v2::{self, MinaBaseStagedLedgerHashStableV1, MinaStateProtocolStateValueStableV2},
};

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
    /// Go on to `--to` or the end of the chain, counting the merge jobs.
    #[arg(long)]
    count: bool,
}

/// Merge jobs that became available, of one kind.
#[derive(Default, Debug)]
struct Count {
    jobs: u64,
    /// With a negative fee excess, left or right.
    negative: u64,
    /// The first block after which one with a negative fee excess was.
    first_negative: Option<u32>,
}

/// The two fixtures: `mesa_mainnet_<name>_job.bin`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// Children are base proofs.
    MergeOfBases,
    /// Children are merge proofs.
    MergeOfMerges,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::MergeOfBases => "merge_of_bases",
            Kind::MergeOfMerges => "merge_of_merges",
        }
    }
}

/// A written merge job, until a block's completed work proves it.
struct Written {
    kind: Kind,
    statement: Statement<()>,
    proved_in: Option<u32>,
}

/// The kind of transaction of a base job, as the base job fixtures name it.
fn kind_of_base(job: &TransactionWithWitness) -> String {
    let kind = match &job.transaction_with_status.data {
        Transaction::Command(UserCommand::SignedCommand(command)) => match command.payload.body {
            Body::Payment(_) => "payment",
            Body::StakeDelegation(_) => "delegation",
        },
        Transaction::Command(UserCommand::ZkAppCommand(_)) => "zkapp",
        Transaction::FeeTransfer(transfer) => match transfer.deref() {
            OneOrTwo::One(_) => "fee_transfer",
            OneOrTwo::Two(_) => "fee_transfer_two",
        },
        Transaction::Coinbase(coinbase) => match coinbase.fee_transfer {
            None => "coinbase",
            Some(_) => "coinbase_fee_transfer",
        },
    };
    match job.transaction_with_status.status {
        TransactionStatus::Applied => kind.to_string(),
        TransactionStatus::Failed(_) => format!("failed_{kind}"),
    }
}

/// What a tree node holds, for the log: a base job's kind, or a merge.
fn describe_node(jobs: &[JobValueWithIndex<'_>], index: usize) -> String {
    match jobs.get(index).map(|job| &job.job) {
        Some(JobValue::Leaf(JobValueBase::Full(record))) => format!(
            "base {} (state {:?})",
            kind_of_base(&record.job),
            record.state
        ),
        Some(JobValue::Node(JobValueMerge::Full(record))) => {
            format!("merge (state {:?})", record.state)
        }
        Some(JobValue::Node(JobValueMerge::Part(_))) => "half a merge".to_string(),
        Some(JobValue::Leaf(JobValueBase::Empty)) | Some(JobValue::Node(JobValueMerge::Empty)) => {
            "empty".to_string()
        }
        None => "none".to_string(),
    }
}

/// The `Merge` work a snark worker gets for a merge job: the merged
/// statement, and the two child proofs.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transaction_snark_scan_state/transaction_snark_scan_state.ml#L1363-L1369>
fn merge_spec(
    left: &LedgerProof,
    right: &LedgerProof,
) -> anyhow::Result<(
    Statement<()>,
    (
        v2::MinaStateSnarkedLedgerStateStableV2,
        v2::LedgerProofProdStableV2,
        v2::LedgerProofProdStableV2,
    ),
)> {
    let merged = left
        .statement()
        .merge(&right.statement())
        .map_err(anyhow::Error::msg)?;
    let spec = ((&merged).into(), left.into(), right.into());
    Ok((merged, spec))
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
    let mut written: Vec<Written> = vec![];
    // With `--count`: the merged statements of the merge jobs available after
    // the previous block, in binprot, and the counts by kind.
    let mut available: HashSet<Vec<u8>> = HashSet::new();
    let mut counts = [Count::default(), Count::default()];
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

        // The block's completed works: OCaml's proofs of earlier jobs.
        for work in diff.completed_works() {
            for proof in work.proofs.iter() {
                let proved = proof.statement();
                for job in written.iter_mut().filter(|job| job.proved_in.is_none()) {
                    if job.statement == proved {
                        println!(
                            "{}: proved on chain in block {height} ({hash}), fee {}, by {}",
                            job.kind.name(),
                            work.fee.as_u64(),
                            work.prover.into_address()
                        );
                        job.proved_in = Some(height);
                    }
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

        // The merge jobs now available: full nodes still to do.
        let mut now_available = HashSet::new();
        for (tree, jobs) in staged_ledger.scan_state().view().enumerate() {
            let jobs: Vec<JobValueWithIndex<'_>> = jobs.collect();
            let leaf_depth = jobs.last().map_or(0, |job| job.depth());
            for job in &jobs {
                let JobValue::Node(JobValueMerge::Full(record)) = &job.job else {
                    continue;
                };
                if record.state.is_done() {
                    continue;
                }
                let kind = if job.depth() + 1 == leaf_depth {
                    Kind::MergeOfBases
                } else {
                    Kind::MergeOfMerges
                };
                if args.count {
                    let merged = record
                        .left
                        .statement()
                        .merge(&record.right.statement())
                        .map_err(anyhow::Error::msg)?;
                    let mut key = vec![];
                    v2::MinaStateSnarkedLedgerStateStableV2::from(&merged)
                        .binprot_write(&mut key)?;
                    if !available.contains(&key) {
                        let count = &mut counts[kind as usize];
                        count.jobs += 1;
                        let excess = &merged.fee_excess;
                        if excess.fee_excess_l.is_neg() || excess.fee_excess_r.is_neg() {
                            count.negative += 1;
                            count.first_negative.get_or_insert(height);
                        }
                    }
                    now_available.insert(key);
                }
                if written.iter().any(|w| w.kind == kind) {
                    continue;
                }
                let (left, right): (&Arc<LedgerProof>, &Arc<LedgerProof>) =
                    (&record.left, &record.right);
                let (statement, spec) = merge_spec(left, right)?;
                let mut bytes = vec![];
                spec.binprot_write(&mut bytes)?;
                let path = args
                    .out
                    .join(format!("mesa_mainnet_{}_job.bin", kind.name()));
                std::fs::write(&path, &bytes)?;
                println!(
                    "{}: tree {tree}, node {} at depth {} (leaves at {leaf_depth}), after block \
                     {height} ({hash}): {} bytes to {}",
                    kind.name(),
                    job.index(),
                    job.depth(),
                    bytes.len(),
                    path.display()
                );
                println!(
                    "  children: {} and {}; sok digests {} and {}",
                    describe_node(&jobs, job.child_left()),
                    describe_node(&jobs, job.child_right()),
                    hex(&left.0.statement.sok_digest.0),
                    hex(&right.0.statement.sok_digest.0),
                );
                if kind == Kind::MergeOfBases {
                    // The base jobs under it, for the base job fixtures.
                    for child in [job.child_left(), job.child_right()] {
                        if let Some(JobValue::Leaf(JobValueBase::Full(record))) =
                            jobs.get(child).map(|job| &job.job)
                        {
                            println!(
                                "  base job {child}: {}, block global slot {}",
                                kind_of_base(&record.job),
                                record.job.block_global_slot.as_u32()
                            );
                        }
                    }
                }
                written.push(Written {
                    kind,
                    statement,
                    proved_in: None,
                });
            }
        }

        available = now_available;

        pred_state = block.protocol_state;
        pred_hash = block_hash;
        if !args.count && written.len() == 2 && written.iter().all(|job| job.proved_in.is_some()) {
            break;
        }
    }

    println!("replayed to {pred_hash} in {:.0?}", started.elapsed());
    if args.count {
        for kind in [Kind::MergeOfBases, Kind::MergeOfMerges] {
            println!("{}: {:?}", kind.name(), counts[kind as usize]);
        }
    }
    for kind in [Kind::MergeOfBases, Kind::MergeOfMerges] {
        match written.iter().find(|job| job.kind == kind) {
            None => bail!("no {} job", kind.name()),
            Some(job) if job.proved_in.is_none() => {
                println!("{}: not proved on chain yet", kind.name())
            }
            Some(_) => {}
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
