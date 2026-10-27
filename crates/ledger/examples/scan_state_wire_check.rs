//! Compare full aux captures and verify the scan-state codec on both snapshots.
//! Pending-coinbase index lists are compared as maps, as in the node fixture.

use std::{fs, io::BufReader};

use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    rpc,
    rpc_kernel::ResponsePayload,
    v2,
};
use mina_tree::scan_state::scan_state::ScanState;
use sha2::{Digest, Sha256};

fn encode(value: &impl BinProtWrite) -> Vec<u8> {
    let mut out = Vec::new();
    value.binprot_write(&mut out).unwrap();
    out
}

fn main() {
    mina_core::NetworkConfig::init("devnet").unwrap();
    type Aux = ResponsePayload<rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response>;
    let mut input = std::env::args().skip(1);
    let left = fs::read(input.next().expect("first aux capture")).unwrap();
    let right = fs::read(input.next().expect("second aux capture")).unwrap();
    let decode = |bytes: &[u8]| {
        let aux = Aux::binprot_read(&mut BufReader::new(bytes)).unwrap();
        assert!(encode(&aux) == bytes, "aux wire codec changed bytes");
        aux.0.unwrap().0.unwrap()
    };
    let (left_scan, left_hash, mut left_pending, left_states) = decode(&left);
    let (right_scan, right_hash, mut right_pending, right_states) = decode(&right);
    let scan_bytes = encode(&left_scan);
    let right_scan_bytes = encode(&right_scan);
    if scan_bytes != right_scan_bytes {
        eprintln!(
            "scan byte lengths: {} / {}",
            scan_bytes.len(),
            right_scan_bytes.len()
        );
    }
    assert!(scan_bytes == right_scan_bytes, "scan states differ");
    for wire in [&left_scan, &right_scan] {
        let scan = ScanState::try_from(wire).unwrap();
        assert!(
            encode(&v2::TransactionSnarkScanStateStableV2::from(&scan)) == scan_bytes,
            "internal scan-state conversion changed bytes"
        );
    }
    assert_eq!(left_hash, right_hash, "ledger hashes differ");
    assert!(
        encode(&left_states) == encode(&right_states),
        "states differ"
    );
    for pending in [&mut left_pending, &mut right_pending] {
        let mut indexes = pending.tree.indexes.iter().cloned().collect::<Vec<_>>();
        indexes.sort_by_key(|(id, _)| id.0.as_u64());
        pending.tree.indexes = indexes.into_iter().collect();
    }
    assert!(
        encode(&left_pending) == encode(&right_pending),
        "pending coinbase maps differ"
    );
    println!(
        "{}",
        serde_json::json!({
            "full_aux_byte_identical":left==right,
            "scan_byte_identical":true,
            "internal_codec_byte_identical":true,
            "scan_bytes":scan_bytes.len(),
            "scan_sha256":hex::encode(Sha256::digest(&scan_bytes)),
            "ledger_hash_identical":true,
            "protocol_states_byte_identical":true,
            "pending_coinbase_maps_identical":true,
        })
    );
}
