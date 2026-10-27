//! Print hex-encoded RPC queries derived from a captured `get_best_tip` response,
//! for use with `mina-rpc-capture --query-hex`. Extra state hashes (base58) get a
//! `get_transition_chain` query each.
//!
//!   cargo run -p mina-p2p-messages --example mesa_rpc_queries -- best_tip.bin [3N...]

use binprot::{BinProtRead, BinProtWrite};
use mina_p2p_messages::{
    rpc::{GetAncestryV2Query, GetBestTipV2Response, WithHashV1},
    rpc_kernel::ResponsePayload,
};

fn hex_of<T: BinProtWrite>(value: &T) -> String {
    let mut bytes = Vec::new();
    value.binprot_write(&mut bytes).unwrap();
    hex::encode(bytes)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: mesa_rpc_queries <best_tip.bin> [state hash...]");
    let bytes = std::fs::read(path).unwrap();
    let payload = ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut &bytes[..]).unwrap();
    let tip = payload.0.unwrap().0.unwrap();
    let block = &tip.data;
    let state_hash = block.header.protocol_state.try_hash().unwrap();
    let parent_hash = block.header.protocol_state.previous_state_hash.clone();
    // The RPC queries carry state hashes as raw field elements (`StateHashV1`).
    let raw = |hash: &mina_p2p_messages::v2::StateHash| hash.clone().into_inner().0;

    println!("tip_state_hash {state_hash}");
    println!("tip_state_hash_hex {}", hex_of(&state_hash));
    println!("parent_state_hash_hex {}", hex_of(&parent_hash));
    println!(
        "transition_chain_query_hex {}",
        hex_of(&mina_p2p_messages::list::List::from_iter([raw(
            &parent_hash
        )]))
    );
    let ancestry: GetAncestryV2Query = WithHashV1 {
        data: block.header.protocol_state.body.consensus_state.clone(),
        hash: raw(&state_hash),
    };
    println!("ancestry_query_hex {}", hex_of(&ancestry));

    for hash in args {
        let hash: mina_p2p_messages::v2::StateHash = hash.parse().expect("base58 state hash");
        println!(
            "transition_chain_query_hex_for {hash} {}",
            hex_of(&mina_p2p_messages::list::List::from_iter([raw(&hash)]))
        );
    }
}
