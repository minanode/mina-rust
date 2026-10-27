//! Check the staged ledger hash of a block against the scan state and pending
//! coinbase a peer sent for it, both captured with `mina-rpc-capture`. The
//! block comes from a `get_transition_chain` response for its state hash.
//!
//!   cargo run --release -p mina-tree --example check_scan_state_hash -- \
//!       get_transition_chain_v2.bin staged_ledger_aux_v3.bin

use mina_p2p_messages::{binprot::BinProtRead, list::List, rpc, rpc_kernel::ResponsePayload, v2};
use mina_tree::{
    scan_state::{pending_coinbase::PendingCoinbase, scan_state::ScanState},
    staged_ledger::hash::StagedLedgerHash,
};

fn read<T: BinProtRead>(path: &str) -> T {
    let bytes = std::fs::read(path).unwrap();
    let payload = ResponsePayload::<T>::binprot_read(&mut bytes.as_slice()).unwrap();
    payload.0.unwrap().0
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (chain, aux) = (args.next().unwrap(), args.next().unwrap());

    let blocks = read::<Option<List<v2::MinaBlockBlockStableV2>>>(&chain).unwrap();
    let block = blocks.iter().next().unwrap();
    let expected = &block
        .header
        .protocol_state
        .body
        .blockchain_state
        .staged_ledger_hash;

    let (scan_state, ledger_hash, pending_coinbase, _states) =
        read::<rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response>(&aux).unwrap();

    let started = std::time::Instant::now();
    let scan_state = ScanState::try_from(&scan_state).unwrap();
    let mut pending_coinbase = PendingCoinbase::try_from(&pending_coinbase).unwrap();
    let computed = StagedLedgerHash::of_aux_ledger_and_coinbase_hash(
        scan_state.hash(),
        ledger_hash.to_field().unwrap(),
        &mut pending_coinbase,
    );
    let computed: v2::MinaBaseStagedLedgerHashStableV1 = (&computed).into();

    println!("block {}", block.header.protocol_state.try_hash().unwrap());
    let check =
        |name: &str, ok: bool| println!("{name}: {}", if ok { "match" } else { "MISMATCH" });
    check(
        "aux hash",
        computed.non_snark.aux_hash == expected.non_snark.aux_hash,
    );
    check(
        "ledger hash",
        computed.non_snark.ledger_hash == expected.non_snark.ledger_hash,
    );
    check(
        "pending coinbase aux",
        computed.non_snark.pending_coinbase_aux == expected.non_snark.pending_coinbase_aux,
    );
    check(
        "pending coinbase hash",
        computed.pending_coinbase_hash == expected.pending_coinbase_hash,
    );
    println!("converted and hashed in {:?}", started.elapsed());
    if computed != *expected {
        println!("expected {expected:?}\ncomputed {computed:?}");
        std::process::exit(1);
    }
}
