//! Local probe (not upstream): verify precomputed-block JSON files against a
//! verifier index JSON (kimchi index or OCaml pickles VK) given by MINA_VK_JSON.
//!
//!   MINA_VK_JSON=vk.json cargo run --release -p mina-verify --example verify_precomputed_file -- block.json...
use std::time::Instant;

use mina_verify::Verifier;

fn main() {
    let vk_path = std::env::var("MINA_VK_JSON").expect("set MINA_VK_JSON");
    let vk_json = std::fs::read_to_string(&vk_path).expect("read MINA_VK_JSON");

    let t = Instant::now();
    let verifier = Verifier::with_index_json(&vk_json).expect("load verifier index");
    println!("loaded {vk_path} in {:.2?}", t.elapsed());

    for path in std::env::args().skip(1) {
        let json = std::fs::read_to_string(&path).expect("read block");
        let t = Instant::now();
        match verifier.verify_precomputed_block(&json) {
            Ok(ok) => println!("{path}: verify_block = {ok} ({:.2?})", t.elapsed()),
            Err(e) => println!("{path}: error: {e}"),
        }
    }
}
