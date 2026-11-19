//! Staged ledgers to serve, from a few bytes to larger than devnet's.
//!
//! The large ones repeat one base job of the devnet scan state at block
//! 590216, from OCaml's answer: a zkApp command with 15 proofs and its two
//! witnesses, 345,619 bytes. Devnet's answer was 172 to 174 MB on 2026-11-18,
//! about 500 such jobs.
#![allow(dead_code)]

use std::sync::Arc;

use binprot::{BinProtRead, BinProtWrite};
use mina_p2p::channels::rpc::StagedLedgerAuxAndPendingCoinbases;
use mina_p2p_messages::{
    list::List,
    rpc::GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response,
    rpc_kernel::{NeedsLength, ResponsePayload, RpcResult},
    v2,
};

/// Encoded bytes of the base job.
pub const BASE_JOB_BYTES: usize = 345_619;

fn base_job() -> v2::TransactionSnarkScanStateStableV2TreesABase {
    let bytes = include_bytes!(
        "../../../p2p-messages/tests/files/mesa/scan_state_base_job_590216_heavy.bin"
    );
    let mut reader = &bytes[..];
    let job = v2::TransactionSnarkScanStateStableV2TreesABase::binprot_read(&mut reader).unwrap();
    assert!(reader.is_empty());
    job
}

fn empty_job() -> v2::TransactionSnarkScanStateStableV2TreesABase {
    (
        v2::ParallelScanWeightStableV1 {
            base: 1.into(),
            merge: 0.into(),
        },
        v2::TransactionSnarkScanStateStableV2ScanStateTreesABaseT1::Empty,
    )
}

/// A staged ledger whose scan state is `jobs` trees of one base job each,
/// `heavy` of them the devnet zkApp job and the rest empty.
pub fn staged_ledger(jobs: usize, heavy: usize) -> Arc<StagedLedgerAuxAndPendingCoinbases> {
    let job = base_job();
    let mut trees = (0..jobs.max(1)).map(|i| {
        v2::TransactionSnarkScanStateStableV2ScanStateTreesA::Leaf(vec![if i < heavy {
            job.clone()
        } else {
            empty_job()
        }])
    });
    let first = trees.next().unwrap();
    Arc::new(StagedLedgerAuxAndPendingCoinbases {
        scan_state: v2::TransactionSnarkScanStateStableV2 {
            scan_state: v2::TransactionSnarkScanStateStableV2ScanState {
                trees: (first, trees.collect()),
                acc: None,
                curr_job_seq_no: 0.into(),
                max_base_jobs: 1.into(),
                delay: 2.into(),
            },
            previous_incomplete_zkapp_updates: (
                List::new(),
                v2::TransactionSnarkScanStateStableV2PreviousIncompleteZkappUpdates1::Border_block_continued_in_the_next_tree(false),
            ),
        },
        staged_ledger_hash: v2::LedgerHash::zero(),
        pending_coinbase: v2::MinaBasePendingCoinbaseStableV2 {
            tree: v2::MinaBasePendingCoinbaseMerkleTreeVersionedStableV2 {
                indexes: List::new(),
                depth: 5.into(),
                tree: v2::MinaBasePendingCoinbaseMerkleTreeVersionedStableV2Tree::Hash(
                    v2::MinaBasePendingCoinbaseHashVersionedStableV1(
                        v2::MinaBasePendingCoinbaseHashBuilderStableV1(
                            mina_p2p_messages::bigint::BigInt::zero(),
                        ),
                    )
                    .into(),
                ),
            },
            pos_list: List::new(),
            new_pos: v2::MinaBasePendingCoinbaseStackIdStableV1(0.into()),
        },
        needed_blocks: List::new(),
    })
}

/// A staged ledger of `jobs` devnet zkApp base jobs.
pub fn heavy_staged_ledger(jobs: usize) -> Arc<StagedLedgerAuxAndPendingCoinbases> {
    staged_ledger(jobs, jobs)
}

/// The answer's payload as OCaml encodes it and as we did before: the
/// four-tuple inside `RpcResult(Ok(NeedsLength(Some(..))))`.
pub fn expected_payload(parts: &StagedLedgerAuxAndPendingCoinbases) -> Vec<u8> {
    let StagedLedgerAuxAndPendingCoinbases {
        scan_state,
        staged_ledger_hash,
        pending_coinbase,
        needed_blocks,
    } = parts.clone();
    let response: ResponsePayload<GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response> =
        RpcResult(Ok(NeedsLength(Some((
            scan_state,
            staged_ledger_hash.inner().0.clone(),
            pending_coinbase,
            needed_blocks,
        )))));
    let mut bytes = Vec::new();
    response.binprot_write(&mut bytes).unwrap();
    bytes
}

/// The payload of an empty (`None`) answer.
pub fn none_payload() -> Vec<u8> {
    let response: ResponsePayload<GetStagedLedgerAuxAndPendingCoinbasesAtHashV2Response> =
        RpcResult(Ok(NeedsLength(None)));
    let mut bytes = Vec::new();
    response.binprot_write(&mut bytes).unwrap();
    bytes
}
