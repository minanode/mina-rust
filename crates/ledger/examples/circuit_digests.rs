//! Recomputes OCaml's digest of each circuit in a blob directory and checks it
//! against the digest OCaml's key cache put in the file name
//! (`...-<digest>_gates.json`). `fork_circuits` makes the same check before it
//! uses a runtime fork's blockchain circuits.
//!
//!   cargo run --release -p mina-tree --example circuit_digests -- <blob-dir>
//!
//! `<blob-dir>` holds one fork's 8 circuits, e.g. the `4.0.2devnet` directory
//! that `tools/mesa-circuits/generate.sh` links. Prints one JSON line; exits 1
//! unless every digest matches its name.
//!
//! These are not the chain id's digests: the chain id hashes the digests of
//! the statement circuits inside the Pickles step circuits, which are not
//! among the blobs (`scripts/ocaml-chain-id-trace.sh` reads them from OCaml).

use std::path::PathBuf;

use mina_curves::pasta::{Fp, Fq};
use mina_tree::proofs::fork_circuits::{digest_in_name, gates_file_digest};

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("blob directory"));
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with("_gates.json"))
        .collect();
    entries.sort();
    let mut all_match = true;
    let mut circuits = serde_json::Map::new();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let stem = name.trim_end_matches("_gates.json");
        let started = std::time::Instant::now();
        // Step circuits are over Fp (Vesta), wrap circuits over Fq (Pallas).
        let digest = if stem.starts_with("wrap-") {
            gates_file_digest::<Fq>(&path)
        } else {
            gates_file_digest::<Fp>(&path)
        }
        .unwrap();
        let matches = Some(digest) == digest_in_name(stem);
        all_match &= matches;
        circuits.insert(
            stem.to_owned(),
            serde_json::json!({
                "digest": hex::encode(digest),
                "matches_name": matches,
                "seconds": started.elapsed().as_secs_f64(),
            }),
        );
    }
    println!(
        "{}",
        serde_json::json!({
            "dir": dir.display().to_string(),
            "circuits": circuits,
            "all_match_names": all_match,
        })
    );
    if !all_match || circuits.is_empty() {
        std::process::exit(1);
    }
}
