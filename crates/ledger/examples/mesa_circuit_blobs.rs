//! R15 prototype: load circuit blobs dumped by OCaml 4.0.2
//! (`MINA_DUMP_CIRCUIT_DATA=1 MINA_USE_MMAP_CACHE=0`) and check that
//! mina-tree's kimchi (proof-systems ab84160) builds the same verifier indexes
//! from them as OCaml and the live chain.
//!
//! For every circuit it parses `*_gates.json`, builds a prover index with
//! `make_prover_index` and takes its verifier index. Then:
//!
//! - wrap circuits: the whole serialised verifier index (domain,
//!   `max_poly_size`, `zk_rows`, `public`, `prev_challenges`, every commitment
//!   as a full compressed point, shifts, optional gate commitments) must equal
//!   `BlockVerifier::make()` / `TransactionVerifier::make()`, the embedded
//!   Mesa indexes of the network. With a live key file (`data/vk/*-blockchain-vk.json`,
//!   GraphQL format) the blockchain wrap index must also equal it position by
//!   position: 28 points as (x, y), 7 shifts, domain size, `public`,
//!   `prev_challenges`, `zk_rows` and `max_poly_size`;
//! - step circuits: kimchi's own file serialisation of the index
//!   (`VerifierIndex::to_file`, MessagePack, what OCaml's
//!   `Kimchi_bindings.Protocol.VerifierIndex.Fp.write` calls) must be
//!   byte-identical to the body of OCaml's `vk-step-...` key-cache file, and
//!   the OCaml file, read back with `VerifierIndex::from_file`, must equal ours
//!   field by field. As a control, every Rust step index is also compared
//!   position by position with every other circuit's OCaml index.
//!
//! It also checks that every circuit the network's `CircuitsConfig` names is
//! in the directory, and that each circuit's gate count is its `ROWS` in
//! `constants.rs`. Exits non-zero unless every check passes.
//!
//!   cargo run --release -p mina-tree --example mesa_circuit_blobs -- \
//!     [--network mainnet|devnet] <blob-dir> [live-blockchain-vk.json]
//!
//! The network is mainnet unless `--network devnet` is given; the devnet
//! blobs come from `MINA_PROFILE=devnet tools/mesa-circuits/generate.sh`.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use ark_ff::{BigInteger, PrimeField};
use ark_serialize::CanonicalSerialize;
use kimchi::circuits::gate::CircuitGate;
use mina_curves::pasta::{Fp, Fq};
use mina_tree::proofs::{
    constants::{
        ProofConstants, StepBlockProof, StepMergeProof, StepTransactionProof,
        StepZkappOptSignedOptSignedProof, StepZkappOptSignedProof, StepZkappProvedProof,
        WrapBlockProof, WrapTransactionProof,
    },
    field::FieldWitness,
    transaction::make_prover_index,
    verifiers::{BlockVerifier, TransactionVerifier},
    VerifierIndex,
};
use poly_commitment::PolyComm;

#[serde_with::serde_as]
#[derive(serde::Deserialize)]
struct GatesFile<F: ark_ff::PrimeField> {
    public_input_size: usize,
    #[serde_as(as = "Vec<_>")]
    gates: Vec<CircuitGate<F>>,
}

fn read_gates<F: FieldWitness>(path: &Path) -> (usize, Vec<CircuitGate<F>>) {
    let bytes = std::fs::read(path).unwrap();
    let file: GatesFile<F> = serde_json::from_slice(&bytes).unwrap();
    (file.public_input_size, file.gates)
}

fn find(dir: &Path, prefix: &str, suffix: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            let name = p.file_name().unwrap().to_str().unwrap();
            name.starts_with(prefix) && name.ends_with(suffix)
        })
}

/// The commitments in kimchi's order (7 sigma, 15 coefficients, generic, psm,
/// complete_add, mul, emul, endomul_scalar), each chunk as its full compressed
/// encoding (x coordinate and flags byte).
fn comms<F: FieldWitness>(vi: &VerifierIndex<F>) -> Vec<(String, Vec<u8>)> {
    let mut out = vec![];
    let mut push = |name: String, c: &PolyComm<F::OtherCurve>| {
        for (j, p) in c.chunks.iter().enumerate() {
            let mut bytes = vec![];
            p.serialize_compressed(&mut bytes).unwrap();
            out.push((format!("{name}.{j}"), bytes));
        }
    };
    for (i, c) in vi.sigma_comm.iter().enumerate() {
        push(format!("sigma_comm[{i}]"), c);
    }
    for (i, c) in vi.coefficients_comm.iter().enumerate() {
        push(format!("coefficients_comm[{i}]"), c);
    }
    push("generic_comm".into(), &vi.generic_comm);
    push("psm_comm".into(), &vi.psm_comm);
    push("complete_add_comm".into(), &vi.complete_add_comm);
    push("mul_comm".into(), &vi.mul_comm);
    push("emul_comm".into(), &vi.emul_comm);
    push("endomul_scalar_comm".into(), &vi.endomul_scalar_comm);
    out
}

