//! Check the base transaction circuit's constraints, without proving, for the
//! base jobs of a Mesa mainnet or devnet scan state: payments, delegations, fee
//! transfers and coinbases, applied or failed. zkApp commands use the zkApp
//! circuits (P5.1) and are skipped.
//!
//! The input is a staged ledger dump, as for `mesa_base_job_fixture`. Each job
//! becomes the `(statement, transaction witness)` a snark worker gets, with the
//! protocol state body of its block, and goes through `generate_tx_proof` with
//! `only_verify_constraints`. Needs the Mesa circuit blobs
//! (`tools/mesa-circuits/generate.sh`, then `MINA_CIRCUIT_BLOBS_BASE_DIR`).
//! Exits non-zero if any checked job fails.
//!
//!   cargo run --release -p mina-tree --example mesa_base_jobs_check -- \
//!     <reconstruct dump> [jobs per kind, default 10] [network, default mainnet]
//!
//! For devnet the blobs are `MINA_PROFILE=devnet tools/mesa-circuits/generate.sh`'s
//! (`4.0.2devnet`).

use std::collections::BTreeMap;

use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    binprot::{
        self,
        macros::{BinProtRead, BinProtWrite},
        BinProtRead,
    },
    list::List,
    v2,
};
use mina_signer::CompressedPubKey;
use mina_tree::{
    proofs::{
        generate_tx_proof,
        provers::TransactionProver,
        transaction::{ProofError, TransactionParams},
    },
    scan_state::{
        currency::Fee,
        scan_state::{transaction_snark::SokMessage, ScanState},
        transaction_logic::{signed_command::Body, Transaction, TransactionStatus, UserCommand},
    },
};

#[derive(BinProtRead, BinProtWrite)]
struct ReconstructContext {
    accounts: Vec<v2::MinaBaseAccountBinableArgStableV2>,
    scan_state: v2::TransactionSnarkScanStateStableV2,
    pending_coinbase: v2::MinaBasePendingCoinbaseStableV2,
    staged_ledger_hash: v2::LedgerHash,
    states: List<v2::MinaStateProtocolStateValueStableV2>,
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(input) = args.next() else {
        anyhow::bail!("usage: mesa_base_jobs_check <reconstruct dump> [jobs per kind] [network]");
    };
    let per_kind: usize = args.next().map(|n| n.parse()).transpose()?.unwrap_or(10);
    let network = args.next().unwrap_or_else(|| "mainnet".to_owned());
    mina_core::NetworkConfig::init(&network).map_err(anyhow::Error::msg)?;
    println!("network {network}");

    let ctx = ReconstructContext::binprot_read(&mut std::fs::read(input)?.as_slice())?;
    let states = ctx
        .states
        .iter()
        .map(|state| Ok((state.try_hash()?.to_field::<Fp>()?, state.clone())))
        .collect::<anyhow::Result<BTreeMap<Fp, _>>>()?;
    let scan_state: ScanState = (&ctx.scan_state).try_into()?;
    let provers = TransactionProver::make(None)?;
    let message = SokMessage {
        fee: Fee::from_u64(0),
        prover: CompressedPubKey::from_address(
            "B62qpK6TcG4sWdtT3BzdbWHiK3RJMj3Zbo9mqwBos7cVsydPMCj5wZx",
        )
        .map_err(|e| anyhow::anyhow!("{e:?}"))?,
    };

    // (kind, applied) -> (seen, checked, satisfied)
    let mut tally: BTreeMap<(&str, bool), (usize, usize, usize)> = BTreeMap::new();
    let mut heights = (u32::MAX, 0);
    let jobs = scan_state
        .base_jobs_on_latest_tree()
        .chain((0..64).flat_map(|i| scan_state.base_jobs_on_earlier_tree(i)));
    for job in jobs {
        let kind = match &job.transaction_with_status.data {
            Transaction::Command(UserCommand::SignedCommand(command)) => {
                match command.payload.body {
                    Body::Payment(_) => "payment",
                    Body::StakeDelegation(_) => "delegation",
                }
            }
            Transaction::Command(UserCommand::ZkAppCommand(_)) => "zkapp (skipped)",
            Transaction::FeeTransfer(_) => "fee transfer",
            Transaction::Coinbase(_) => "coinbase",
        };
        let applied = job.transaction_with_status.status == TransactionStatus::Applied;
        let entry = tally.entry((kind, applied)).or_default();
        entry.0 += 1;
        if kind.starts_with("zkapp") || entry.1 >= per_kind {
            continue;
        }
        let Some(state) = states.get(&job.state_hash.0) else {
            continue;
        };
        let height = state.body.consensus_state.blockchain_length.0.as_u32();
        heights = (heights.0.min(height), heights.1.max(height));

        let base: v2::TransactionSnarkScanStateTransactionWithWitnessStableV2 = (&*job).into();
        let statement = base.statement.0;
        let witness = v2::TransactionWitnessStableV2 {
            transaction: base.transaction_with_status.data,
            first_pass_ledger: base.first_pass_ledger_witness,
            second_pass_ledger: base.second_pass_ledger_witness,
            protocol_state_body: state.body.clone(),
            init_stack: base.init_stack,
            status: base.transaction_with_status.status,
            block_global_slot: base.block_global_slot,
        };
        let result = generate_tx_proof(TransactionParams {
            statement: &statement,
            tx_witness: &witness,
            message: &message,
            tx_step_prover: &provers.tx_step_prover,
            tx_wrap_prover: &provers.tx_wrap_prover,
            only_verify_constraints: true,
            expected_step_proof: None,
            ocaml_wrap_witness: None,
        });
        entry.1 += 1;
        match result {
            Err(e)
                if matches!(
                    e.downcast_ref::<ProofError>(),
                    Some(ProofError::ConstraintsOk)
                ) =>
            {
                entry.2 += 1
            }
            Err(e) => println!(
                "{kind} (applied {applied}) at global slot {}: {e}",
                job.block_global_slot.as_u32()
            ),
            Ok(_) => println!("{kind}: made a proof instead of checking constraints"),
        }
    }

    println!("checked jobs from blocks {} to {}", heights.0, heights.1);
    let mut ok = true;
    for ((kind, applied), (seen, checked, satisfied)) in &tally {
        println!(
            "{kind:16} {:7} {seen:5} jobs, {checked:3} checked, {satisfied:3} satisfy the circuit",
            if *applied { "applied" } else { "failed" }
        );
        ok &= satisfied == checked;
    }
    if !ok {
        anyhow::bail!("some base jobs don't satisfy the circuit");
    }
    Ok(())
}
