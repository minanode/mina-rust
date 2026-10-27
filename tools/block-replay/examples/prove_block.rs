//! Prove a block with the Rust block prover, as a block producer does, check
//! the proof with the network's blockchain verifier (the live Mesa key;
//! mainnet unless `--network devnet`), and write it in the JSON of OCaml's
//! blockchain proofs:
//!
//!   MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs \
//!     cargo run --release -p mina-block-replay --example prove_block -- \
//!       --input crates/ledger/src/proofs/data/mesa_mainnet_genesis_block_input.bin \
//!       --out genesis-blockchain.json
//!
//! The input is a `ProverExtendBlockchainInputStableV2` in binprot (for the
//! genesis block, `mesa_genesis_block_input` writes one). The output is one
//! line, a list of one `Blockchain_snark.Blockchain.t`, `{"state", "proof"}`:
//! the new protocol state and its proof, as OCaml's
//! `internal test-genesis-block-generation` prints a block and
//! `internal run-verifier --mode blockchain` parses a list of them. In that JSON
//! a proof is the base64 of its S-expression (`Pickles.Proof.to_yojson`), not
//! the base64 binprot of precomputed blocks.
//!
//! With `--repeat <n>` the input is proved n times in one process, each proof
//! checked: a stress test for intermittent failures. A failed attempt leaves
//! next to `--out` kimchi's error with its witness
//! (`<out stem>.attempt-<i>.kimchi-error.bin`, as the node dumps it) or the
//! proof the live key rejected (`<out stem>.attempt-<i>.invalid.json`), and
//! the run fails at the end.
//!
//! Two more modes, which prove nothing:
//!
//! - `--precomputed <block.json[.zst]> --out <file>` writes a precomputed
//!   block's own state and proof in the same form;
//! - `--verify <file>` reads such a list (one block, as OCaml prints it, or a
//!   list) and checks each proof for its state with the network's blockchain
//!   verifier.
//!
//! OCaml 4.0.2's `run-verifier --mode blockchain` parses this JSON, ours and
//! its own, but rejects every blockchain proof it was given in P4.5 with
//! `dlog_check`, its own ones included (a live mainnet block, and genesis
//! blocks it proved with and without the Mesa fork constants), so it is no
//! oracle for block proofs.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/cli_entrypoint/mina_cli_entrypoint.ml#L1869-L1984>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/crypto/pickles/proof.ml#L331-L353>

use std::{io::Read, path::PathBuf, time::Instant};

use anyhow::{bail, Context};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE},
    Engine as _,
};
use clap::Parser;
use ledger::proofs::{
    block::BlockParams,
    generate_block_proof,
    provers::BlockProver,
    verification::verify_block_proof,
    verifiers::{BlockVerifier, TransactionVerifier},
};
use mina_p2p_messages::{binprot::BinProtRead, v2};
use serde_json::Value;

#[derive(Parser)]
struct Args {
    /// The block prover's input, in binprot: prove it.
    #[arg(long)]
    input: Option<PathBuf>,
    /// A precomputed block: write its own state and proof.
    #[arg(long)]
    precomputed: Option<PathBuf>,
    /// A block or a list of blocks in OCaml's JSON: verify them.
    #[arg(long)]
    verify: Option<PathBuf>,
    /// Where to write the state and its proof, with `--input` or
    /// `--precomputed`.
    #[arg(long)]
    out: Option<PathBuf>,
    /// With `--input`: prove it this many times in this process, checking
    /// each proof, and write the last. A stress test for intermittent
    /// failures: each failed attempt is reported, and the run fails if any
    /// did.
    #[arg(long, default_value_t = 1)]
    repeat: usize,
    /// The network whose circuits and blockchain key are used.
    #[arg(long, default_value = "mainnet")]
    network: String,
}

/// How an attempt to prove a block failed, as the stress test records it.
enum Failure {
    /// The prover returned an error: kimchi's, when it is one.
    Prover(anyhow::Error),
    /// A proof came out, but the live key rejects it.
    Invalid(Box<v2::MinaBaseProofStableV2>),
}

/// `Pickles.Proof.to_yojson`: the base64 (standard alphabet) of the proof's
/// S-expression.
fn proof_to_yojson(proof: &v2::MinaBaseProofStableV2) -> Value {
    let sexp = rsexp::SexpOf::sexp_of(&proof.0);
    Value::String(STANDARD.encode(sexp.to_bytes_mach()))
}

