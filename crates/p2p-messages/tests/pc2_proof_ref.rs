//! OCaml fixture bytes and JSON remain transparent through cached handles.
use std::{path::PathBuf, sync::Arc};

use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    proof_cache::ProofStore,
    rpc::{GetBestTipV2Response, GetCompletedSnarksV1},
    rpc_kernel::{ResponsePayload, RpcMethod},
    v2::{self, TransactionSnarkWorkTStableV2Proofs},
    ProofRef,
};

fn encode(value: &impl BinProtWrite) -> Vec<u8> {
    let mut bytes = Vec::new();
    value.binprot_write(&mut bytes).unwrap();
    bytes
}

fn scratch(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-data/proof-cache");
    let dir = root.join(format!("{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn proofs(
    work: &TransactionSnarkWorkTStableV2Proofs,
) -> Vec<&ProofRef<v2::TransactionSnarkProofStableV2>> {
    match work {
        TransactionSnarkWorkTStableV2Proofs::One(p) => vec![&p.0.proof],
        TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => vec![&p.0.proof, &q.0.proof],
    }
}

fn cache_zkapp(
    node: &v2::MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA,
    store: &ProofStore,
) -> usize {
    let mut count = 0;
    if let v2::MinaBaseControlStableV2::Proof(proof) = &node.account_update.authorization {
        proof.cache_in(store).unwrap();
        count += 1;
    }
    count
        + node
            .calls
            .iter()
            .map(|call| cache_zkapp(&call.elt, store))
            .sum::<usize>()
}

#[test]
fn ocaml_works_block_and_zkapp_have_identical_binprot_and_json() {
    let dir = scratch("transparent");
    for store in [ProofStore::memory(), ProofStore::dir(&dir).unwrap()] {
        let bytes = include_bytes!("files/mesa/get_completed_snarks_v1.bin");
        let response =
            ResponsePayload::<<GetCompletedSnarksV1 as RpcMethod>::Response>::binprot_read(
                &mut bytes.as_slice(),
            )
            .unwrap();
        let json = serde_json::to_vec(&response).unwrap();
        for work in response.0.as_ref().unwrap().0.as_ref().unwrap().0.iter() {
            for p in proofs(&work.proofs) {
                p.cache_in(&store).unwrap();
            }
        }
        assert_eq!(encode(&response), bytes);
        assert_eq!(serde_json::to_vec(&response).unwrap(), json);

        let bytes = include_bytes!("files/mesa/get_best_tip_v3.bin");
        let response =
            ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut bytes.as_slice()).unwrap();
        let json = serde_json::to_vec(&response).unwrap();
        let block = &response.0.as_ref().unwrap().0.as_ref().unwrap().data;
        for diff in std::iter::once(&block.body.staged_ledger_diff.diff.0.completed_works).chain(
            block
                .body
                .staged_ledger_diff
                .diff
                .1
                .as_ref()
                .map(|d| &d.completed_works),
        ) {
            for work in diff.iter() {
                for p in proofs(&work.proofs) {
                    p.cache_in(&store).unwrap();
                }
            }
        }
        assert_eq!(encode(&response), bytes);
        assert_eq!(serde_json::to_vec(&response).unwrap(), json);

        let bytes = include_bytes!("../../../tests/files/zkapps/with_proof_auth.bin");
        let command =
            v2::MinaBaseZkappCommandTStableV1WireStableV1::binprot_read(&mut bytes.as_slice())
                .unwrap();
        let json = serde_json::to_vec(&command).unwrap();
        let n = command
            .account_updates
            .iter()
            .map(|u| cache_zkapp(&u.elt, &store))
            .sum::<usize>();
        assert!(n > 0);
        assert_eq!(encode(&command), bytes);
        assert_eq!(serde_json::to_vec(&command).unwrap(), json);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cache_releases_every_shared_decoded_copy_and_reads_check_integrity() {
    let dir = scratch("integrity");
    let store = ProofStore::dir(&dir).unwrap();
    let raw = mina_p2p_messages::v2::dummy_transaction_proof();
    // An independent decoded value, so the static dummy does not retain it.
    let raw = Arc::new((*raw).clone());
    let weak = Arc::downgrade(&raw);
    let proof: ProofRef<v2::TransactionSnarkProofStableV2> = raw.into();
    let shared = proof.clone();
    let bytes = encode(&proof);
    let json = serde_json::to_vec(&proof).unwrap();
    proof.cache_in(&store).unwrap();
    assert!(
        weak.upgrade().is_none(),
        "decoded allocations still retained"
    );
    assert!(shared.is_cached());
    assert_eq!(store.stats().files, 1);
    assert_eq!(encode(&shared), bytes);
    assert_eq!(serde_json::to_vec(&shared).unwrap(), json);
    assert_eq!(store.stats().reads, 2);
    let file = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut other = (*proof.get().unwrap()).clone();
    other.0.proof.ft_eval1 = other.0.proof.bulletproof.z_1.clone();
    let swapped = encode(&other);
    assert_eq!(swapped.len(), bytes.len());
    assert_ne!(swapped, bytes);
    assert!(v2::TransactionSnarkProofStableV2::binprot_read(&mut swapped.as_slice()).is_ok());
    std::fs::write(&file, swapped).unwrap();
    assert!(proof.get().is_err());
    assert!(shared.binprot_write(&mut Vec::new()).is_err());
    assert!(serde_json::to_vec(&shared).is_err());
    assert_ne!(proof, shared);
    std::fs::write(&file, bytes).unwrap();
    assert!(proof.get().is_err());
    assert_eq!(store.stats().unreadable, 1);
    drop((proof, shared));
    assert_eq!(store.stats().files, 0);
    std::fs::remove_dir_all(dir).unwrap();
}
