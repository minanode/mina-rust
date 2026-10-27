//! Extract one real merge snark job whose children are base proofs, with
//! OCaml's two proofs, from a staged ledger dump, for the merge tests in
//! `proofs::transaction::mesa_tests`.
//!
//! The dump is what a node writes with `MINA_DUMP_RECONSTRUCT` set, as for
//! `mesa_base_job_fixture`: the root's snarked ledger, scan state and the
//! protocol states its jobs refer to. Its scan state came from OCaml peers,
//! so the ledger proofs in its full merge nodes are OCaml's: the snark work
//! the chain's blocks carried. The first full merge node on the level above
//! the leaves that is still to do becomes the `(statement, left proof, right
//! proof)` triple in binprot that a snark worker gets for `Merge` work, as
//! `mesa_merge_job_fixtures` (in `tools/block-replay`) writes it from a
//! replay. The statement is the merge of the children's statements, as
//! OCaml's `single_spec_of_job` makes it.
//!
//! The two base jobs under it, still in the tree, are written too, as
//! `mesa_base_job_fixture` writes one (`Transition` work, with the protocol
//! state body of the job's state hash from the dump): `<output>` with
//! `_left_base_job.bin` and `_right_base_job.bin` in place of `_job.bin`, so
//! that the base proofs can be made in Rust and merged for the same statement.
//!
//!   cargo run --release -p mina-tree --example mesa_merge_job_from_dump -- \
//!     <reconstruct dump> <output file> [network, default mainnet]
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transaction_snark_scan_state/transaction_snark_scan_state.ml#L1363-L1369>

use std::collections::BTreeMap;

use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    binprot::{
        self,
        macros::{BinProtRead, BinProtWrite},
        BinProtRead, BinProtWrite,
    },
    list::List,
    v2,
};
use mina_tree::scan_state::scan_state::{
    transaction_snark::TransactionWithWitness, JobValue, JobValueBase, JobValueMerge,
    JobValueWithIndex, ScanState,
};

#[derive(BinProtRead, BinProtWrite)]
struct ReconstructContext {
    accounts: Vec<v2::MinaBaseAccountBinableArgStableV2>,
    scan_state: v2::TransactionSnarkScanStateStableV2,
    pending_coinbase: v2::MinaBasePendingCoinbaseStableV2,
    staged_ledger_hash: v2::LedgerHash,
    states: List<v2::MinaStateProtocolStateValueStableV2>,
}

/// The `Transition` work a snark worker gets for a base job, as
/// `mesa_base_job_fixture` makes it.
fn transition_spec(
    job: &TransactionWithWitness,
    state: &v2::MinaStateProtocolStateValueStableV2,
) -> (
    v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement,
    v2::TransactionWitnessStableV2,
) {
    let base: v2::TransactionSnarkScanStateTransactionWithWitnessStableV2 = job.into();
    let witness = v2::TransactionWitnessStableV2 {
        transaction: base.transaction_with_status.data,
        first_pass_ledger: base.first_pass_ledger_witness,
        second_pass_ledger: base.second_pass_ledger_witness,
        protocol_state_body: state.body.clone(),
        init_stack: base.init_stack,
        status: base.transaction_with_status.status,
        block_global_slot: base.block_global_slot,
    };
    (base.statement.0, witness)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        anyhow::bail!("usage: mesa_merge_job_from_dump <reconstruct dump> <output file> [network]");
    };
    let network = args.next().unwrap_or_else(|| "mainnet".to_owned());
    mina_core::NetworkConfig::init(&network).map_err(anyhow::Error::msg)?;

    let ctx = ReconstructContext::binprot_read(&mut std::fs::read(input)?.as_slice())?;
    let states = ctx
        .states
        .iter()
        .map(|state| Ok((state.try_hash()?.to_field::<Fp>()?, state.clone())))
        .collect::<anyhow::Result<BTreeMap<Fp, _>>>()?;
    let scan_state: ScanState = (&ctx.scan_state).try_into()?;

    let mut full_merges = 0;
    for (tree, jobs) in scan_state.view().enumerate() {
        let jobs: Vec<JobValueWithIndex<'_>> = jobs.collect();
        let leaf_depth = jobs.last().map_or(0, |job| job.depth());
        for job in &jobs {
            let JobValue::Node(JobValueMerge::Full(record)) = &job.job else {
                continue;
            };
            full_merges += 1;
            if record.state.is_done() || job.depth() + 1 != leaf_depth {
                continue;
            }
            let (left, right) = (&record.left, &record.right);
            let merged = left
                .statement()
                .merge(&right.statement())
                .map_err(anyhow::Error::msg)?;
            let spec: (
                v2::MinaStateSnarkedLedgerStateStableV2,
                v2::LedgerProofProdStableV2,
                v2::LedgerProofProdStableV2,
            ) = ((&merged).into(), (&**left).into(), (&**right).into());
            let mut bytes = vec![];
            spec.binprot_write(&mut bytes)?;
            std::fs::write(&output, &bytes)?;

            let base = |index: usize| match jobs.get(index).map(|job| &job.job) {
                Some(JobValue::Leaf(JobValueBase::Full(record))) => format!(
                    "base job {index}, block global slot {}",
                    record.job.block_global_slot.as_u32()
                ),
                _ => format!("node {index}, no base job (already merged)"),
            };
            for (side, index) in [("left", job.child_left()), ("right", job.child_right())] {
                let Some(JobValue::Leaf(JobValueBase::Full(record))) =
                    jobs.get(index).map(|job| &job.job)
                else {
                    anyhow::bail!("the {side} child, node {index}, is not a base job");
                };
                let state = states
                    .get(&record.job.state_hash.0)
                    .ok_or_else(|| anyhow::anyhow!("no state for the {side} base job"))?;
                let mut bytes = vec![];
                transition_spec(&record.job, state).binprot_write(&mut bytes)?;
                let path = output.replace("_job.bin", &format!("_{side}_base_job.bin"));
                anyhow::ensure!(path != output, "the output file name must end in _job.bin");
                std::fs::write(&path, &bytes)?;
                println!(
                    "  {side} base job: {} bytes to {path}, state {} (height {})",
                    bytes.len(),
                    v2::StateHash::from_fp(record.job.state_hash.0),
                    state.body.consensus_state.blockchain_length.as_u32(),
                );
            }
            println!(
                "{network}: merge of base proofs, tree {tree}, node {} at depth {} (leaves at \
                 {leaf_depth}): {} bytes to {output}",
                job.index(),
                job.depth(),
                bytes.len(),
            );
            println!(
                "  children: {} and {}; sok digests {} and {}",
                base(job.child_left()),
                base(job.child_right()),
                hex(&left.0.statement.sok_digest.0),
                hex(&right.0.statement.sok_digest.0),
            );
            println!(
                "  merged statement: first pass ledger {} to {}, supply increase {:?}, fee \
                 excess {:?}",
                v2::LedgerHash::from_fp(merged.source.first_pass_ledger),
                v2::LedgerHash::from_fp(merged.target.first_pass_ledger),
                merged.supply_increase,
                merged.fee_excess,
            );
            return Ok(());
        }
    }
    anyhow::bail!("no merge of base proofs to do among {full_merges} full merge nodes");
}
