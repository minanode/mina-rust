//! Extract one real Mesa mainnet or devnet payment, as a base snark job, from
//! a staged ledger dump, for the tests in `proofs::transaction::mesa_tests`.
//!
//! The dump is what a node writes with `MINA_DUMP_RECONSTRUCT` set
//! (`<data dir>/debug/failed_reconstruct_ctx.binprot`): the root's snarked
//! ledger, scan state and the protocol states its jobs refer to. The scan
//! state came from OCaml peers, so its base jobs carry OCaml's own sparse
//! ledgers. The first applied payment among the base jobs becomes a
//! `(statement, transaction witness)` pair in binprot, the input a snark
//! worker gets (`Transition` work), with the protocol state body taken from
//! the dump by the job's state hash, as OCaml's `single_spec_of_job` does.
//!
//!   cargo run --release -p mina-tree --example mesa_base_job_fixture -- \
//!     <reconstruct dump> <output file> [network, default mainnet]

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
use mina_tree::scan_state::{
    scan_state::ScanState,
    transaction_logic::{signed_command::Body, Transaction, TransactionStatus, UserCommand},
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
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        anyhow::bail!("usage: mesa_base_job_fixture <reconstruct dump> <output file> [network]");
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

    let jobs = scan_state
        .base_jobs_on_latest_tree()
        .chain((0..64).flat_map(|i| scan_state.base_jobs_on_earlier_tree(i)));
    let mut seen = 0;
    for job in jobs {
        seen += 1;
        let Transaction::Command(UserCommand::SignedCommand(command)) =
            &job.transaction_with_status.data
        else {
            continue;
        };
        let Body::Payment(payment) = &command.payload.body else {
            continue;
        };
        if job.transaction_with_status.status != TransactionStatus::Applied {
            continue;
        }
        let Some(state) = states.get(&job.state_hash.0) else {
            continue;
        };

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

        let mut bytes = vec![];
        (statement, witness).binprot_write(&mut bytes)?;
        std::fs::write(&output, &bytes)?;
        println!(
            "payment of {} from {} to {}, fee {}, nonce {}, in the block of state {} \
             (height {}, global slot {}), base job {seen}: {} bytes to {output}",
            payment.amount.as_u64(),
            command.payload.common.fee_payer_pk.into_address(),
            payment.receiver_pk.into_address(),
            command.payload.common.fee.as_u64(),
            command.payload.common.nonce.as_u32(),
            v2::StateHash::from_fp(job.state_hash.0),
            state.body.consensus_state.blockchain_length.0.as_u32(),
            job.block_global_slot.as_u32(),
            bytes.len(),
        );
        return Ok(());
    }
    anyhow::bail!("no applied payment among {seen} base jobs");
}
