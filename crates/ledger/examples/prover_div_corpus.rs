//! Offline adapter for tools/prover-div-corpus. No node or publication path.
use std::{fs, io::Cursor, path::Path};

use anyhow::{bail, ensure, Context, Result};
use mina_core::snark::Snark;
use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    v2::{
        self, SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Instances as WorkSpec,
        SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single as SingleSpec,
    },
};
use mina_tree::{
    proofs::{
        snark_worker::{prove_work, WorkProvers},
        verifiers::TransactionVerifier,
    },
    scan_state::scan_state::{
        transaction_snark::{OneOrTwo, SokMessage, Statement},
        AvailableJobMessage,
    },
    verifier::{get_srs, snark_work::verify_snark_works},
};
use serde_json::json;
use sha2::{Digest, Sha256};

type Base = (
    v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement,
    v2::TransactionWitnessStableV2,
);
type Merge = (
    v2::MinaStateSnarkedLedgerStateStableV2,
    v2::LedgerProofProdStableV2,
    v2::LedgerProofProdStableV2,
);

fn decode<T: BinProtRead>(bytes: &[u8]) -> Result<T> {
    let mut reader = Cursor::new(bytes);
    let value = T::binprot_read(&mut reader)?;
    ensure!(
        reader.position() == bytes.len() as u64,
        "trailing input bytes"
    );
    Ok(value)
}

fn encode<T: BinProtWrite>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    value.binprot_write(&mut bytes)?;
    Ok(bytes)
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn statement(single: &SingleSpec) -> &v2::MinaStateSnarkedLedgerStateStableV2 {
    match single {
        SingleSpec::Transition(s, _) => s,
        SingleSpec::Merge(m) => &m.0,
    }
}

