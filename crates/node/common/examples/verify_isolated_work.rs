//! P5.3: checks SNARK work the web node's isolated prover returned, natively
//! and apart from the browser: the response answers the request, its proofs
//! are of the statements the request asked for, and they verify with the
//! network's transaction verifier key for the sok digest of the request's
//! fee and public key (P5.2's self-check). As a control, the same proofs
//! sold for another fee must fail.
//!
//! ```text
//! cargo run --release -p mina-node-common --example verify_isolated_work -- \
//!     devnet REQUEST RESPONSE [REQUEST RESPONSE ...]
//! ```
//!
//! Prints one JSON line per pair; exits non-zero if any pair fails.

#[cfg(not(target_family = "wasm"))]
fn main() -> anyhow::Result<()> {
    use mina_node::{
        core::snark::Snark,
        snark::{get_srs, TransactionVerifier},
    };
    use mina_node_common::block_producer::isolated::{
        decode_exact, proofs_match_statements, work_kinds, work_statements, WorkRequest,
        WorkResponse, BRIDGE_VERSION, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
    };
    use mina_p2p_messages::v2;

    let args: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        args.len() >= 3 && args.len() % 2 == 1,
        "usage: verify_isolated_work NETWORK REQUEST RESPONSE [REQUEST RESPONSE ...]"
    );
    mina_node::core::NetworkConfig::init(&args[0]).map_err(anyhow::Error::msg)?;
    mina_node::core::init_work_dir_if_unset(|| std::env::temp_dir().join("p53-verifier"));
    let verifier = TransactionVerifier::make();
    let srs = get_srs();
    let mut failed = 0;
    for pair in args[1..].chunks(2) {
        let request_bytes = std::fs::read(&pair[0])?;
        let response_bytes = std::fs::read(&pair[1])?;
        let request: WorkRequest =
            decode_exact(&request_bytes, MAX_REQUEST_BYTES).map_err(anyhow::Error::msg)?;
        let response: WorkResponse =
            decode_exact(&response_bytes, MAX_RESPONSE_BYTES).map_err(anyhow::Error::msg)?;
        let statements = work_statements(&request.spec);
        let answers = response.version == BRIDGE_VERSION
            && request.version == BRIDGE_VERSION
            && response.request_id == request.request_id;
        let statements_match = proofs_match_statements(&response.proofs, &statements);
        let started = std::time::Instant::now();
        let work = Snark {
            snarker: request.prover.clone(),
            fee: request.fee.clone(),
            proofs: response.proofs.clone().into(),
        };
        let verified = ledger::proofs::snark_worker::check_work(&work, &verifier, &srs);
        let verify_ms = started.elapsed().as_millis();
        // Control: the same proofs, sold for one nanomina more.
        let resold = Snark {
            fee: v2::CurrencyFeeStableV1(v2::UnsignedExtendedUInt64Int64ForVersionTagsStableV1(
                (request.fee.0.as_u64() + 1).into(),
            )),
            ..work.clone()
        };
        let control_refused =
            ledger::proofs::snark_worker::check_work(&resold, &verifier, &srs).is_err();
        let ok = answers && statements_match && verified.is_ok() && control_refused;
        failed += usize::from(!ok);
        println!(
            "{}",
            serde_json::json!({
                "request": pair[0],
                "response": pair[1],
                "jobs": work_kinds(&request.spec),
                "fee_nanomina": request.fee.0.as_u64(),
                "prover": request.prover.to_string(),
                "prove_ms": response.prove_ms,
                "answers_request": answers,
                "statements_match": statements_match,
                "verified": verified.is_ok(),
                "error": verified.err().map(|e| e.to_string()),
                "changed_fee_refused": control_refused,
                "verify_ms": verify_ms,
                "ok": ok,
            })
        );
    }
    anyhow::ensure!(failed == 0, "{failed} pairs failed");
    Ok(())
}

#[cfg(target_family = "wasm")]
fn main() {}
