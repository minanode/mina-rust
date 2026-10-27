//! R11 prototype: time the steps of building the block and transaction
//! verifier indexes from their embedded JSON, time loading them from the
//! postcard cache instead, and write the cache files a web build would serve
//! from `assets/webnode/circuit-blobs/<circuits directory>/`.
//!
//!   cargo run --release -p mina-tree --example verifier_index_cache -- \
//!       mainnet <output dir>
//!
//! The output dir is used as `$HOME`, so the cache files end up in
//! `<output dir>/.mina/circuit-blobs/<circuits directory>/`. Set
//! `RAYON_NUM_THREADS=1` to time the single-threaded case.

use std::time::Instant;

use kimchi::mina_curves::pasta::{Pallas, Vesta};
use mina_curves::pasta::Fq;
use mina_tree::proofs::{
    caching::{srs_from_bytes, srs_to_bytes, verifier_index_from_bytes},
    verifiers::{BlockVerifier, TransactionVerifier},
    VerifierIndex,
};
use poly_commitment::{ipa::SRS, SRS as _};
use sha2::{Digest, Sha256};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let mut args = std::env::args().skip(1);
    let network = args.next().unwrap_or_else(|| "mainnet".to_owned());
    let out = args.next().expect("output dir");
    std::fs::create_dir_all(&out).unwrap();
    std::env::set_var("HOME", &out);
    mina_core::NetworkConfig::init(&network).unwrap();
    let dir = mina_core::NetworkConfig::global()
        .circuits_config
        .directory_name;
    println!(
        "network {network}, circuits directory {dir}, rayon threads {}",
        rayon::current_num_threads()
    );

    let block_json = match network.as_str() {
        "mainnet" => include_str!("../src/proofs/data/mainnet_blockchain_verifier_index.json"),
        _ => include_str!("../src/proofs/data/devnet_blockchain_verifier_index.json"),
    };

    // The steps of `make_verifier_index`, timed one by one.
    let t = Instant::now();
    let index: VerifierIndex<Fq> = serde_json::from_str(block_json).unwrap();
    println!("parse json                        {:8.1} ms", ms(t));
    println!(
        "  max_poly_size {}, domain size {}",
        index.max_poly_size, index.domain.size
    );
    let t = Instant::now();
    let srs = SRS::<Pallas>::create(index.max_poly_size);
    println!("SRS::<Pallas>::create             {:8.1} ms", ms(t));
    let t = Instant::now();
    let _ = srs.get_lagrange_basis(index.domain);
    println!("get_lagrange_basis                {:8.1} ms", ms(t));
    let t = Instant::now();
    let _ = srs.get_lagrange_basis(index.domain);
    println!("get_lagrange_basis, second call   {:8.1} ms", ms(t));

    // The full path: a cache miss in an empty $HOME, then a write.
    let t = Instant::now();
    let block = BlockVerifier::make();
    println!("BlockVerifier::make (miss, write) {:8.1} ms", ms(t));
    let t = Instant::now();
    let tx = TransactionVerifier::make();
    println!("TransactionVerifier::make         {:8.1} ms", ms(t));

    // Load the files just written, as the web node does on a hit.
    for name in ["block_verifier_index", "transaction_verifier_index"] {
        let path = format!("{out}/.mina/circuit-blobs/{dir}/{name}.postcard");
        let bytes = std::fs::read(&path).unwrap();
        let t = Instant::now();
        let mut hasher = Sha256::new();
        hasher.update(&bytes[64..]);
        let ok = hasher.finalize().as_slice() == &bytes[32..64];
        let loaded = verifier_index_from_bytes(&bytes[64..]).unwrap();
        println!(
            "load {name:27} {:8.1} ms ({} bytes, digest ok {ok}, srs {})",
            ms(t),
            bytes.len(),
            loaded.srs.g.len()
        );
    }
    println!(
        "block and transaction indexes share an SRS shape: {}",
        block.max_poly_size == tx.max_poly_size && block.domain.size == tx.domain.size
    );

    // The proof-checking SRS that `NodeBuilder::build` creates (`get_srs`).
    let t = Instant::now();
    let vesta = SRS::<Vesta>::create(65536);
    println!("SRS::<Vesta>::create(65536)       {:8.1} ms", ms(t));
    let bytes = srs_to_bytes(&vesta);
    let t = Instant::now();
    let back: SRS<Vesta> = srs_from_bytes(&bytes);
    println!(
        "srs_from_bytes Vesta              {:8.1} ms ({} bytes, equal {})",
        ms(t),
        bytes.len(),
        back.g == vesta.g && back.h == vesta.h
    );
}
