//! Turn o1js zkApp commands into the base jobs a snark worker proves, on a
//! fresh ledger, for the differential prover corpus
//! (`tools/prover-div-corpus`).
//!
//! `tools/prover-div-corpus/zkapp-gates/gen.mjs` builds proof-authorised
//! commands on an o1js `LocalBlockchain`, one custom gate per proof, and
//! writes their GraphQL `ZkappCommandInput`s, as `Transaction.toJSON` does,
//! with the fee payer's starting balance. This funds the fee payer in a new
//! ledger and applies each command in order, as the only transaction of a
//! block whose parent is `--parent-block`, with an empty pending coinbase
//! stack, as `StagedLedger` applies a block's transactions. A job is
//! `(statement, transaction witness)` in binprot, the `base` format of
//! `prover_div_corpus normalize`. Its statement is self-consistent, not the
//! chain's: the ledger is a new one.
//!
//!   cargo run --release -p mina-cli --no-default-features --example zkapp_gate_jobs -- \
//!       --network devnet --parent-block 581805.json --inputs zkapps.json --out jobs
//!
//! `--inputs` is `{fund: [{public_key, balance}], commands: [{name, graphql}]}`,
//! balances in nanomina. `--parent-block` is a precomputed block, as JSON. The
//! global slot is the parent's plus one unless `--global-slot` is given.

use std::{fs, path::PathBuf};

use anyhow::{ensure, Context, Result};
use clap::Parser;
use ledger::{
    proofs::block::ProtocolState,
    scan_state::{
        currency::{Balance, Slot},
        pending_coinbase::Stack,
        scan_state::transaction_snark::TransactionWithWitness,
        transaction_logic::{
            protocol_state::protocol_state_view, zkapp_command::Control, Transaction,
            TransactionStatus, UserCommand, WithStatus,
        },
    },
    staged_ledger::staged_ledger::StagedLedger,
    Account, AccountId, BaseLedger, Database, Mask, TokenId,
};
use mina_core::NetworkConfig;
use mina_node_native::graphql::{
    api::{FromInputValue, InputValue},
    zkapp::InputGraphQLZkappCommand,
};
use mina_p2p_messages::{
    binprot::BinProtWrite,
    v2::{self, precomputed, MinaStateProtocolStateValueStableV2},
};
use mina_signer::PubKey;
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Parser)]
struct Args {
    /// `mainnet` or `devnet`.
    #[arg(long, default_value = "devnet")]
    network: String,
    /// The parent block, a precomputed block as JSON.
    #[arg(long)]
    parent_block: PathBuf,
    /// The block's global slot since genesis; the parent's plus one if not
    /// given.
    #[arg(long)]
    global_slot: Option<u32>,
    /// The fee payer and the commands, see the module doc.
    #[arg(long)]
    inputs: PathBuf,
    /// Where to write `<name>_base_job.bin`.
    #[arg(long)]
    out: PathBuf,
}

#[derive(Deserialize)]
struct Inputs {
    fund: Vec<Fund>,
    commands: Vec<Command>,
}

#[derive(Deserialize)]
struct Fund {
    public_key: String,
    balance: String,
}

#[derive(Deserialize)]
struct Command {
    name: String,
    graphql: serde_json::Value,
}

impl Command {
    /// As the node's `sendZkapp` reads its input.
    fn user_command(&self) -> Result<v2::MinaBaseUserCommandStableV2> {
        let input: InputValue = serde_json::from_value(self.graphql.clone())?;
        let input = InputGraphQLZkappCommand::from_input_value(&input)
            .map_err(|e| anyhow::anyhow!("{}: not a ZkappCommandInput: {e:?}", self.name))?;
        input
            .try_into()
            .map_err(|e| anyhow::anyhow!("{}: {e:?}", self.name))
    }
}