/// Number of commitments equal at the same position, and the larger count.
fn positional<F: FieldWitness>(a: &VerifierIndex<F>, b: &VerifierIndex<F>) -> (usize, usize) {
    let (a, b) = (comms::<F>(a), comms::<F>(b));
    let same = a.iter().zip(b.iter()).filter(|(x, y)| x.1 == y.1).count();
    (same, a.len().max(b.len()))
}

fn scalars<F: FieldWitness>(vi: &VerifierIndex<F>) -> String {
    format!(
        "domain {}, max_poly_size {}, zk_rows {}, public {}, prev_challenges {}",
        vi.domain.size, vi.max_poly_size, vi.zk_rows, vi.public, vi.prev_challenges
    )
}

/// Compare two verifier indexes on every serialised field. Prints the
/// top-level fields that differ.
fn same_serialised<F: FieldWitness>(
    label: &str,
    a: &VerifierIndex<F>,
    b: &VerifierIndex<F>,
) -> bool {
    let (ja, jb) = (
        serde_json::to_value(a).unwrap(),
        serde_json::to_value(b).unwrap(),
    );
    let (oa, ob) = (ja.as_object().unwrap(), jb.as_object().unwrap());
    let differing: Vec<&String> = oa.keys().filter(|k| oa.get(*k) != ob.get(*k)).collect();
    if !differing.is_empty() || oa.len() != ob.len() {
        println!("  {label}: serialised fields differ: {differing:?}");
    }
    ja == jb
}

