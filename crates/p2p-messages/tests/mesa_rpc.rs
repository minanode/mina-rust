//! Decode RPC responses captured from Mesa (OCaml 4.0.x) mainnet nodes with
//! `mina-rpc-capture`, so the wire types can be checked against what the network
//! actually sends.

use binprot::BinProtRead;
use mina_p2p_messages::{
    list::List,
    rpc::{GetAncestryV2, GetBestTipV2, GetBestTipV2Response, GetTransitionChainV2},
    rpc_kernel::{ResponsePayload, RpcMethod},
    v2::{self, MinaBlockBlockStableV2},
};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/files/mesa/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Decode a whole response payload, failing with the byte offset reached.
fn decode<T: BinProtRead>(name: &str) -> T {
    let bytes = fixture(name);
    let mut cursor = &bytes[..];
    let result = ResponsePayload::<T>::binprot_read(&mut cursor);
    let consumed = bytes.len() - cursor.len();
    let payload = result.unwrap_or_else(|e| {
        panic!(
            "{name}: decode failed after {consumed} of {} bytes: {e}",
            bytes.len()
        )
    });
    assert_eq!(cursor.len(), 0, "{name}: trailing bytes");
    payload.0.expect("ok response").0
}

fn zkapp_commands(block: &MinaBlockBlockStableV2) -> usize {
    let diff = &block.body.staged_ledger_diff.diff;
    diff.0
        .commands
        .iter()
        .chain(diff.1.iter().flat_map(|d| d.commands.iter()))
        .filter(|c| matches!(c.data, v2::MinaBaseUserCommandStableV2::ZkappCommand(_)))
        .count()
}

/// The RPCs this node requests must be the versions Mesa nodes serve. The menu
/// was captured from a mainnet seed running 4.0.x.
#[test]
fn requested_rpc_versions_are_served_by_mesa_nodes() {
    let menu: List<(mina_p2p_messages::string::CharString, u32)> =
        decode("versioned_rpc_menu_v1.bin");
    let served: Vec<(String, u32)> = menu.iter().map(|(n, v)| (n.to_string(), *v)).collect();
    for (name, version) in [
        (GetBestTipV2::NAME_STR, GetBestTipV2::VERSION),
        (GetAncestryV2::NAME_STR, GetAncestryV2::VERSION),
        (
            GetTransitionChainV2::NAME_STR,
            GetTransitionChainV2::VERSION,
        ),
    ] {
        assert!(
            served.contains(&(name.to_string(), version)),
            "{name}:{version} is not served; Mesa serves {served:?}"
        );
    }
}

/// We answer `__Versioned_rpc.Menu` with the very bytes a Mesa mainnet seed
/// answered it with: the same RPCs and versions, in the same order.
#[test]
fn our_menu_answer_is_a_mesa_nodes() {
    use binprot::BinProtWrite;
    use mina_p2p_messages::{
        rpc::VersionedRpcMenuV1,
        rpc_kernel::{NeedsLength, RpcResult},
    };

    let payload: ResponsePayload<<VersionedRpcMenuV1 as RpcMethod>::Response> =
        RpcResult(Ok(NeedsLength(VersionedRpcMenuV1::menu())));
    let mut bytes = Vec::new();
    payload.binprot_write(&mut bytes).unwrap();
    assert_eq!(bytes, fixture("versioned_rpc_menu_v1.bin"));
}

#[test]
fn get_best_tip_v3_response_decodes() {
    let tip = decode::<GetBestTipV2Response>("get_best_tip_v3.bin").expect("some best tip");
    let height = tip
        .data
        .header
        .protocol_state
        .body
        .consensus_state
        .blockchain_length
        .as_u32();
    assert_eq!(height, 564003);
    assert_eq!(zkapp_commands(&tip.data), 0);
}

/// Block 563900 carries 12 zkApp commands, whose types changed in Mesa (zkApp
/// state grew from 8 to 32 fields).
#[test]
fn mesa_block_with_zkapp_commands_decodes() {
    let blocks: List<MinaBlockBlockStableV2> =
        decode::<Option<List<MinaBlockBlockStableV2>>>("get_transition_chain_v2_563900_zkapp.bin")
            .expect("some blocks");
    assert_eq!(blocks.len(), 1);
    assert_eq!(zkapp_commands(blocks.iter().next().unwrap()), 12);
}

/// OCaml rebroadcasts pool diffs with the time in nanoseconds as the nonce,
/// which needs more than 32 bits.
#[test]
fn gossip_rebroadcast_nonce_decodes() {
    use mina_p2p_messages::gossip::GossipNetMessageV2;

    let nonce: i64 = 1_792_383_635_000_000_000;
    // Transaction_pool_diff (tag 2), an empty diff, then the nonce as a bin_prot
    // int: 0xfc and 8 little-endian bytes.
    let mut bytes = vec![2, 0, 0xfc];
    bytes.extend_from_slice(&nonce.to_le_bytes());

    let message = GossipNetMessageV2::binprot_read(&mut bytes.as_slice()).unwrap();
    let GossipNetMessageV2::TransactionPoolDiff {
        message,
        nonce: got,
    } = message
    else {
        panic!("expected a transaction pool diff");
    };
    assert!(message.0.is_empty());
    assert_eq!(got.as_u64(), nonce as u64);
}

