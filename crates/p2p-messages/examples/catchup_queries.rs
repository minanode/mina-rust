//! Build capture queries from an OCaml best-tip response. Prints only public
//! block data, suitable for `mina-rpc-capture --query-hex`.
use binprot::{BinProtRead, BinProtWrite};
use mina_p2p_messages::{list::List, rpc, rpc_kernel::ResponsePayload};

fn hex(value: &impl BinProtWrite) -> String {
    let mut bytes = Vec::new();
    value.binprot_write(&mut bytes).unwrap();
    hex::encode(bytes)
}

fn main() {
    let path = std::env::args().nth(1).expect("best-tip response path");
    let bytes = std::fs::read(path).unwrap();
    let tip = ResponsePayload::<rpc::GetBestTipV2Response>::binprot_read(&mut &bytes[..])
        .unwrap()
        .0
        .unwrap()
        .0
        .unwrap();
    let tip_hash = tip.data.try_hash().unwrap();
    let root_hash = tip.proof.1.try_hash().unwrap();
    let query = rpc::GetAncestryV2Query {
        data: tip.data.header.protocol_state.body.consensus_state.clone(),
        hash: tip_hash.inner().0.clone(),
    };
    let hashes: List<_> = [root_hash.inner().0.clone(), tip_hash.inner().0.clone()]
        .into_iter()
        .collect();
    println!(
        "{}",
        serde_json::json!({
            "ancestry": hex(&query),
            "proof": hex(&tip_hash.inner().0),
            "chain": hex(&hashes),
            "root": root_hash.to_string(),
            "tip": tip_hash.to_string(),
        })
    );
}
