//! PC1: what a decoded proof costs on the heap, against its binprot bytes.
//!
//! A counting global allocator measures the requested heap bytes a decoded
//! value retains (its inline size is added), for proofs OCaml nodes sent:
//! the 32 Mesa mainnet bundles of R24c's `get_completed_snarks` capture, the
//! best tip block of `get_best_tip_v3.bin` (its protocol state proof and its
//! completed works) and the zkApp command of `with_proof_auth.bin`. Requested
//! bytes exclude the allocator's size-class rounding and metadata.
//!
//! Run: `cargo test --release -p mina-core --test pc1_proof_sizes --
//! --nocapture --test-threads=1`.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    rpc::{GetBestTipV2, GetCompletedSnarksV1},
    rpc_kernel::{ResponsePayload, RpcMethod},
    v2::{
        LedgerProofProdStableV2, MinaBaseControlStableV2, MinaBaseProofStableV2,
        MinaBaseZkappCommandTStableV1WireStableV1,
        MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA, MinaBlockBlockStableV2,
        PicklesProofProofsVerifiedMaxStableV2, TransactionSnarkProofStableV2,
        TransactionSnarkWorkTStableV2, TransactionSnarkWorkTStableV2Proofs,
    },
};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        ALLOCS.fetch_sub(1, Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(new_size, Ordering::Relaxed);
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn encode(value: &impl BinProtWrite) -> Vec<u8> {
    let mut bytes = Vec::new();
    value.binprot_write(&mut bytes).unwrap();
    bytes
}

/// Bytes and heap allocations a value decoded from `bytes` retains, with its
/// inline size, and the decode time.
#[derive(Default, Clone, Copy)]
struct Cost {
    count: usize,
    binprot: usize,
    decoded: usize,
    allocations: usize,
    decode_ns: u128,
    encode_ns: u128,
}

impl Cost {
    fn add(&mut self, other: Cost) {
        self.count += other.count;
        self.binprot += other.binprot;
        self.decoded += other.decoded;
        self.allocations += other.allocations;
        self.decode_ns += other.decode_ns;
        self.encode_ns += other.encode_ns;
    }

    fn print(&self, what: &str) {
        let n = self.count.max(1);
        println!(
            "{what:<44} n={:<4} binprot={:>8.0} B decoded={:>8.0} B ratio={:.2} \
             allocs={:>5.0} decode={:>6.1} us encode={:>6.1} us",
            self.count,
            self.binprot as f64 / n as f64,
            self.decoded as f64 / n as f64,
            self.decoded as f64 / self.binprot.max(1) as f64,
            self.allocations as f64 / n as f64,
            self.decode_ns as f64 / n as f64 / 1000.0,
            self.encode_ns as f64 / n as f64 / 1000.0,
        );
    }
}

fn cost<T: BinProtRead + BinProtWrite>(value: &T) -> Cost {
    let bytes = encode(value);
    let (live, allocs) = (LIVE.load(Ordering::SeqCst), ALLOCS.load(Ordering::SeqCst));
    let start = Instant::now();
    let decoded = T::binprot_read(&mut &bytes[..]).unwrap();
    let decode_ns = start.elapsed().as_nanos();
    let heap = LIVE.load(Ordering::SeqCst) - live;
    let allocations = ALLOCS.load(Ordering::SeqCst) - allocs;
    let start = Instant::now();
    let again = encode(&decoded);
    let encode_ns = start.elapsed().as_nanos();
    assert_eq!(again, bytes);
    drop(decoded);
    Cost {
        count: 1,
        binprot: bytes.len(),
        decoded: size_of::<T>() + heap,
        allocations,
        decode_ns,
        encode_ns,
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../p2p-messages/tests/files/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn ledger_proofs(proofs: &TransactionSnarkWorkTStableV2Proofs) -> Vec<&LedgerProofProdStableV2> {
    match proofs {
        TransactionSnarkWorkTStableV2Proofs::One(p) => vec![p],
        TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => vec![p, q],
    }
}

fn works_cost(works: &[TransactionSnarkWorkTStableV2], label: &str) {
    let (mut work, mut ledger, mut proof) = (Cost::default(), Cost::default(), Cost::default());
    for w in works {
        work.add(cost(w));
        for p in ledger_proofs(&w.proofs) {
            ledger.add(cost(p));
            proof.add(cost(&*p.0.proof.get().unwrap()));
        }
    }
    work.print(&format!("{label}: TransactionSnarkWorkTStableV2"));
    ledger.print(&format!("{label}: LedgerProofProdStableV2"));
    proof.print(&format!("{label}: TransactionSnarkProofStableV2"));
}

fn zkapp_proofs<'a>(
    node: &'a MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA,
    out: &mut Vec<std::sync::Arc<PicklesProofProofsVerifiedMaxStableV2>>,
) {
    if let MinaBaseControlStableV2::Proof(p) = &node.account_update.authorization {
        out.push(p.get().unwrap());
    }
    for call in node.calls.iter() {
        zkapp_proofs(&call.elt, out);
    }
}

#[test]
fn decoded_proof_heap_against_binprot() {
    // 32 two-proof bundles OCaml served.
    let bytes = fixture("mesa/get_completed_snarks_v1.bin");
    type Snarks = <GetCompletedSnarksV1 as RpcMethod>::Response;
    let payload = ResponsePayload::<Snarks>::binprot_read(&mut &bytes[..]).unwrap();
    let works: Vec<_> = payload.0.unwrap().0.unwrap().0.into_iter().collect();
    works_cost(&works, "pool works");

    // The best tip block, its proof and its completed works.
    let bytes = fixture("mesa/get_best_tip_v3.bin");
    type BestTip = <GetBestTipV2 as RpcMethod>::Response;
    let payload = ResponsePayload::<BestTip>::binprot_read(&mut &bytes[..]).unwrap();
    let block = payload.0.unwrap().0.unwrap().data;
    cost::<MinaBlockBlockStableV2>(&block).print("best tip: MinaBlockBlockStableV2");
    cost::<MinaBaseProofStableV2>(&block.header.protocol_state_proof)
        .print("best tip: protocol_state_proof");
    let diff = &block.body.staged_ledger_diff.diff;
    let block_works: Vec<_> = diff
        .0
        .completed_works
        .iter()
        .chain(diff.1.iter().flat_map(|d| d.completed_works.iter()))
        .cloned()
        .collect();
    works_cost(&block_works, "best tip works");

    // A zkApp command whose account update is proved.
    let bytes = std::fs::read(format!(
        "{}/../../tests/files/zkapps/with_proof_auth.bin",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let zkapp = MinaBaseZkappCommandTStableV1WireStableV1::binprot_read(&mut &bytes[..]).unwrap();
    cost(&zkapp).print("zkapp: MinaBaseZkappCommandTStableV1Wire");
    let mut proofs = Vec::new();
    for update in zkapp.account_updates.iter() {
        zkapp_proofs(&update.elt, &mut proofs);
    }
    let mut zk = Cost::default();
    for p in proofs {
        zk.add(cost(&*p));
    }
    zk.print("zkapp: PicklesProofProofsVerifiedMaxStableV2");

    // Decode and encode time per ledger proof, repeated for a steadier figure.
    let decoded = ledger_proofs(&works[0].proofs)[0].0.proof.get().unwrap();
    let proof = &*decoded;
    let bytes = encode(proof);
    let rounds = 2000u32;
    let start = Instant::now();
    for _ in 0..rounds {
        let p = TransactionSnarkProofStableV2::binprot_read(&mut &bytes[..]).unwrap();
        std::hint::black_box(&p);
    }
    let decode = start.elapsed() / rounds;
    let start = Instant::now();
    for _ in 0..rounds {
        std::hint::black_box(encode(proof));
    }
    let encode = start.elapsed() / rounds;
    println!("transaction proof decode {decode:?}, encode {encode:?} (mean of {rounds})");

    // What a read of a cached pool work costs (PC1 review fix): the SHA-256
    // check of one proof's bytes, and the whole read of a two-proof work from
    // files (two reads, two digest checks, two decodes), page cache warm.
    use sha2::{Digest, Sha256};
    let start = Instant::now();
    for _ in 0..rounds {
        std::hint::black_box(Sha256::digest(std::hint::black_box(&bytes)));
    }
    let digest = start.elapsed() / rounds;
    let dir = std::env::temp_dir().join(format!("mina-pc1-read-cost-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = mina_core::snark::proof_cache::ProofStore::dir(&dir).unwrap();
    let cached = mina_core::snark::Snark::from(works[0].clone())
        .proofs
        .cache_in(&store);
    let start = Instant::now();
    for _ in 0..rounds {
        std::hint::black_box(cached.get().unwrap());
    }
    let read = start.elapsed() / rounds;
    drop(cached);
    std::fs::remove_dir_all(&dir).unwrap();
    println!(
        "SHA-256 of one {}-byte proof {digest:?}; cached two-proof work read {read:?} (mean of {rounds})",
        bytes.len()
    );
}

#[test]
fn pc2_cache_reader_costs() {
    use mina_p2p_messages::proof_cache::ProofStore;
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/test-data/proof-cache/reader-cost-{}",
        std::process::id()
    ));
    let store = ProofStore::dir(&dir).unwrap();
    let bytes = fixture("mesa/get_completed_snarks_v1.bin");
    type Snarks = <GetCompletedSnarksV1 as RpcMethod>::Response;
    let work_response = ResponsePayload::<Snarks>::binprot_read(&mut &bytes[..]).unwrap();
    let works = &work_response.0.as_ref().unwrap().0.as_ref().unwrap().0;
    let heap = LIVE.load(Ordering::SeqCst);
    let started = Instant::now();
    for work in works.iter() {
        for p in ledger_proofs(&work.proofs) {
            p.0.proof.cache_in(&store).unwrap();
        }
    }
    let write = started.elapsed();
    let saved = heap.saturating_sub(LIVE.load(Ordering::SeqCst));
    println!("PC2 retained heap: {} proofs / {} binprot bytes, {saved} requested heap bytes released; total cache writes {write:?}", store.stats().files, store.stats().file_bytes);
    let first = ledger_proofs(&works.iter().next().unwrap().proofs);
    let rounds = 2000;
    let start = Instant::now();
    for _ in 0..rounds {
        std::hint::black_box(first[0].0.proof.get().unwrap());
    }
    println!(
        "PC2 one proof get (emitted/verifier/zkApp): {:?}",
        start.elapsed() / rounds
    );
    let start = Instant::now();
    for _ in 0..rounds {
        std::hint::black_box((
            first[0].0.proof.get().unwrap(),
            first[1].0.proof.get().unwrap(),
        ));
    }
    println!(
        "PC2 merge's two input reads: {:?}",
        start.elapsed() / rounds
    );

    let bytes = fixture("mesa/get_best_tip_v3.bin");
    type BestTip = <GetBestTipV2 as RpcMethod>::Response;
    let response = ResponsePayload::<BestTip>::binprot_read(&mut bytes.as_slice()).unwrap();
    let block = &response.0.as_ref().unwrap().0.as_ref().unwrap().data;
    let expected = encode(block);
    for work in block
        .body
        .staged_ledger_diff
        .diff
        .0
        .completed_works
        .iter()
        .chain(
            block
                .body
                .staged_ledger_diff
                .diff
                .1
                .iter()
                .flat_map(|d| d.completed_works.iter()),
        )
    {
        for p in ledger_proofs(&work.proofs) {
            p.0.proof.cache_in(&store).unwrap();
        }
    }
    let start = Instant::now();
    for _ in 0..200 {
        assert_eq!(encode(block), expected);
    }
    println!(
        "PC2 serve/checkpoint 51-proof block binprot: {:?}",
        start.elapsed() / 200
    );
    let start = Instant::now();
    for _ in 0..200 {
        std::hint::black_box(serde_json::to_vec(block).unwrap());
    }
    println!("PC2 full block JSON: {:?}", start.elapsed() / 200);
    drop(first);
    drop(response);
    drop(work_response);
    assert_eq!(store.stats().files, 0);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}
