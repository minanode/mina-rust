//! Verify precomputed blocks against one verification key and print, per block,
//! the height, the protocol state hash we compute, the protocol version and
//! genesis state hash it carries, and whether its proof verifies.
//!
//!   cargo run --release -p mina-web-verifier --example verify_blocks -- \
//!     data/vk/mainnet-mesa-blockchain-vk.json mainnet-548148-3NKtYZ….json …
//!
//! The key may be the OCaml JSON form or a mina-tree verifier index JSON.

use std::time::Instant;

use mina_web_verifier::{header_from_precomputed, Verifier};

fn main() {
    let mut args = std::env::args().skip(1);
    let vk_path = args
        .next()
        .expect("usage: verify_blocks <vk.json> <block.json>...");
    let vk_json = std::fs::read_to_string(&vk_path).expect("read verification key");
    let verifier = Verifier::from_vk_json(&vk_json).expect("load verification key");
    verifier.warm_up();

    for path in args {
        let json = std::fs::read_to_string(&path).expect("read block");
        let value: serde_json::Value = serde_json::from_str(&json).expect("parse block");
        let data = value.get("data").unwrap_or(&value);
        let version = data
            .get("protocol_version")
            .map(|v| format!("{}.{}.{}", v["transaction"], v["network"], v["patch"]))
            .unwrap_or_else(|| "?".to_string());

        let header = match header_from_precomputed(&json) {
            Ok(header) => header,
            Err(e) => {
                println!("{path}: {e}");
                continue;
            }
        };
        let state = &header.protocol_state;
        let hash = header
            .try_hash()
            .map(|h| h.to_string())
            .unwrap_or_else(|e| format!("hash error {e:?}"));
        let started = Instant::now();
        let ok = verifier.verify_header(&header);
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        println!(
            "{name}: height={} version={version} hash={hash} hash_in_name={} \
             genesis={} verified={ok} ({:.2?})",
            state.body.consensus_state.blockchain_length.as_u32(),
            name.contains(&hash),
            state.body.genesis_state_hash,
            started.elapsed(),
        );
    }
}
