//! Opt-in measurement against an OCaml RPC capture. Inflated cases repeat
//! captured protocol states to exercise the wire caps, not consensus validity.
use super::*;
use crate::channels::{
    rpc::{P2pRpcResponse, RpcChannelMsg, StagedLedgerAuxAndPendingCoinbases},
    streaming_rpc::{
        staged_ledger_parts::{StagedLedgerPartsBase, StagedLedgerPartsResponse},
        P2pStreamingRpcResponse, StreamingRpcChannelMsg,
    },
};
use binprot::BinProtRead;
use mina_p2p_messages::{
    rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response, rpc_kernel::ResponsePayload, v2,
};
use std::time::Instant;

fn measure(label: &str, msg: ChannelMsg) {
    struct Borrowed<'a>(&'a ChannelMsg);
    impl Weight for Borrowed<'_> {
        fn weight(&self) -> usize {
            message_weight(self.0)
        }
    }
    let (tx, mut rx) = queue::channel(queue::Kind::Command, 1, PEER_BYTES, Arc::default());
    let mut rows = Vec::new();
    for _ in 0..11 {
        let started = Instant::now();
        let bytes = message_weight(std::hint::black_box(&msg));
        let traversal_ns = started.elapsed().as_nanos();
        tx.tracked_send(Borrowed(&msg)).unwrap();
        let gate_ns = queue::last_gate_ns();
        drop(rx.try_recv().unwrap());
        rows.push((traversal_ns, gate_ns));
        println!(
            "queue_gate label={label} bytes={bytes} traversal_ns={traversal_ns} gate_ns={gate_ns}"
        );
    }
    let mut gates: Vec<_> = rows.iter().map(|row| row.1).collect();
    gates.sort_unstable();
    rows.sort_unstable();
    println!(
        "queue_gate_median label={label} traversal_ns={} gate_ns={}",
        rows[5].0, gates[5]
    );
}

#[test]
#[ignore = "requires MINA_QUEUE_GATE_FIXTURE: raw OCaml v3 staged-ledger response"]
fn measure_large_message_queue_gate() {
    let bytes = std::fs::read(std::env::var_os("MINA_QUEUE_GATE_FIXTURE").unwrap()).unwrap();
    let mut reader = bytes.as_slice();
    let response =
        ResponsePayload::<GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response>::binprot_read(
            &mut reader,
        )
        .unwrap();
    assert!(reader.is_empty());
    let (scan_state, hash, pending_coinbase, needed_blocks) = response.0.unwrap().0.unwrap();
    let mut parts = StagedLedgerAuxAndPendingCoinbases {
        scan_state,
        staged_ledger_hash: v2::MinaBaseLedgerHash0StableV1(hash).into(),
        pending_coinbase,
        needed_blocks,
    };
    let rpc = |parts| {
        ChannelMsg::Rpc(RpcChannelMsg::Response(
            0,
            Some(P2pRpcResponse::StagedLedgerAuxAndPendingCoinbasesAtBlock(
                Arc::new(parts),
            )),
        ))
    };
    measure("ocaml_staged_ledger", rpc(parts.clone()));
    for (i, tree) in std::iter::once(&parts.scan_state.scan_state.trees.0)
        .chain(parts.scan_state.scan_state.trees.1.iter())
        .enumerate()
    {
        measure(
            &format!("ocaml_tree_{i}"),
            ChannelMsg::StreamingRpc(StreamingRpcChannelMsg::Response(
                0,
                Some(P2pStreamingRpcResponse::StagedLedgerParts(
                    StagedLedgerPartsResponse::ScanStateTree(tree.clone()),
                )),
            )),
        );
    }
    // Near-cap binprot messages, constructed from captured state objects. These
    // isolate size traversal cost and are not offered as valid ledger responses.
    let state = parts
        .needed_blocks
        .iter()
        .next()
        .expect("capture has needed blocks")
        .clone();
    let mut counter = queue::Counter::default();
    binprot::BinProtWrite::binprot_write(&state, &mut counter).unwrap();
    let state_bytes = counter.0;
    let base = |blocks| {
        ChannelMsg::StreamingRpc(StreamingRpcChannelMsg::Response(
            0,
            Some(P2pStreamingRpcResponse::StagedLedgerParts(
                StagedLedgerPartsResponse::Base(StagedLedgerPartsBase {
                    staged_ledger_hash: parts.staged_ledger_hash.clone(),
                    pending_coinbase: Box::new(parts.pending_coinbase.clone()),
                    needed_blocks: blocks,
                }),
            )),
        ))
    };
    let overhead = message_weight(&base(mina_p2p_messages::list::List::new()));
    let count = (ChannelId::StreamingRpc.max_msg_size() - overhead - 16) / state_bytes;
    measure(
        "streaming_near_16MiB",
        base(std::iter::repeat_n(state.clone(), count).collect()),
    );
    parts.needed_blocks = mina_p2p_messages::list::List::new();
    let overhead = message_weight(&rpc(parts.clone()));
    let count = (ChannelId::Rpc.max_msg_size() - overhead - 16) / state_bytes;
    parts.needed_blocks = std::iter::repeat_n(state, count).collect();
    measure("rpc_near_256MiB", rpc(parts));
}