fn run(args: Vec<String>, provers_cache: &mut Option<WorkProvers>) -> Result<bool> {
    ensure!(
        args.len() >= 4,
        "usage: prover_div_corpus NETWORK COMMAND INPUT OUTPUT [FORMAT|MESSAGE PROOF]"
    );
    let (network, command, input, output) = (&args[0], &args[1], &args[2], &args[3]);
    ensure!(
        network == mina_core::NetworkConfig::global().name,
        "server network mismatch"
    );
    if command == "new-message" {
        // Only a fresh public address leaves this process; no key file exists.
        let key = mina_signer::Keypair::rand(&mut rand::rngs::OsRng)?;
        fs::write(
            output,
            serde_json::to_vec_pretty(&json!({"fee":"0.001","prover":key.get_address()}))?,
        )?;
        return Ok(true);
    }
    let bytes = fs::read(input).context("read input")?;
    match command.as_str() {
        "normalize" => {
            let singles = match args.get(4).map(String::as_str).unwrap_or("single") {
                "base" => {
                    let (s, w): Base = decode(&bytes)?;
                    vec![SingleSpec::Transition(
                        v2::MinaStateSnarkedLedgerStateStableV2(s),
                        w,
                    )]
                }
                "merge" => vec![SingleSpec::Merge(Box::new(decode::<Merge>(&bytes)?))],
                "single" => vec![decode(&bytes)?],
                "rpc" => {
                    ensure!(bytes.len() >= 8, "missing RPC length");
                    ensure!(
                        u64::from_le_bytes(bytes[..8].try_into()?) == (bytes.len() - 8) as u64,
                        "wrong RPC length"
                    );
                    let response: v2::SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponse =
                        decode(&bytes[8..])?;
                    let (spec, _) = response.0.context("empty RPC response")?;
                    match spec.instances {
                        WorkSpec::One(s) => vec![s],
                        WorkSpec::Two((a, b)) => vec![a, b],
                    }
                }
                "capture" => {
                    #[derive(serde::Deserialize)]
                    struct Capture {
                        job: OneOrTwo<AvailableJobMessage>,
                        protocol_states: Vec<v2::MinaStateProtocolStateValueStableV2>,
                    }
                    let capture: Capture = serde_json::from_slice(&bytes)?;
                    let states = capture
                        .protocol_states
                        .into_iter()
                        .map(|state| Ok((state.try_hash()?.to_string(), state)))
                        .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
                    let jobs = match capture.job {
                        OneOrTwo::One(j) => vec![j],
                        OneOrTwo::Two((a, b)) => vec![a, b],
                    };
                    jobs.into_iter()
                        .map(|job| -> Result<_> {
                            // Same construction as node's available_job_to_snark_worker_spec.
                            // OCaml: https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transaction_snark_scan_state/transaction_snark_scan_state.ml
                            Ok(match job {
                                AvailableJobMessage::Base(base) => {
                                    let state = states
                                        .get(&base.state_hash.0.to_string())
                                        .context("missing protocol state")?;
                                    ensure!(
                                        state.try_hash()? == base.state_hash.0,
                                        "protocol state hash mismatch"
                                    );
                                    SingleSpec::Transition(
                                        base.statement,
                                        v2::TransactionWitnessStableV2 {
                                            transaction: base.transaction_with_status.data,
                                            status: base.transaction_with_status.status,
                                            first_pass_ledger: base.first_pass_ledger_witness,
                                            second_pass_ledger: base.second_pass_ledger_witness,
                                            protocol_state_body: state.body.clone(),
                                            init_stack: base.init_stack,
                                            block_global_slot: base.block_global_slot,
                                        },
                                    )
                                }
                                AvailableJobMessage::Merge { left, right } => {
                                    let a: Statement<()> = (&left.0.statement).try_into()?;
                                    let b: Statement<()> = (&right.0.statement).try_into()?;
                                    let merged = a.merge(&b).map_err(anyhow::Error::msg)?;
                                    SingleSpec::Merge(Box::new(((&merged).into(), left, right)))
                                }
                            })
                        })
                        .collect::<Result<Vec<_>>>()?
                }
                other => bail!("unknown format {other}"),
            };
            fs::create_dir_all(output)?;
            for (i, single) in singles.into_iter().enumerate() {
                let raw = encode(&single)?;
                fs::write(Path::new(output).join(format!("{i}.bin")), &raw)?;
                let stmt = encode(statement(&single))?;
                let value = serde_json::to_value(&single)?;
                let metadata = json!({"network":network,"input_sha256":hash(&raw),"statement_sha256":hash(&stmt),"statement":serde_json::to_value(statement(&single))?,"job":value});
                fs::write(
                    Path::new(output).join(format!("{i}.json")),
                    serde_json::to_vec_pretty(&metadata)?,
                )?;
            }
        }
        "prove" | "verify" => {
            mina_core::init_work_dir_if_unset(|| {
                Path::new(output)
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_path_buf()
            });
            ensure!(args.len() >= 5, "message JSON required");
            let single: SingleSpec = decode(&bytes)?;
            let message_wire: v2::MinaBaseSokMessageStableV1 =
                serde_json::from_slice(&fs::read(&args[4])?)?;
            let message: SokMessage = (&message_wire).try_into()?;
            let verifier = TransactionVerifier::make();
            if command == "prove" {
                let started = std::time::Instant::now();
                if provers_cache.is_none() {
                    *provers_cache = Some(WorkProvers::make(Some(verifier))?);
                }
                let provers = provers_cache.as_ref().unwrap();
                let load_seconds = started.elapsed().as_secs_f64();
                let proved = prove_work(&WorkSpec::One(single), &message, &provers)?;
                let v2::TransactionSnarkWorkTStableV2Proofs::One(proof) = proved.proofs else {
                    bail!("expected one proof")
                };
                fs::write(output, encode(&proof)?)?;
                fs::write(
                    format!("{output}.json"),
                    serde_json::to_vec_pretty(
                        &json!({"load_seconds":load_seconds,"proof_seconds":proved.prove_times[0].as_secs_f64(),"proof_sha256":hash(&encode(&proof)?)}),
                    )?,
                )?;
            } else {
                ensure!(args.len() == 6, "proof file required");
                let proof: v2::LedgerProofProdStableV2 = decode(&fs::read(&args[5])?)?;
                let actual = &proof.0.statement;
                let expected = &statement(&single).0;
                let statement_matches = actual.source == expected.source
                    && actual.target == expected.target
                    && actual.connecting_ledger_left == expected.connecting_ledger_left
                    && actual.connecting_ledger_right == expected.connecting_ledger_right
                    && actual.supply_increase == expected.supply_increase
                    && actual.fee_excess == expected.fee_excess;
                let work = Snark {
                    snarker: message_wire.prover,
                    fee: message_wire.fee,
                    proofs: v2::TransactionSnarkWorkTStableV2Proofs::One(proof).into(),
                };
                // The node also contains verifier panics at its service boundary.
                // Keep the panic visible in the receipt; never count it as acceptance.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    verify_snark_works(&[work], &verifier, &get_srs::<mina_curves::pasta::Fp>())
                }));
                let panicked = result.is_err();
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(e)) => Some(e.to_string()),
                    Err(_) => Some("verifier panicked".to_owned()),
                };
                let accepted = statement_matches && error.is_none();
                fs::write(
                    output,
                    serde_json::to_vec_pretty(
                        &json!({"accepted":accepted,"statement_matches":statement_matches,"verification_panicked":panicked,"verification_error":error}),
                    )?,
                )?;
                if panicked {
                    bail!("verifier panicked; result unavailable");
                }
                if !accepted {
                    return Ok(false);
                }
            }
        }
        _ => bail!("unknown command {command}"),
    }
    Ok(true)
}

fn main() -> Result<()> {
    use std::io::{BufRead, Write};
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(args.len() >= 2, "network and command required");
    mina_core::NetworkConfig::init(&args[0]).map_err(anyhow::Error::msg)?;
    let mut provers = None;
    if args[1] == "serve" {
        for line in std::io::stdin().lock().lines() {
            let request: Vec<String> = serde_json::from_str(&line?)?;
            let result = match run(request, &mut provers) {
                Ok(true) => json!({"exit_code":0}),
                Ok(false) => json!({"exit_code":1}),
                Err(error) => json!({"exit_code":2,"error":format!("{error:#}")}),
            };
            println!("CORPUS_RESULT {result}");
            std::io::stdout().flush()?;
        }
    } else if !run(args, &mut provers)? {
        std::process::exit(1);
    }
    Ok(())
}
