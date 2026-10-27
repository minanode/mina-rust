//! P5.2: the snark pool diff our worker gossips, laid out against OCaml's own
//! bytes. The works are the 32 Mesa mainnet bundles an OCaml 4.0.x seed sent
//! in its `get_completed_snarks` answer (R24c's capture,
//! `crates/p2p-messages/tests/files/mesa/get_completed_snarks_v1.bin`). Each
//! is offered as our node offers its own work, and the diff must be the bytes
//! OCaml's types give: `Gossip_net.Message.V3.Snark_pool_diff` (tag 1) of
//! `With_nonce {message; nonce}`, whose message is `Add_solved_work` (tag 0)
//! of the statement bundle and `Priced_proof {proof; fee = {fee; prover}}`.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/gossip_net/message.ml#L27-L37>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/snark_pool_diff.ml#L27-L29>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/priced_proof.ml#L8-L10>

use mina_core::snark::Snark;
use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    gossip::{decode_pubsub_data, GossipNetMessageV2},
    rpc::GetCompletedSnarksV1,
    rpc_kernel::{ResponsePayload, RpcMethod},
    v2::{TransactionSnarkWorkTStableV2, TransactionSnarkWorkTStableV2Proofs},
};

fn encode(value: &impl BinProtWrite) -> Vec<u8> {
    let mut bytes = Vec::new();
    value.binprot_write(&mut bytes).unwrap();
    bytes
}

/// The capture's raw bytes and its works.
fn ocaml_works() -> (Vec<u8>, Vec<TransactionSnarkWorkTStableV2>) {
    let path = format!(
        "{}/../p2p-messages/tests/files/mesa/get_completed_snarks_v1.bin",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    type Response = <GetCompletedSnarksV1 as RpcMethod>::Response;
    let payload = ResponsePayload::<Response>::binprot_read(&mut &bytes[..]).unwrap();
    let works = payload.0.expect("ok response").0.expect("Some works");
    (bytes, works.0.into_iter().collect())
}

#[test]
fn our_snark_pool_diff_is_ocamls_layout_of_ocamls_bytes() {
    let (raw, works) = ocaml_works();
    assert_eq!(works.len(), 32);
    let mut two = 0;
    for work in works {
        // `Transaction_snark_work.T.Stable.V2` is {fee; proofs; prover}. Find
        // the work in OCaml's bytes; its three fields are consecutive there.
        let (fee, proofs, prover) = (
            encode(&work.fee),
            encode(&work.proofs),
            encode(&work.prover),
        );
        let whole = [&fee[..], &proofs[..], &prover[..]].concat();
        assert!(
            raw.windows(whole.len()).any(|w| w == whole),
            "work is not OCaml's bytes"
        );

        // Each ledger proof starts with its statement, which ends with the
        // 32-byte sok digest (length 0x20, then the digest). The statement a
        // diff carries is the same with `sok_digest : unit`, encoded 0x00.
        let ledger_proofs = match &work.proofs {
            TransactionSnarkWorkTStableV2Proofs::One(p) => vec![p],
            TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => {
                two += 1;
                vec![p, q]
            }
        };
        let proof_lens: usize = ledger_proofs.iter().map(|p| encode(*p).len()).sum();
        // `One_or_two` is a polymorphic variant: its tag is OCaml's hash of
        // the constructor, the same for the statement bundle and the proofs.
        let tag = &proofs[..proofs.len() - proof_lens];
        let mut statements = tag.to_vec();
        let mut offset = tag.len();
        for proof in &ledger_proofs {
            let with_sok = encode(&proof.0.statement);
            assert_eq!(&proofs[offset..offset + with_sok.len()], &with_sok[..]);
            let (body, digest) = with_sok.split_at(with_sok.len() - 33);
            assert_eq!(digest[0], 0x20);
            statements.extend_from_slice(body);
            statements.push(0x00);
            offset += encode(*proof).len();
        }

        let snark = Snark::from(work.clone());
        let nonce = 0u64;
        let message = GossipNetMessageV2::SnarkPoolDiff {
            message: snark.gossip_diff().unwrap(),
            nonce: nonce.into(),
        };
        let ours = encode(&message);
        let expected = [&[1u8, 0][..], &statements, &proofs, &fee, &prover, &[0]].concat();
        assert_eq!(ours, expected);
        // PC1: a work the pool stored, its proofs cached, gossips the same.
        let cached = GossipNetMessageV2::SnarkPoolDiff {
            message: snark.cached().gossip_diff().unwrap(),
            nonce: nonce.into(),
        };
        assert_eq!(encode(&cached), expected);

        // What an OCaml peer reads: the 8-byte size header, then the message.
        let mut data = (ours.len() as u64).to_le_bytes().to_vec();
        data.extend_from_slice(&ours);
        assert_eq!(decode_pubsub_data(&data).unwrap(), message);
    }
    // All 32 are two-proof bundles, the shape the worker gossips most.
    assert_eq!(two, 32);
}