/// `Pickles.Proof.of_yojson`.
fn proof_of_yojson(json: &Value) -> anyhow::Result<v2::MinaBaseProofStableV2> {
    let base64 = json.as_str().context("a proof is a base64 string")?;
    let sexp = rsexp::from_slice(&STANDARD.decode(base64)?)
        .map_err(|e| anyhow::anyhow!("S-expression: {e:?}"))?;
    let proof = <v2::PicklesProofProofsVerified2ReprStableV2 as rsexp::OfSexp>::of_sexp(&sexp)
        .map_err(|e| anyhow::anyhow!("proof: {e}"))?;
    Ok(v2::MinaBaseProofStableV2(proof))
}

/// The `[Blockchain_snark.Blockchain.t]` that `run-verifier` reads.
fn blockchain_json(state: Value, proof: &v2::MinaBaseProofStableV2) -> String {
    let blockchain = serde_json::json!([{ "state": state, "proof": proof_to_yojson(proof) }]);
    format!("{blockchain}\n")
}

/// Whether the network's blockchain verifier accepts `proof` for `state`, as
/// the node checks a block it receives and its own block proof.
fn verifies(
    state: &v2::MinaStateProtocolStateValueStableV2,
    proof: &v2::MinaBaseProofStableV2,
    verifier: &BlockVerifier,
) -> bool {
    verify_block_proof(state, proof, verifier)
}

fn precomputed(path: &PathBuf) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    if path.extension().is_some_and(|e| e == "zst") {
        zstd::Decoder::new(std::fs::File::open(path)?)?.read_to_end(&mut bytes)?;
    } else {
        bytes = std::fs::read(path)?;
    }
    let mut block: Value = serde_json::from_slice(&bytes)?;
    let mut block = match block.get_mut("data") {
        Some(data) => data.take(),
        None => block,
    };
    // A precomputed block's proof is the uri-safe base64 of its binprot.
    let proof = block["protocol_state_proof"]
        .as_str()
        .context("protocol_state_proof is a string")?;
    let proof = v2::MinaBaseProofStableV2::binprot_read(&mut URL_SAFE.decode(proof)?.as_slice())?;
    Ok(blockchain_json(block["protocol_state"].take(), &proof))
}

/// Proves `input` once, and checks the proof with the live key.
fn prove_once(
    input: &v2::ProverExtendBlockchainInputStableV2,
    provers: &BlockProver,
    verifier: &BlockVerifier,
) -> Result<v2::MinaBaseProofStableV2, Failure> {
    let started = Instant::now();
    let proof = generate_block_proof(BlockParams {
        input,
        block_step_prover: &provers.block_step_prover,
        block_wrap_prover: &provers.block_wrap_prover,
        tx_wrap_prover: &provers.tx_wrap_prover,
        only_verify_constraints: false,
        expected_step_proof: None,
        ocaml_wrap_witness: None,
    })
    .map_err(Failure::Prover)?;
    println!("proved, step and wrap, in {:.1?}", started.elapsed());
    let proof = v2::MinaBaseProofStableV2((&proof).into());
    if !verifies(&input.next_state, &proof, verifier) {
        return Err(Failure::Invalid(Box::new(proof)));
    }
    println!(
        "verified with the {} blockchain key",
        mina_core::NetworkConfig::global().name
    );
    Ok(proof)
}

/// Where a failed attempt's record goes, next to `out`:
/// `<out's stem>.attempt-<attempt>.<suffix>`, one file per attempt.
/// (`Path::with_extension` would replace `attempt-<attempt>` with the
/// suffix, so every attempt would write the same file.)
fn failure_path(out: &std::path::Path, attempt: usize, suffix: &str) -> PathBuf {
    let stem = out.file_stem().unwrap_or_default().to_string_lossy();
    out.with_file_name(format!("{stem}.attempt-{attempt}.{suffix}"))
}

/// Writes what a failed attempt left, next to `out`, and returns a line
/// saying what failed: kimchi's error and its witness (`KimchiProofError`,
/// in binprot as the node dumps it) for a prover error, the proof for an
/// invalid one.
fn record_failure(
    failure: &Failure,
    state: &v2::MinaStateProtocolStateValueStableV2,
    out: &std::path::Path,
    attempt: usize,
) -> anyhow::Result<String> {
    use ledger::proofs::transaction::ProofError;
    use mina_p2p_messages::binprot::BinProtWrite;

    match failure {
        Failure::Prover(error) => {
            let mut line = format!("prover error: {error:#}");
            let kimchi = error
                .chain()
                .find_map(|e| match e.downcast_ref::<ProofError>() {
                    Some(ProofError::ProvingErrorWithContext(context)) => Some(context),
                    _ => None,
                });
            if let Some(context) = kimchi {
                line = format!("kimchi error: {}", context.inner_error);
                let mut bytes = Vec::new();
                context.binprot_write(&mut bytes)?;
                let path = failure_path(out, attempt, "kimchi-error.bin");
                std::fs::write(&path, bytes)?;
                line = format!("{line} ({})", path.display());
            }
            Ok(line)
        }
        Failure::Invalid(proof) => {
            let path = failure_path(out, attempt, "invalid.json");
            std::fs::write(&path, blockchain_json(serde_json::to_value(state)?, proof))?;
            Ok(format!(
                "the proof does not verify with the {} blockchain key: {}",
                mina_core::NetworkConfig::global().name,
                path.display()
            ))
        }
    }
}