/// Re-encodes a whole response payload and checks it gives back OCaml's bytes.
fn reencodes_identically<T: BinProtRead + binprot::BinProtWrite>(name: &str) {
    use binprot::BinProtWrite;

    let bytes = fixture(name);
    let payload = ResponsePayload::<T>::binprot_read(&mut &bytes[..]).expect(name);
    let mut encoded = Vec::with_capacity(bytes.len());
    payload.binprot_write(&mut encoded).expect(name);
    assert!(
        encoded == bytes,
        "{name}: re-encoding differs from OCaml's bytes"
    );
}

/// Our nodes gossip blocks they received by RPC, re-encoded, and an OCaml peer
/// bans a sender (and its IP, for a day) whose gossip it fails to decode. So a
/// block must re-encode to exactly the bytes OCaml sent.
#[test]
fn mesa_blocks_reencode_byte_identically() {
    reencodes_identically::<GetBestTipV2Response>("get_best_tip_v3.bin");
    reencodes_identically::<Option<List<MinaBlockBlockStableV2>>>(
        "get_transition_chain_v2_563900_zkapp.bin",
    );
}

#[test]
fn ancestry_reencodes_ocaml_response_exactly() {
    reencodes_identically::<<GetAncestryV2 as RpcMethod>::Response>("get_ancestry_v3.bin");
}

#[test]
fn catchup_proof_knowledge_and_multi_hash_chain_match_ocaml_bytes() {
    use mina_p2p_messages::rpc::{GetTransitionChainProofV1ForV2, GetTransitionKnowledgeV1ForV2};
    reencodes_identically::<<GetTransitionChainProofV1ForV2 as RpcMethod>::Response>(
        "get_transition_chain_proof_v1.bin",
    );
    reencodes_identically::<<GetTransitionKnowledgeV1ForV2 as RpcMethod>::Response>(
        "get_transition_knowledge_v1.bin",
    );
    reencodes_identically::<<GetTransitionChainV2 as RpcMethod>::Response>(
        "get_transition_chain_v2_multi.bin",
    );
    assert!(decode::<<GetTransitionChainV2 as RpcMethod>::Response>(
        "get_transition_chain_v2_over_limit.bin"
    )
    .is_none());
    assert!(
        decode::<<GetTransitionChainProofV1ForV2 as RpcMethod>::Response>(
            "get_transition_chain_proof_v1_unknown.bin"
        )
        .is_none()
    );
}

#[test]
fn ocaml_chain_proof_reaches_below_knowledge_root() {
    use mina_p2p_messages::rpc::{
        GetTransitionChainProofV1ForV2Response, GetTransitionKnowledgeV1ForV2,
    };

    // Both responses were captured from the same OCaml peer for the same tip.
    // The proof includes its retained root history, which our best-chain-only
    // responder cannot currently serve.
    let knowledge: <GetTransitionKnowledgeV1ForV2 as RpcMethod>::Response =
        decode("get_transition_knowledge_v1.bin");
    let (proof_root, body_hashes) =
        decode::<GetTransitionChainProofV1ForV2Response>("get_transition_chain_proof_v1.bin")
            .expect("OCaml answered with a proof");
    assert_eq!(knowledge.len(), 291);
    assert_eq!(body_hashes.len(), 590);
    assert_ne!(proof_root, *knowledge.front().expect("knowledge root"));
}

#[test]
fn completed_snarks_v1_matches_ocaml_response_bytes() {
    use mina_p2p_messages::rpc::{GetCompletedSnarksV1, MAX_COMPLETED_SNARKS};
    type Response = <GetCompletedSnarksV1 as RpcMethod>::Response;
    let works: Response = decode("get_completed_snarks_v1.bin");
    // The raw OCaml payload starts with Some, list length 32 (0x01 0x20).
    assert_eq!(works.unwrap().0.len(), MAX_COMPLETED_SNARKS);
    reencodes_identically::<Response>("get_completed_snarks_v1.bin");
    let menu: List<(mina_p2p_messages::string::CharString, u32)> =
        decode("versioned_rpc_menu_v1.bin");
    assert!(menu.iter().any(
        |(name, version)| name.as_ref() == GetCompletedSnarksV1::NAME
            && *version == GetCompletedSnarksV1::VERSION
    ));
}

#[test]
fn completed_snarks_rejects_oversized_list_before_decoding_proofs() {
    use mina_p2p_messages::rpc::CompletedSnarks;
    // OCaml bin_prot list prefix: 33. No proof body is present, so an EOF
    // instead of InvalidData would mean we tried decoding an over-limit list.
    let error = CompletedSnarks::binprot_read(&mut &[33u8][..]).unwrap_err();
    assert!(error.to_string().contains("32-work limit"), "{error}");
}
