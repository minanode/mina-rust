use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{
        AtomicBool, AtomicUsize,
        Ordering::{Acquire, Relaxed, Release},
    },
};

use ledger::proofs::{
    block::BlockParams,
    generate_block_proof, generate_tx_proof,
    provers::{BlockProver, TransactionProver},
    transaction::TransactionParams,
    verifiers::{BlockVerifier, TransactionVerifier},
};
use mina_core::thread;
use mina_curves::pasta::{Fp, Fq};
use mina_p2p_messages::{binprot::BinProtWrite, v2};
use serde::Serialize;
use wasm_bindgen::prelude::*;

// Counts requested allocations across this instance's worker pool, including
// wasm-bindgen's heap-allocated worker stacks/TLS. Static stack space, JS, code
// and allocator metadata are excluded. Linear memory is measured in JS.
struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ISOLATION_TAG: AtomicUsize = AtomicUsize::new(0);
static BLOCK_READY: AtomicBool = AtomicBool::new(false);

/// Both realms write this same static address. Different values demonstrate
/// separate backing stores, rather than just different JS object identities.
#[wasm_bindgen]
pub fn isolation_tag(value: usize) -> usize {
    ISOLATION_TAG.swap(value, Relaxed)
}

fn allocated(n: usize) {
    let live = LIVE.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(live, Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            allocated(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc_zeroed(layout);
        if !p.is_null() {
            allocated(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        System.dealloc(p, layout);
        LIVE.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let new = System.realloc(p, layout, size);
        if !new.is_null() {
            if size >= layout.size() {
                allocated(size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - size, Relaxed);
            }
        }
        new
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[derive(Serialize)]
struct Sample {
    phase: String,
    elapsed_ms: f64,
    live_bytes: usize,
    peak_bytes: usize,
}

fn sample(phase: &str, start: f64) -> Sample {
    Sample {
        phase: phase.into(),
        elapsed_ms: js_sys::Date::now() - start,
        live_bytes: LIVE.load(Relaxed),
        peak_bytes: PEAK.load(Relaxed),
    }
}

#[wasm_bindgen]
pub fn memory_stats() -> String {
    serde_json::to_string(&sample("snapshot", js_sys::Date::now())).unwrap()
}

fn js_error(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

async fn worker<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, JsValue> {
    thread::spawn(f)
        .join_async()
        .await
        .map_err(|_| js_error("worker panicked"))
}

#[wasm_bindgen]
pub async fn setup(threads: usize, kind: &str) -> Result<String, JsValue> {
    let block = match kind {
        "block" => true,
        "transaction" => false,
        _ => return Err(js_error("unknown proof kind")),
    };
    console_error_panic_hook::set_once();
    mina_core::NetworkConfig::init("mainnet").map_err(js_error)?;
    mina_core::init_work_dir_if_unset(|| "/i34-prover".into());
    thread::main_thread_init();
    worker(move || {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads.clamp(1, 32))
            .spawn_handler(|worker| {
                thread::spawn(move || worker.run());
                Ok(())
            })
            .build_global()
            .map_err(|e| e.to_string())
    })
    .await?
    .map_err(js_error)?;

    worker(move || {
        let mut samples = Vec::new();
        let start = js_sys::Date::now();
        let _ = ledger::verifier::get_srs_mut::<Fp>();
        let _ = ledger::verifier::get_srs_mut::<Fq>();
        samples.push(sample("prover_srs", start));
        let block_verifier = if block {
            let start = js_sys::Date::now();
            let verifier = futures::executor::block_on(BlockVerifier::make());
            samples.push(sample("block_verifier", start));
            Some(verifier)
        } else {
            None
        };
        let start = js_sys::Date::now();
        let verifier = futures::executor::block_on(TransactionVerifier::make());
        samples.push(sample("transaction_verifier", start));
        let start = js_sys::Date::now();
        if block {
            // P4.5's public setup: block step/wrap and transaction wrap only.
            // Do not load the transaction step, merge or zkApp circuits.
            BlockProver::make(block_verifier, Some(verifier))
                .map_err(|error| format!("block provers: {error:#}"))?;
            BLOCK_READY.store(true, Release);
        } else {
            TransactionProver::make(Some(verifier))
                .map_err(|error| format!("transaction provers: {error:#}"))?;
        }
        samples.push(sample("blobs_and_prover_indexes", start));
        Ok::<_, String>(serde_json::to_string(&samples).unwrap())
    })
    .await?
    .map_err(js_error)
}

#[wasm_bindgen]
pub struct ProofResult {
    proof: Vec<u8>,
    metrics: String,
}

#[wasm_bindgen]
impl ProofResult {
    pub fn take_proof(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.proof)
    }

    pub fn metrics(&self) -> String {
        self.metrics.clone()
    }
}

#[wasm_bindgen]
pub async fn prove(input: Vec<u8>) -> Result<ProofResult, JsValue> {
    worker(move || -> anyhow::Result<ProofResult> {
        let mut samples = Vec::new();
        let start = js_sys::Date::now();
        let (statement, tx_witness) = crate::decode::<crate::Input>(&input)?;
        samples.push(sample("decode_input", start));
        let verifier =
            TransactionVerifier::get().ok_or_else(|| anyhow::anyhow!("call setup first"))?;
        let provers = TransactionProver::make(Some(verifier))?;
        let message = crate::message(0);
        PEAK.store(LIVE.load(Relaxed), Relaxed);
        let start = js_sys::Date::now();
        let proof = generate_tx_proof(TransactionParams {
            statement: &statement,
            tx_witness: &tx_witness,
            message: &message,
            tx_step_prover: &provers.tx_step_prover,
            tx_wrap_prover: &provers.tx_wrap_prover,
            only_verify_constraints: false,
            expected_step_proof: None,
            ocaml_wrap_witness: None,
        })?;
        samples.push(sample("prove_step_and_wrap", start));
        let start = js_sys::Date::now();
        let proof = v2::TransactionSnarkProofStableV2((&proof).into());
        let mut bytes = Vec::new();
        proof.binprot_write(&mut bytes)?;
        samples.push(sample("encode_proof", start));
        Ok(ProofResult {
            proof: bytes,
            metrics: serde_json::to_string(&samples)?,
        })
    })
    .await?
    .map_err(js_error)
}

#[wasm_bindgen]
pub async fn prove_block(input: Vec<u8>) -> Result<ProofResult, JsValue> {
    worker(move || -> anyhow::Result<ProofResult> {
        let mut samples = Vec::new();
        let start = js_sys::Date::now();
        let input = crate::decode::<v2::ProverExtendBlockchainInputStableV2>(&input)?;
        samples.push(sample("decode_input", start));
        anyhow::ensure!(BLOCK_READY.load(Acquire), "call block setup first");
        let provers = BlockProver::get_once_made();
        PEAK.store(LIVE.load(Relaxed), Relaxed);
        let start = js_sys::Date::now();
        let proof = generate_block_proof(BlockParams {
            input: &input,
            block_step_prover: &provers.block_step_prover,
            block_wrap_prover: &provers.block_wrap_prover,
            tx_wrap_prover: &provers.tx_wrap_prover,
            only_verify_constraints: false,
            expected_step_proof: None,
            ocaml_wrap_witness: None,
        })?;
        samples.push(sample("prove_step_and_wrap", start));
        let start = js_sys::Date::now();
        let proof = v2::MinaBaseProofStableV2((&proof).into());
        let mut bytes = Vec::new();
        proof.binprot_write(&mut bytes)?;
        samples.push(sample("encode_proof", start));
        Ok(ProofResult {
            proof: bytes,
            metrics: serde_json::to_string(&samples)?,
        })
    })
    .await?
    .map_err(js_error)
}