fn prove(path: &PathBuf, repeat: usize, out: &std::path::Path) -> anyhow::Result<String> {
    if std::env::var_os("MINA_CIRCUIT_BLOBS_BASE_DIR").is_none() {
        bail!("set MINA_CIRCUIT_BLOBS_BASE_DIR to the Mesa circuit blobs (tools/mesa-circuits)");
    }
    let bytes = std::fs::read(path).with_context(|| path.display().to_string())?;
    let input = v2::ProverExtendBlockchainInputStableV2::binprot_read(&mut bytes.as_slice())?;

    let started = Instant::now();
    let verifier = BlockVerifier::make();
    let provers = BlockProver::make(Some(verifier.clone()), Some(TransactionVerifier::make()))?;
    println!("provers loaded in {:.1?}", started.elapsed());

    let state = &input.next_state;
    let (mut last, mut failed) = (None, 0);
    for attempt in 1..=repeat {
        match prove_once(&input, &provers, &verifier) {
            Ok(proof) => {
                println!("attempt {attempt}: ok");
                last = Some(proof);
            }
            Err(failure) => {
                failed += 1;
                println!(
                    "attempt {attempt}: FAILED: {}",
                    record_failure(&failure, state, out, attempt)?
                );
            }
        }
    }
    if failed > 0 {
        bail!("{failed} of {repeat} attempts failed");
    }
    let proof = last.context("no attempt")?;
    Ok(blockchain_json(serde_json::to_value(state)?, &proof))
}

fn verify(path: &PathBuf) -> anyhow::Result<()> {
    let json: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let blocks = match json {
        Value::Array(blocks) => blocks,
        block => vec![block],
    };
    let verifier = BlockVerifier::make();
    let mut failed = 0;
    for (i, block) in blocks.iter().enumerate() {
        let state: v2::MinaStateProtocolStateValueStableV2 =
            serde_json::from_value(block["state"].clone()).context("state")?;
        let proof = proof_of_yojson(&block["proof"])?;
        let ok = verifies(&state, &proof, &verifier);
        println!(
            "block {i}: height {}, {}",
            state.body.consensus_state.blockchain_length.as_u32(),
            if ok { "verifies" } else { "does not verify" }
        );
        failed += usize::from(!ok);
    }
    if failed > 0 {
        bail!("{failed} of {} proofs do not verify", blocks.len());
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    mina_core::NetworkConfig::init(&args.network).map_err(anyhow::Error::msg)?;
    // `verify_block` dumps the header of a proof that fails to `debug/` in
    // the work dir.
    mina_core::set_work_dir(std::env::temp_dir().join("mina-prove-block"));
    let json = match (&args.input, &args.precomputed, &args.verify) {
        (Some(input), None, None) => {
            prove(input, args.repeat, args.out.as_ref().context("--out")?)?
        }
        (None, Some(block), None) => precomputed(block)?,
        (None, None, Some(blocks)) => return verify(blocks),
        _ => bail!("give one of --input, --precomputed and --verify"),
    };
    let out = args.out.context("--out")?;
    std::fs::write(&out, json)?;
    println!("wrote {}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::failure_path;

    /// Each attempt's record has its own file, which keeps the attempt
    /// number: `/tmp/stress/548160.json` used to give
    /// `/tmp/stress/548160.kimchi-error.bin` for every attempt.
    #[test]
    fn each_failed_attempt_has_its_own_file() {
        let out = Path::new("/tmp/stress/548160.json");
        assert_eq!(
            failure_path(out, 3, "kimchi-error.bin"),
            Path::new("/tmp/stress/548160.attempt-3.kimchi-error.bin")
        );
        assert_eq!(
            failure_path(out, 10, "invalid.json"),
            Path::new("/tmp/stress/548160.attempt-10.invalid.json")
        );
        assert_ne!(
            failure_path(out, 1, "kimchi-error.bin"),
            failure_path(out, 2, "kimchi-error.bin")
        );
        assert_eq!(
            failure_path(Path::new("548160"), 1, "invalid.json"),
            Path::new("548160.attempt-1.invalid.json")
        );
    }
}
