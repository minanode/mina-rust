//! Convert a verification key from the OCaml node's JSON form into the verifier
//! index JSON that mina-tree embeds (`crates/ledger/src/proofs/data/`).
//!
//!   cargo run --release -p mina-web-verifier --example export_verifier_index -- \
//!     data/vk/mainnet-mesa-blockchain-vk.json out.json

use serde::Serialize;

fn main() {
    let mut args = std::env::args().skip(1);
    let (input, output) = match (args.next(), args.next()) {
        (Some(input), Some(output)) => (input, output),
        _ => panic!("usage: export_verifier_index <vk.json> <out.json>"),
    };
    let json = std::fs::read_to_string(&input).expect("read verification key");
    let index = mina_web_verifier::verifier_index_auto(&json).expect("parse verification key");

    // mina-tree's embedded indexes are pretty-printed with four-space indents.
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    index.serialize(&mut serializer).expect("serialize index");
    out.push(b'\n');
    std::fs::write(&output, out).expect("write index");
}
