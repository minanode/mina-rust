//! Verify saved gossip payloads and print each block's facts as JSON.
//!
//!   cargo run --release -p mina-web-verifier --example verify_gossip -- \
//!     data/vk/mainnet-mesa-blockchain-vk.json payload-0.bin payload-1.bin

use std::time::Instant;

use mina_web_verifier::{ForkChoice, Verifier};

fn main() {
    let mut args = std::env::args().skip(1);
    let vk_path = args
        .next()
        .expect("usage: verify_gossip <vk.json> <payload>...");
    let vk_json = std::fs::read_to_string(&vk_path).expect("read verification key");
    let verifier = Verifier::from_vk_json(&vk_json).expect("load verification key");

    let mut fork_choice = ForkChoice::default();
    for path in args {
        let payload = std::fs::read(&path).expect("read payload");
        let started = Instant::now();
        match verifier.verify_gossip(&payload) {
            Ok(block) => {
                let best = fork_choice.consider(&block);
                println!(
                    "{path}: {} ({:.2?}){}",
                    serde_json::to_string(&block.facts).unwrap(),
                    started.elapsed(),
                    if best { " best tip" } else { "" }
                );
            }
            Err(e) => println!("{path}: {e}"),
        }
    }
    if let Some(best) = fork_choice.best() {
        println!(
            "best tip: {} at height {}",
            best.facts.state_hash, best.facts.height
        );
    }
}
