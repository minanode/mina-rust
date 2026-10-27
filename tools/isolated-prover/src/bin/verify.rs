#[cfg(not(target_family = "wasm"))]
fn main() -> anyhow::Result<()> {
    use ledger::proofs::verifiers::{BlockVerifier, TransactionVerifier};
    let args: Vec<_> = std::env::args_os().collect();
    mina_core::NetworkConfig::init("mainnet").map_err(anyhow::Error::msg)?;
    if args.len() == 2 && args[1] == "--sizes" {
        return print_sizes();
    }
    if args.len() == 4 && args[1] == "--bridge" {
        mina_core::init_work_dir_if_unset(|| std::env::temp_dir().join("i34c-verifier"));
        let request = std::fs::read(&args[2])?;
        let response = std::fs::read(&args[3])?;
        let checked =
            mina_isolated_prover::verify_bridge(&request, &response, &BlockVerifier::make())?;
        println!(
            "{}",
            serde_json::json!({
                "verified": true,
                "request_id": checked.request_id,
                "block_hash": checked.block_hash.to_string(),
                "is_genesis": checked.is_genesis,
                "height": checked.input.next_state.body.consensus_state.blockchain_length.as_u32(),
                "global_slot": checked.input.next_state.body.consensus_state
                    .curr_global_slot_since_hard_fork.slot_number.as_u32(),
                "has_ledger_proof": checked.input.ledger_proof.is_some(),
                "request_bytes": request.len(),
                "response_bytes": response.len(),
                "controls": "previous state and tampered genesis hash rejected; no key in request",
            })
        );
        return Ok(());
    }
    if args.len() == 4 && args[1] == "--block" {
        mina_core::init_work_dir_if_unset(|| std::env::temp_dir().join("i34b-verifier"));
        let input = std::fs::read(&args[2])?;
        let proof = std::fs::read(&args[3])?;
        mina_isolated_prover::verify_block(&input, &proof, &BlockVerifier::make())?;
        println!(
            "mainnet block verification passed; previous state and tampered genesis hash rejected; input={} proof={} bytes",
            input.len(), proof.len()
        );
        return Ok(());
    }
    anyhow::ensure!(
        args.len() == 3,
        "usage: verify-isolated-proof [--block] INPUT PROOF | --bridge REQUEST RESPONSE | --sizes"
    );
    mina_core::init_work_dir_if_unset(|| std::env::temp_dir().join("i34-verifier"));
    let verifier = TransactionVerifier::make();
    let input = std::fs::read(&args[1])?;
    let proof = std::fs::read(&args[2])?;
    mina_isolated_prover::verify(&input, &proof, &verifier)?;
    println!(
        "mainnet verification passed; changed SOK rejected; input={} proof={} bytes",
        input.len(),
        proof.len()
    );
    Ok(())
}

#[cfg(target_family = "wasm")]
fn main() {}

/// Sizes from public fixtures only. The sparse ledger is a payment ledger, not
/// a captured block-production stake witness; the pending tree is empty.
#[cfg(not(target_family = "wasm"))]
fn print_sizes() -> anyhow::Result<()> {
    use mina_p2p_messages::{
        binprot::{BinProtRead, BinProtWrite},
        v2,
    };
    fn size(value: &impl BinProtWrite) -> usize {
        let mut bytes = Vec::new();
        value.binprot_write(&mut bytes).unwrap();
        bytes.len()
    }
    let gossip =
        include_bytes!("../../../../crates/web-verifier/tests/fixtures/mainnet-563978.gossipbin");
    anyhow::ensure!(gossip[8] == 0, "expected public NewState fixture");
    let header = v2::MinaBlockHeaderStableV2::binprot_read(&mut &gossip[9..])?;
    let fixture = include_bytes!(
        "../../../../crates/ledger/src/proofs/data/mesa_mainnet_payment_base_job.bin"
    );
    let (statement, witness) =
        mina_isolated_prover::decode::<mina_isolated_prover::Input>(fixture)?;
    let mut pending = ledger::scan_state::pending_coinbase::PendingCoinbase::create(5);
    pending.merkle_root();
    let pending: v2::MinaBasePendingCoinbaseStableV2 = (&pending).into();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "public_block_height": 563978,
            "protocol_state_bytes": size(&header.protocol_state),
            "blockchain_proof_bytes": size(&header.protocol_state_proof),
            "blockchain_state_bytes": size(&header.protocol_state.body.blockchain_state),
            "payment_statement_bytes": size(&statement),
            "payment_first_pass_sparse_ledger_bytes": size(&witness.first_pass_ledger),
            "payment_second_pass_sparse_ledger_bytes": size(&witness.second_pass_ledger),
            "empty_depth_5_pending_coinbase_bytes": size(&pending),
            "payment_input_bytes": fixture.len(),
        }))?
    );
    Ok(())
}
