//! PC1: a staged-ledger aux answer an OCaml node sent, through the `Arc`-shared
//! proof types. The answer to `get_staged_ledger_aux_and_pending_coinbases_at_hash`
//! decodes and encodes back to OCaml's bytes; its scan state converts to the
//! ledger's types and back to the same bytes, sharing every proof rather than
//! copying it; and the staged-ledger hash still matches the block's.
//!
//! The captures are too large to commit (about 34 MB). They were made with
//! `mina-rpc-capture` from OCaml mainnet peers and are kept in the main
//! user-supplied capture directory:
//!
//!   MINA_PC1_AUX=/path/to/captures/staged_ledger_aux_563859.bin \
//!   MINA_PC1_CHAIN=/path/to/captures/transition_chain_563859.bin \
//!   cargo test --release -p mina-tree --test pc1_staged_ledger_aux_roundtrip \
//!       -- --ignored --nocapture

use std::collections::HashSet;

use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    list::List,
    rpc,
    rpc_kernel::ResponsePayload,
    v2,
};
use mina_tree::{
    scan_state::{pending_coinbase::PendingCoinbase, scan_state::ScanState},
    staged_ledger::hash::StagedLedgerHash,
};

fn encode(value: &impl BinProtWrite) -> Vec<u8> {
    let mut bytes = Vec::new();
    value.binprot_write(&mut bytes).unwrap();
    bytes
}

/// The addresses of every proof the scan state's jobs hold.
fn proof_addresses(scan_state: &ScanState) -> HashSet<usize> {
    let mut proofs = HashSet::new();
    scan_state.scan_state.for_each_retained_job(
        |_| {},
        |merge| {
            proofs.insert(merge.0.proof.as_ptr() as usize);
        },
    );
    proofs
}

#[test]
#[ignore = "needs MINA_PC1_AUX and MINA_PC1_CHAIN, OCaml captures supplied by the developer"]
fn ocaml_staged_ledger_aux_round_trips_and_shares_proofs() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/test-data/proof-cache/ocaml-aux-{}",
        std::process::id()
    ));
    mina_p2p_messages::proof_cache::set_global_dir(&dir).unwrap();
    type Aux = ResponsePayload<rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response>;
    type Chain = ResponsePayload<Option<List<v2::MinaBlockBlockStableV2>>>;
    let aux_bytes = std::fs::read(std::env::var("MINA_PC1_AUX").unwrap()).unwrap();
    let chain_bytes = std::fs::read(std::env::var("MINA_PC1_CHAIN").unwrap()).unwrap();

    // The wire bytes are OCaml's, with the proofs behind `Arc`.
    let aux = Aux::binprot_read(&mut aux_bytes.as_slice()).unwrap();
    assert_eq!(encode(&aux), aux_bytes, "re-encoded answer differs");
    let (wire_scan_state, ledger_hash, pending_coinbase, _states) = aux.0.unwrap().0.unwrap();
    let expected_wire = encode(&wire_scan_state);
    let expected_json = serde_json::to_vec(&wire_scan_state).unwrap();
    let chain = Chain::binprot_read(&mut chain_bytes.as_slice()).unwrap();
    assert_eq!(encode(&chain), chain_bytes, "re-encoded block differs");
    let blocks = chain.0.unwrap().0.unwrap();
    let block = blocks.iter().next().unwrap();

    // Wire to ledger, back to wire, and to ledger again: the same proofs.
    let ledger = ScanState::try_from(&wire_scan_state).unwrap();
    let back = v2::TransactionSnarkScanStateStableV2::from(&ledger);
    let again = ScanState::try_from(&back).unwrap();
    let (first, second) = (proof_addresses(&ledger), proof_addresses(&again));
    assert!(!first.is_empty());
    assert_eq!(first, second, "a conversion copied a proof");
    assert_eq!(
        encode(&back),
        encode(&wire_scan_state),
        "the ledger's scan state does not encode to OCaml's bytes"
    );
    println!(
        "{} distinct scan-state proofs shared through both conversions",
        first.len()
    );

    // The staged-ledger hash still matches the block's.
    let mut pending_coinbase = PendingCoinbase::try_from(&pending_coinbase).unwrap();
    let computed = StagedLedgerHash::of_aux_ledger_and_coinbase_hash(
        again.hash(),
        ledger_hash.to_field().unwrap(),
        &mut pending_coinbase,
    );
    let computed: v2::MinaBaseStagedLedgerHashStableV1 = (&computed).into();
    let expected = &block
        .header
        .protocol_state
        .body
        .blockchain_state
        .staged_ledger_hash;
    assert_eq!(&computed, expected);

    again.scan_state.for_each_retained_job(
        |_| {},
        |proof| {
            assert!(
                proof.0.proof.is_cached(),
                "a retained completed job stayed decoded"
            )
        },
    );
    let store = mina_p2p_messages::proof_cache::global();
    assert!(store.stats().files >= first.len() as u64);
    let reads = store.stats().reads;
    let start = std::time::Instant::now();
    for _ in 0..100 {
        std::hint::black_box(again.hash());
    }
    println!(
        "PC2 repeated aux hash: {:?} mean, no cache reads",
        start.elapsed() / 100
    );
    assert_eq!(store.stats().reads, reads, "job hashes read cached proofs");
    let start = std::time::Instant::now();
    assert_eq!(encode(&back), expected_wire);
    println!(
        "PC2 OCaml aux cached binprot: {:?}, {} reads",
        start.elapsed(),
        store.stats().reads - reads
    );
    let start = std::time::Instant::now();
    assert_eq!(serde_json::to_vec(&back).unwrap(), expected_json);
    println!("PC2 OCaml aux cached JSON: {:?}", start.elapsed());

    let stable_hash = ledger.hash();
    let file = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::remove_file(file).unwrap();
    // The cached job hash stays usable, while serving/checkpointing the
    // unavailable proof fails cleanly.
    assert_eq!(again.hash(), stable_hash);
    assert!(back.binprot_write(&mut Vec::new()).is_err());
    drop((again, ledger, back, wire_scan_state));
    assert_eq!(store.stats().files, 0);
    std::fs::remove_dir_all(dir).unwrap();
}
