//! Read a precomputed block from the public `mina_network_block_data` bucket,
//! written by an OCaml 4.0.x node, and write it back byte for byte.
//!
//! The block, Mesa mainnet 562564, has a completed work whose sok digest is not
//! UTF-8, fees in decimal MINA, created accounts and a proof. Blocks with zkApp
//! commands need the ledger's hashing, so `tools/block-replay` checks those,
//! and every other held block (`examples/precomputed_blocks.rs`).

use mina_p2p_messages::{
    bigint::BigInt,
    v2::{
        precomputed::PrecomputedHashes, MinaBaseVerificationKeyWireStableV1,
        MinaBaseZkappCommandTStableV1WireStableV1, PrecomputedBlockData,
    },
};

const BLOCK: &str = "precomputed-562564-3NKi1mmaYFxSQp835QYHAFq9r2PWFUskowEqjgUiLU3RpnWCivim.json";

struct NoHashes;

impl PrecomputedHashes for NoHashes {
    fn verification_key(&self, _: &MinaBaseVerificationKeyWireStableV1) -> Result<BigInt, String> {
        Err("the block has no verification key".to_owned())
    }

    fn zkapp_command(
        &self,
        _: &MinaBaseZkappCommandTStableV1WireStableV1,
    ) -> Result<Vec<(BigInt, BigInt)>, String> {
        Err("the block has no zkApp command".to_owned())
    }
}

fn fixture() -> Vec<u8> {
    let path = format!("{}/tests/files/mesa/{BLOCK}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn precomputed_block_round_trip() {
    let bytes = fixture();
    assert!(
        std::str::from_utf8(&bytes).is_err(),
        "the fixture should hold bytes that are not UTF-8"
    );
    let block = PrecomputedBlockData::from_ocaml_json(&bytes).unwrap();
    assert_eq!(block.version, 4);
    assert_eq!(
        block.data.protocol_state.try_hash().unwrap().to_string(),
        "3NKi1mmaYFxSQp835QYHAFq9r2PWFUskowEqjgUiLU3RpnWCivim"
    );
    let written = block.to_ocaml_json(&NoHashes).unwrap();
    if written != bytes {
        let at = bytes
            .iter()
            .zip(&written)
            .position(|(a, b)| a != b)
            .unwrap_or(bytes.len().min(written.len()));
        let around = |b: &[u8]| {
            String::from_utf8_lossy(&b[at.saturating_sub(60)..(at + 60).min(b.len())]).into_owned()
        };
        panic!(
            "written back differently at byte {at}:\n  bucket: {}\n  ours:   {}",
            around(&bytes),
            around(&written)
        );
    }
}
