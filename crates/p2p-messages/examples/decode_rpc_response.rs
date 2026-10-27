//! Decode a raw RPC response captured with `mina-rpc-capture` using this crate's
//! types, and report how far decoding gets.
//!
//!   cargo run -p mina-p2p-messages --example decode_rpc_response -- get_best_tip response.bin

use binprot::BinProtRead;
use mina_p2p_messages::{rpc, rpc_kernel::ResponsePayload, v2};

fn decode<T: BinProtRead + std::fmt::Debug>(
    bytes: &[u8],
) -> (Result<ResponsePayload<T>, String>, usize) {
    let mut cursor = bytes;
    let result = ResponsePayload::<T>::binprot_read(&mut cursor).map_err(|e| e.to_string());
    (result, bytes.len() - cursor.len())
}

fn commands(block: &v2::MinaBlockBlockStableV2) -> String {
    let diff = &block.body.staged_ledger_diff.diff;
    let all: Vec<_> = diff
        .0
        .commands
        .iter()
        .chain(diff.1.iter().flat_map(|d| d.commands.iter()))
        .collect();
    let zkapps = all
        .iter()
        .filter(|c| matches!(c.data, v2::MinaBaseUserCommandStableV2::ZkappCommand(_)))
        .count();
    format!(
        "height {}: {} commands ({zkapps} zkApp)",
        block
            .header
            .protocol_state
            .body
            .consensus_state
            .blockchain_length
            .as_u32(),
        all.len()
    )
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (rpc, path) = (args.next().unwrap(), args.next().unwrap());
    let bytes = std::fs::read(&path).unwrap();
    let total = bytes.len();
    let report = |ok: bool, consumed: usize, detail: String| {
        println!(
            "{rpc}: {} — consumed {consumed} of {total} bytes{}; {detail}",
            if ok && consumed == total {
                "OK"
            } else {
                "FAILED"
            },
            if consumed == total { "" } else { " (mismatch)" }
        )
    };
    match rpc.as_str() {
        "get_best_tip" | "get_ancestry" => {
            let (r, n) = decode::<rpc::GetBestTipV2Response>(&bytes);
            match r {
                Ok(p) => {
                    let tip = p.0.unwrap().0.unwrap();
                    report(
                        true,
                        n,
                        format!(
                            "data {}; proof block {}",
                            commands(&tip.data),
                            commands(&tip.proof.1)
                        ),
                    )
                }
                Err(e) => report(false, n, e),
            }
        }
        "get_transition_chain" => {
            let (r, n) =
                decode::<Option<mina_p2p_messages::list::List<v2::MinaBlockBlockStableV2>>>(&bytes);
            match r {
                Ok(p) => {
                    let blocks = p.0.unwrap().0.unwrap();
                    report(
                        true,
                        n,
                        blocks.iter().map(commands).collect::<Vec<_>>().join(", "),
                    )
                }
                Err(e) => report(false, n, e),
            }
        }
        "get_staged_ledger_aux_and_pending_coinbases_at_hash" => {
            let (r, n) =
                decode::<rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response>(&bytes);
            match r {
                Ok(p) => {
                    let (scan_state, ledger_hash, _, states) = p.0.unwrap().0.unwrap();
                    let trees = scan_state.scan_state.trees.1.len() + 1;
                    report(
                        true,
                        n,
                        format!(
                            "{trees} trees, ledger {ledger_hash:?}, {} protocol states",
                            states.len()
                        ),
                    )
                }
                Err(e) => report(false, n, e),
            }
        }
        other => panic!("unknown rpc {other}"),
    }
}