fn hex_be<F: PrimeField>(x: &F) -> String {
    x.into_bigint()
        .to_bytes_be()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

/// Compare the blockchain wrap verifier index with a live key in the GraphQL
/// `blockchainVerificationKey` format, position by position.
fn compare_live(vi: &VerifierIndex<Fq>, live_path: &Path) -> bool {
    let live: serde_json::Value =
        serde_json::from_slice(&std::fs::read(live_path).unwrap()).unwrap();
    let norm = |s: &str| s.trim_start_matches("0x").to_uppercase();
    let point = |v: &serde_json::Value| -> (String, String) {
        let a = v.as_array().unwrap();
        (norm(a[0].as_str().unwrap()), norm(a[1].as_str().unwrap()))
    };
    let lc = &live["commitments"];
    let mut theirs: Vec<(String, String)> = vec![];
    for fam in ["sigma_comm", "coefficients_comm"] {
        for p in lc[fam].as_array().unwrap() {
            theirs.push(point(p));
        }
    }
    for fam in [
        "generic_comm",
        "psm_comm",
        "complete_add_comm",
        "mul_comm",
        "emul_comm",
        "endomul_scalar_comm",
    ] {
        theirs.push(point(&lc[fam]));
    }
    let mut ours: Vec<(String, String)> = vec![];
    let mut push = |c: &PolyComm<<Fq as FieldWitness>::OtherCurve>| {
        assert_eq!(c.chunks.len(), 1);
        let p = &c.chunks[0];
        ours.push((hex_be(&p.x), hex_be(&p.y)));
    };
    vi.sigma_comm.iter().for_each(&mut push);
    vi.coefficients_comm.iter().for_each(&mut push);
    for c in [
        &vi.generic_comm,
        &vi.psm_comm,
        &vi.complete_add_comm,
        &vi.mul_comm,
        &vi.emul_comm,
        &vi.endomul_scalar_comm,
    ] {
        push(c);
    }
    let points = ours
        .iter()
        .zip(theirs.iter())
        .filter(|(a, b)| a == b)
        .count();

    let idx = &live["index"];
    let live_shifts: Vec<String> = idx["shifts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| norm(s.as_str().unwrap()))
        .collect();
    let our_shifts: Vec<String> = vi.shift.iter().map(hex_be).collect();
    let shifts = our_shifts
        .iter()
        .zip(live_shifts.iter())
        .filter(|(a, b)| a == b)
        .count();
    let log2 = idx["domain"]["log_size_of_group"].as_u64().unwrap();
    let scalars_ok = (1u64 << log2) == vi.domain.size
        && idx["max_poly_size"].as_u64().unwrap() == vi.max_poly_size as u64
        && idx["public"].as_u64().unwrap() == vi.public as u64
        && idx["prev_challenges"].as_u64().unwrap() == vi.prev_challenges as u64
        && idx["zk_rows"].as_u64().unwrap() == vi.zk_rows;
    let ok = points == theirs.len()
        && ours.len() == theirs.len()
        && shifts == live_shifts.len()
        && our_shifts.len() == live_shifts.len()
        && scalars_ok;
    println!(
        "blockchain wrap vs live key {}: {points}/{} points (x, y) equal by position, \
         {shifts}/{} shifts, domain/max_poly_size/public/prev_challenges/zk_rows {} \
         (live: 2^{log2}, {}, {}, {}, {}) -> {}",
        live_path.display(),
        theirs.len(),
        live_shifts.len(),
        if scalars_ok { "equal" } else { "DIFFER" },
        idx["max_poly_size"],
        idx["public"],
        idx["prev_challenges"],
        idx["zk_rows"],
        if ok { "PASS" } else { "FAIL" }
    );
    ok
}

/// Offset of the kimchi body in a key-cache file: after the
/// `MINA_SNARK_KEYS\n` prefix and the JSON header line.
fn body_offset(bytes: &[u8]) -> usize {
    let first = bytes.iter().position(|b| *b == b'\n').unwrap();
    let second = first + 1 + bytes[first + 1..].iter().position(|b| *b == b'\n').unwrap();
    second + 1
}

struct Step {
    name: &'static str,
    ours: VerifierIndex<Fp>,
    ocaml: Option<VerifierIndex<Fp>>,
}

fn step<C: ProofConstants>(dir: &Path, name: &'static str, ok: &mut bool) -> Option<Step> {
    let prefix = format!("step-step-proving-key-{name}-");
    let Some(path) = find(dir, &prefix, "_gates.json") else {
        println!("{name}: no gate file");
        *ok = false;
        return None;
    };
    let (public, gates) = read_gates::<Fp>(&path);
    let n = gates.len();
    let ours = make_prover_index::<C, Fp>(gates, None).verifier_index();

    // kimchi's own serialisation of our index, as OCaml writes it.
    let tmp = std::env::temp_dir().join(format!("r15-rust-vk-step-{name}-{}", std::process::id()));
    std::fs::File::create(&tmp).unwrap();
    ours.to_file(&tmp, Some(true)).unwrap();
    let our_bytes = std::fs::read(&tmp).unwrap();
    let _ = std::fs::remove_file(&tmp);

    let vk_prefix = format!("vk-step-step-verification-key-{name}-");
    let Some(vk_path) = find(dir, &vk_prefix, "") else {
        println!("{name}: no OCaml vk-step file");
        *ok = false;
        return Some(Step {
            name,
            ours,
            ocaml: None,
        });
    };
    let file = std::fs::read(&vk_path).unwrap();
    let offset = body_offset(&file);
    let bytes_equal = file[offset..] == our_bytes[..];
    let ocaml = VerifierIndex::<Fp>::from_file(
        Arc::clone(&ours.srs),
        &vk_path,
        Some(offset as u64),
        ours.endo,
    )
    .unwrap();
    let fields_equal = same_serialised::<Fp>(name, &ours, &ocaml);
    let (same, total) = positional::<Fp>(&ours, &ocaml);
    let pass = bytes_equal && fields_equal && same == total && n == C::ROWS;
    *ok &= pass;
    println!(
        "{name}: gates {n} (ROWS in constants.rs {}), public {public}; {}; \
         kimchi file body {} bytes, {} OCaml's; all serialised fields {}; \
         {same}/{total} commitments equal by position -> {}",
        C::ROWS,
        scalars::<Fp>(&ours),
        our_bytes.len(),
        if bytes_equal {
            "byte-identical to"
        } else {
            "DIFFERENT from"
        },
        if fields_equal { "equal" } else { "DIFFER" },
        if pass { "PASS" } else { "FAIL" }
    );
    Some(Step {
        name,
        ours,
        ocaml: Some(ocaml),
    })
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let network = match args.iter().position(|a| a == "--network") {
        Some(i) => {
            let network = args.get(i + 1).expect("--network needs a name").clone();
            args.drain(i..i + 2);
            network
        }
        None => "mainnet".to_owned(),
    };
    let dir = PathBuf::from(args.first().expect("blob directory"));
    let live = args.get(1).map(PathBuf::from);
    mina_core::NetworkConfig::init(&network).unwrap();
    println!("network {network}");

    // The blobs the network's `CircuitsConfig` names are the ones in the dump.
    let config = mina_core::NetworkConfig::global().circuits_config;
    let mut ok = [
        config.step_transaction_gates,
        config.wrap_transaction_gates,
        config.step_merge_gates,
        config.step_blockchain_gates,
        config.wrap_blockchain_gates,
        config.step_transaction_opt_signed_opt_signed_gates,
        config.step_transaction_opt_signed_gates,
        config.step_transaction_proved_gates,
    ]
    .iter()
    .all(|name| {
        let found = ["_gates.json", "_internal_vars.bin", "_rows_rev.bin"]
            .iter()
            .all(|suffix| dir.join(format!("{name}{suffix}")).exists());
        if !found {
            println!(
                "CircuitsConfig names {name}, which is not in {}",
                dir.display()
            );
        }
        found
    });
    println!(
        "CircuitsConfig ({}): all 8 circuits {}",
        config.directory_name,
        if ok { "found" } else { "NOT found" }
    );

    // Wrap circuits: compare with the network's embedded Mesa verifier indexes
    // and, for the blockchain, with the live key.
    for (prefix, is_block) in [
        ("wrap-wrap-proving-key-blockchain-snark-", true),
        ("wrap-wrap-proving-key-transaction-snark-", false),
    ] {
        let Some(path) = find(&dir, prefix, "_gates.json") else {
            println!("{prefix}: not found");
            ok = false;
            continue;
        };
        let (public, gates) = read_gates::<Fq>(&path);
        let n = gates.len();
        let t = std::time::Instant::now();
        let vi = if is_block {
            make_prover_index::<WrapBlockProof, Fq>(gates, None)
        } else {
            make_prover_index::<WrapTransactionProof, Fq>(gates, None)
        }
        .verifier_index();
        let (block, transaction);
        let (label, embedded): (&str, &VerifierIndex<Fq>) = if is_block {
            block = BlockVerifier::make();
            ("blockchain wrap", &block)
        } else {
            transaction = TransactionVerifier::make();
            ("transaction wrap", &transaction)
        };
        let fields_equal = same_serialised::<Fq>(label, &vi, embedded);
        let (same, total) = positional::<Fq>(&vi, embedded);
        let rows = if is_block {
            WrapBlockProof::ROWS
        } else {
            WrapTransactionProof::ROWS
        };
        let pass = fields_equal && same == total && n == rows;
        ok &= pass;
        println!(
            "{label} ({}): gates {n} (ROWS in constants.rs {rows}), public {public}, index \
             built in {:?}; ours: {}; embedded: {}; all serialised fields {}; {same}/{total} \
             commitments equal by position -> {}",
            path.file_name().unwrap().to_str().unwrap(),
            t.elapsed(),
            scalars::<Fq>(&vi),
            scalars::<Fq>(embedded),
            if fields_equal { "equal" } else { "DIFFER" },
            if pass { "PASS" } else { "FAIL" }
        );
        if is_block {
            if let Some(live) = &live {
                ok &= compare_live(&vi, live);
            }
        }
    }

    // Step circuits: compare with OCaml's key-cache verification keys.
    let mut ok_steps = true;
    let steps: Vec<Step> = [
        step::<StepTransactionProof>(&dir, "transaction-snark-transaction-0", &mut ok_steps),
        step::<StepMergeProof>(&dir, "transaction-snark-merge-1", &mut ok_steps),
        step::<StepZkappOptSignedOptSignedProof>(
            &dir,
            "transaction-snark-opt_signed-opt_signed-2",
            &mut ok_steps,
        ),
        step::<StepZkappOptSignedProof>(&dir, "transaction-snark-opt_signed-3", &mut ok_steps),
        step::<StepZkappProvedProof>(&dir, "transaction-snark-proved-4", &mut ok_steps),
        step::<StepBlockProof>(&dir, "blockchain-snark-step-0", &mut ok_steps),
    ]
    .into_iter()
    .flatten()
    .collect();
    ok &= ok_steps;

    // Control: each Rust step index against every OCaml step index.
    println!("control matrix: commitments equal by position, rows Rust, columns OCaml");
    for a in &steps {
        let row: Vec<String> = steps
            .iter()
            .map(|b| match &b.ocaml {
                Some(o) => {
                    let (same, total) = positional::<Fp>(&a.ours, o);
                    format!("{same:2}/{total}")
                }
                None => "  -  ".into(),
            })
            .collect();
        println!("  {:45} {}", a.name, row.join("  "));
    }

    println!("all checks pass: {ok}");
    if !ok {
        std::process::exit(1);
    }
}