/// The authorization of each account update, fee payer first, as
/// `mesa_zkapp_job_fixtures` names shapes, and each proof's feature flags and
/// step domain.
fn describe(command: &UserCommand) -> (String, Vec<String>) {
    let UserCommand::ZkAppCommand(command) = command else {
        return ("signed command".to_owned(), vec![]);
    };
    let mut shape = String::from("S");
    let mut proofs = vec![];
    command
        .account_updates
        .fold((), |(), update| match &update.authorization {
            Control::Signature(_) => shape.push('S'),
            Control::NoneGiven => shape.push('N'),
            Control::Proof(proof) => {
                shape.push('P');
                proofs.push(match proof.get() {
                    Ok(proof) => {
                        let deferred = &proof.statement.proof_state.deferred_values;
                        format!(
                            "flags {}, domain_log2 {}",
                            serde_json::to_string(&deferred.plonk.feature_flags)
                                .unwrap_or_default(),
                            deferred.branch_data.domain_log2.as_u8()
                        )
                    }
                    Err(e) => format!("unreadable: {e:?}"),
                });
            }
        });
    (shape, proofs)
}

/// The `Transition` work a snark worker gets for a base job, as
/// `mesa_zkapp_job_fixtures` writes it.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_snark_scan_state/transaction_snark_scan_state.ml#L1334-L1363>
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

fn main() -> Result<()> {
    let args = Args::parse();
    NetworkConfig::init(&args.network).map_err(anyhow::Error::msg)?;
    let constraint_constants = mina_core::constants::constraint_constants();

    let mut block = precomputed::ocaml_json_to_value(&fs::read(&args.parent_block)?)?;
    let mut block = match block.get_mut("data") {
        Some(data) => data.take(),
        None => block,
    };
    precomputed::ocaml_forms_to_ours(&mut block)?;
    let state: MinaStateProtocolStateValueStableV2 =
        serde_json::from_value(block["protocol_state"].take()).context("--parent-block")?;
    let view = protocol_state_view(&state)?;
    let hashes = ProtocolState::try_from(&state)?.hashes();
    let global_slot = Slot::from_u32(args.global_slot.unwrap_or_else(|| {
        state
            .body
            .consensus_state
            .global_slot_since_genesis
            .as_u32()
            + 1
    }));
    let inputs: Inputs = serde_json::from_slice(&fs::read(&args.inputs)?).context("--inputs")?;

    let mut ledger = Mask::new_root(Database::create(constraint_constants.ledger_depth as u8));
    for fund in &inputs.fund {
        let public_key = PubKey::from_address(&fund.public_key)?.into_compressed();
        let id = AccountId::new(public_key, TokenId::default());
        let account = Account::create_with(id.clone(), Balance::from_u64(fund.balance.parse()?));
        ledger
            .get_or_create_account(id, account)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    }

    fs::create_dir_all(&args.out)?;
    for command in &inputs.commands {
        let user_command = UserCommand::try_from(&command.user_command()?)?;
        let (shape, proofs) = describe(&user_command);
        let (jobs, _, _, _) = StagedLedger::update_ledger_and_get_statements(
            constraint_constants,
            global_slot,
            ledger.clone(),
            &Stack::empty(),
            (
                vec![WithStatus {
                    data: Transaction::Command(user_command),
                    status: TransactionStatus::Applied,
                }],
                None,
            ),
            &view,
            hashes,
        )
        .map_err(|e| anyhow::anyhow!("{}: {e:?}", command.name))?;
        ensure!(jobs.len() == 1, "{}: {} jobs", command.name, jobs.len());
        let (job, _) = &jobs[0];
        let spec = transition_spec(job, &state.body);
        let mut bytes = vec![];
        spec.binprot_write(&mut bytes)?;
        let mut statement = vec![];
        spec.0.binprot_write(&mut statement)?;
        let path = args.out.join(format!("{}_base_job.bin", command.name));
        fs::write(&path, &bytes)?;
        println!(
            "{}: shape {shape}, {:?}, statement sha256 {}, {} bytes to {}{}",
            command.name,
            job.transaction_with_status.status,
            hex::encode(Sha256::digest(&statement)),
            bytes.len(),
            path.display(),
            proofs
                .iter()
                .map(|p| format!("\n  proof: {p}"))
                .collect::<String>()
        );
    }
    Ok(())
}
