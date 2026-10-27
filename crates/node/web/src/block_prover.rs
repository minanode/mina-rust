//! The web node's isolated prover (I34c, P5.3).
//!
//! A Mesa block proof needs about 2.5 GiB of prover memory (I34b), and the
//! web node's own WASM memory is capped at 4 GiB, so the web node never
//! proves in its own memory. When block production or the SNARK worker is
//! enabled it creates a hidden, same-origin iframe that instantiates this
//! same module a second time, with a fresh memory and its own worker pool,
//! and loads the circuits there at once (the prewarm): the block circuits
//! for a producer, or the transaction circuits for a SNARK worker (P5.3),
//! never both: a block producer's prover refuses the SNARK worker, so that
//! no block proof waits behind SNARK work. (The realm can still load both,
//! for `tools/webnode-snark-worker/measure.mjs`.) Requests and proofs cross
//! between the two memories as owned bytes; see `mina_node_common::
//! block_producer::isolated` for the protocol and its limits, and
//! `js/block_prover_host.js` for the iframe's lifecycle.
//!
//! Two halves live here:
//!
//! - the **transport**, on the node's main thread, which carries the bridge's
//!   commands to `js/block_prover_host.js` and its answers back to the bridge
//!   thread, plus a tick each second for deadlines;
//! - the **prover realm**'s entry points, `block_prover_realm_*`, which only
//!   the iframe calls. They run all work in that instance's workers, never
//!   on its main thread, which is the page's main thread too.

use std::sync::atomic::{AtomicBool, Ordering};

use js_sys::Uint8Array;
use ledger::{
    proofs::{
        block::BlockParams,
        generate_block_proof,
        provers::{BlockProver, TransactionProver},
        snark_worker::{is_zkapp_command, prove_work, spec_jobs, WorkProvers},
        verifiers::{BlockVerifier, TransactionVerifier},
    },
    scan_state::scan_state::transaction_snark::SokMessage,
};
use mina_node::{
    account::AccountSecretKey,
    core::{channels::mpsc, thread},
};
use mina_node_common::{
    block_producer::isolated::{
        self, BlockProofChecker, BridgeInput, BridgeInputSender, Command, ProveRequest,
        ProveResponse, ProverKey, WorkRequest, WorkResponse, BRIDGE_VERSION, MAX_REQUEST_BYTES,
        MAX_RESPONSE_BYTES,
    },
    NodeServiceCommonBuilder,
};
use mina_p2p_messages::v2;
use once_cell::sync::OnceCell;
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::{spawn_local, JsFuture};

#[wasm_bindgen(module = "/js/block_prover_host.js")]
extern "C" {
    /// `key` is the encoded producer key for a host that proves blocks, or
    /// null for one that only proves SNARK work.
    #[wasm_bindgen(js_name = startBlockProverHost)]
    fn start_block_prover_host(
        glue_url: &str,
        generation: f64,
        network: &str,
        threads: u32,
        key: JsValue,
        block: bool,
        work: bool,
        on_failure: &js_sys::Function,
    ) -> js_sys::Promise;

    #[wasm_bindgen(js_name = proveWorkWithBlockProverHost)]
    fn prove_work_with_block_prover_host(
        generation: f64,
        request_id: f64,
        timeout_ms: f64,
        request: Uint8Array,
    ) -> js_sys::Promise;

    #[wasm_bindgen(js_name = proveWithBlockProverHost)]
    fn prove_with_block_prover_host(
        generation: f64,
        request_id: f64,
        deadline_ms: f64,
        is_genesis: bool,
        request: Uint8Array,
    ) -> js_sys::Promise;

    #[wasm_bindgen(js_name = retireBlockProverHost)]
    fn retire_block_prover_host(generation: f64, reason: &str) -> js_sys::Promise;
}

/// The global that marks the prover iframe's realm, set by its document
/// before it imports this module.
const PROVER_REALM_FLAG: &str = "__MINA_BLOCK_PROVER_REALM__";

pub fn in_prover_realm() -> bool {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(PROVER_REALM_FLAG))
        .is_ok_and(|flag| flag.is_truthy())
}

/// Starts the bridge and the transport for its commands: for the block
/// producer, with this node's producer key and block verifier, or else for
/// the SNARK worker if `work` (P5.3). Called while the node is built, on the
/// node's thread.
pub(crate) fn start(
    service: &mut NodeServiceCommonBuilder,
    producer: Option<(AccountSecretKey, BlockProofChecker)>,
    work: bool,
    network: String,
) {
    // A block producer's bridge refuses the SNARK worker: don't load its
    // circuits.
    let work = work && producer.is_none();
    let (command_tx, command_rx) = mpsc::unbounded_channel::<Command>();
    let prover_key = producer.as_ref().map(|(key, _)| {
        isolated::encode(&ProverKey::new(key)).expect("a key message has no cached proof")
    });
    let checker = producer.as_ref().map(|(_, checker)| checker.clone());
    let input = service.isolated_prover_init(checker, move |command| {
        let _ = command_tx.send(command);
    });
    if let Some((key, _)) = producer {
        service.block_producer_init_isolated(key);
    }
    thread::start_task_in_main_thread(async move {
        spawn_local(transport(command_rx, input, prover_key, work, network));
    });
}

/// A copy in a standalone (not shared, so transferable) JS buffer.
fn js_bytes(bytes: &[u8]) -> Uint8Array {
    let array = Uint8Array::new_with_length(bytes.len() as u32);
    array.copy_from(bytes);
    array
}

fn js_error_text(error: &JsValue) -> String {
    error
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .or_else(|| error.as_string())
        .unwrap_or_else(|| format!("{error:?}"))
}

fn js_error_is_fatal(error: &JsValue) -> bool {
    js_sys::Reflect::get(error, &JsValue::from_str("fatal")).is_ok_and(|v| v.is_truthy())
}

/// Prover threads: at most 8 (I34b measured 4 and 8), one core left to the
/// node. `globalThis.MINA_BLOCK_PROVER_THREADS` overrides it.
fn prover_threads() -> u32 {
    let configured = js_sys::Reflect::get(
        &js_sys::global(),
        &JsValue::from_str("MINA_BLOCK_PROVER_THREADS"),
    )
    .ok()
    .and_then(|v| v.as_f64())
    .filter(|n| *n >= 1.0);
    if let Some(threads) = configured {
        return (threads as u32).min(32);
    }
    let cores = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    cores.saturating_sub(1).clamp(1, 8) as u32
}

async fn transport(
    mut commands: mpsc::UnboundedReceiver<Command>,
    input: BridgeInputSender,
    prover_key: Option<Vec<u8>>,
    work: bool,
    network: String,
) {
    let glue_url = thread::get_wasm_bindgen_shim_script_path();
    let threads = prover_threads();
    let configured_ms = |name: &str, fallback: u64| {
        js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(name))
            .ok()
            .and_then(|v| v.as_f64())
            .filter(|n| n.is_finite() && *n >= 0.0)
            .map_or(fallback, |n| n as u64)
    };
    let defaults = isolated::ProofBudget::default();
    let _ = input.send(BridgeInput::SetProofBudget(isolated::ProofBudget {
        proof_ms: configured_ms("MINA_BLOCK_PROVER_PROOF_MS", defaults.proof_ms),
        margin_ms: configured_ms("MINA_BLOCK_PROVER_MARGIN_MS", defaults.margin_ms),
    }));
    let failure_input = input.clone();
    let on_failure = Closure::<dyn FnMut(f64, String)>::new(move |generation: f64, reason| {
        let _ = failure_input.send(BridgeInput::HostFailed {
            generation: generation as u64,
            reason,
        });
    });
    let tick_input = input.clone();
    spawn_local(async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(1_000).await;
            if tick_input.send(BridgeInput::Tick).is_err() {
                return;
            }
        }
    });
    while let Some(command) = commands.recv().await {
        match command {
            Command::Start { generation } => {
                let key = prover_key
                    .as_deref()
                    .map_or(JsValue::NULL, |key| js_bytes(key).into());
                let promise = start_block_prover_host(
                    &glue_url,
                    generation as f64,
                    &network,
                    threads,
                    key,
                    prover_key.is_some(),
                    work,
                    on_failure.as_ref().unchecked_ref(),
                );
                let input = input.clone();
                spawn_local(async move {
                    let message = match JsFuture::from(promise).await {
                        Ok(report) => BridgeInput::HostReady {
                            generation,
                            report: report.as_string().unwrap_or_default(),
                        },
                        Err(error) => BridgeInput::HostFailed {
                            generation,
                            reason: js_error_text(&error),
                        },
                    };
                    let _ = input.send(message);
                });
            }
            Command::Prove {
                generation,
                request_id,
                deadline_ms,
                is_genesis,
                bytes,
            } => {
                let promise = prove_with_block_prover_host(
                    generation as f64,
                    request_id as f64,
                    deadline_ms as f64,
                    is_genesis,
                    js_bytes(&bytes),
                );
                drop(bytes);
                spawn_local(answer(input.clone(), generation, request_id, promise));
            }
            Command::ProveWork {
                generation,
                request_id,
                timeout_ms,
                bytes,
            } => {
                let promise = prove_work_with_block_prover_host(
                    generation as f64,
                    request_id as f64,
                    timeout_ms as f64,
                    js_bytes(&bytes),
                );
                drop(bytes);
                spawn_local(answer(input.clone(), generation, request_id, promise));
            }
            Command::Retire { generation, reason } => {
                let _ = retire_block_prover_host(generation as f64, &reason);
            }
        }
    }
    drop(on_failure);
}

/// Carries a host's answer to a block or work request back to the bridge.
async fn answer(
    input: BridgeInputSender,
    generation: u64,
    request_id: u64,
    promise: js_sys::Promise,
) {
    let message = match JsFuture::from(promise).await {
        Ok(response) => {
            let response: Uint8Array = response.unchecked_into();
            // Checked before anything is copied into this memory.
            if response.length() as usize > MAX_RESPONSE_BYTES {
                BridgeInput::HostFailed {
                    generation,
                    reason: format!("response of {} bytes exceeds the limit", response.length()),
                }
            } else {
                BridgeInput::Response {
                    generation,
                    request_id,
                    bytes: response.to_vec(),
                }
            }
        }
        Err(error) if js_error_is_fatal(&error) => BridgeInput::HostFailed {
            generation,
            reason: js_error_text(&error),
        },
        Err(error) => BridgeInput::ProveError {
            generation,
            request_id,
            error: js_error_text(&error),
        },
    };
    let _ = input.send(message);
}

// The prover realm. Everything below runs in the iframe's instance.

/// Set up, whatever for. Then `REALM_BLOCK` and `REALM_WORK` say for what.
static REALM_READY: AtomicBool = AtomicBool::new(false);
/// The block circuits are loaded.
static REALM_BLOCK: AtomicBool = AtomicBool::new(false);
/// The transaction circuits are loaded (P5.3).
static REALM_WORK: AtomicBool = AtomicBool::new(false);
static REALM_KEY: OnceCell<v2::SignatureLibPrivateKeyStableV1> = OnceCell::new();

fn js_error(error: impl std::fmt::Display) -> JsValue {
    js_sys::Error::new(&error.to_string()).into()
}

fn realm_check() -> Result<(), JsValue> {
    if in_prover_realm() {
        Ok(())
    } else {
        Err(js_error("only the block prover iframe proves"))
    }
}

type RealmJob = Box<dyn FnOnce() + Send>;

/// The instance's request worker, started on first use. Every setup step
/// and request runs on it, one at a time.
static REALM_WORKER: OnceCell<std::sync::mpsc::Sender<RealmJob>> = OnceCell::new();

/// Runs `f` on this instance's request worker: never on the main thread,
/// which this same-origin iframe shares with the node's page. One worker for
/// the instance's life, not one per request (P5.3): I34c saw the instance
/// grow about 10 MiB a block proof with a new worker each, and a SNARK
/// worker sends a request every minute or so.
async fn in_worker<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, JsValue> {
    let worker = REALM_WORKER.get_or_init(|| {
        let (jobs, requests) = std::sync::mpsc::channel::<RealmJob>();
        thread::spawn(move || {
            for job in requests {
                job();
            }
        });
        jobs
    });
    let (answer, answered) = futures::channel::oneshot::channel();
    worker
        .send(Box::new(move || {
            let _ = answer.send(f());
        }))
        .map_err(|_| js_error("block prover worker stopped"))?;
    answered
        .await
        .map_err(|_| js_error("block prover worker panicked"))
}

fn elapsed_ms(start: f64) -> f64 {
    js_sys::Date::now() - start
}

/// Builds this instance's worker pool, then loads the prover SRS and what
/// the instance proves: for blocks (`block`), the block verifier key and the
/// block step, block wrap and transaction wrap provers; for SNARK work
/// (`work`, P5.3), the transaction step and merge step provers too, and the
/// transaction wrap prover a block prover shares. Both need the transaction
/// verifier key. Never the zkApp provers (P5.1). Returns phase times and
/// the linear memory after each as JSON.
#[wasm_bindgen]
pub async fn block_prover_realm_setup(
    network: String,
    threads: u32,
    block: bool,
    work: bool,
) -> Result<String, JsValue> {
    realm_check()?;
    if REALM_READY.load(Ordering::Acquire) {
        return Err(js_error("block prover already set up"));
    }
    if !block && !work {
        return Err(js_error(
            "a prover instance proves blocks, SNARK work or both",
        ));
    }
    mina_node::core::NetworkConfig::init(&network).map_err(js_error)?;
    let threads = threads.clamp(1, 32) as usize;
    let start = js_sys::Date::now();
    in_worker(move || {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .spawn_handler(|worker| {
                thread::spawn(move || worker.run());
                Ok(())
            })
            .build_global()
            .map_err(|e| e.to_string())
    })
    .await?
    .map_err(js_error)?;
    let pool_ms = elapsed_ms(start);
    let report = in_worker(move || {
        let mut report = serde_json::json!({
            "threads": threads,
            "block": block,
            "work": work,
            "pool_ms": pool_ms,
        });
        let mut phase = |name: &str, start: f64| {
            report[format!("{name}_ms")] = elapsed_ms(start).into();
            report[format!("{name}_linear_bytes")] = linear_bytes().into();
        };
        let start = js_sys::Date::now();
        let _ = ledger::verifier::get_srs_mut::<mina_curves::pasta::Fp>();
        let _ = ledger::verifier::get_srs_mut::<mina_curves::pasta::Fq>();
        phase("prover_srs", start);
        let start = js_sys::Date::now();
        let tx_verifier = futures::executor::block_on(TransactionVerifier::make());
        phase("transaction_verifier", start);
        if block {
            let start = js_sys::Date::now();
            let block_verifier = futures::executor::block_on(BlockVerifier::make());
            phase("block_verifier", start);
            let start = js_sys::Date::now();
            BlockProver::make(Some(block_verifier), Some(tx_verifier.clone()))
                .map_err(|error| format!("block provers: {error:#}"))?;
            phase("block_provers", start);
            REALM_BLOCK.store(true, Ordering::Release);
        }
        if work {
            // Shares the transaction wrap prover with the block provers.
            let start = js_sys::Date::now();
            TransactionProver::make(Some(tx_verifier))
                .map_err(|error| format!("transaction provers: {error:#}"))?;
            phase("transaction_provers", start);
            REALM_WORK.store(true, Ordering::Release);
        }
        REALM_READY.store(true, Ordering::Release);
        Ok::<_, String>(report.to_string())
    })
    .await?
    .map_err(js_error)?;
    Ok(report)
}

/// This instance's linear memory in bytes (it never shrinks).
fn linear_bytes() -> f64 {
    (core::arch::wasm32::memory_size::<0>() as f64) * 65536.0
}

/// Installs the producer key, once, before the first request.
#[wasm_bindgen]
pub fn block_prover_realm_install_key(bytes: &[u8]) -> Result<(), JsValue> {
    realm_check()?;
    let key = isolated::decode_exact::<ProverKey>(bytes, 1024).map_err(js_error)?;
    if key.version != BRIDGE_VERSION {
        return Err(js_error("producer key message has another bridge version"));
    }
    REALM_KEY
        .set(key.key)
        .map_err(|_| js_error("a producer key is already installed"))
}

fn prove_request(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let request = isolated::decode_exact::<ProveRequest>(bytes, MAX_REQUEST_BYTES)?;
    if request.version != BRIDGE_VERSION {
        return Err(format!(
            "request version {} is not {BRIDGE_VERSION}",
            request.version
        ));
    }
    if !REALM_BLOCK.load(Ordering::Acquire) {
        return Err("block prover is not set up".to_owned());
    }
    let mut input = request.input;
    // Requests carry no key. The genesis transition is proved with the
    // public genesis winner's key, as the node builds it
    // (`transition_frontier::genesis`), every other block with the
    // installed producer key.
    input.prover_state.producer_private_key = if request.is_genesis {
        AccountSecretKey::genesis_producer().into()
    } else {
        REALM_KEY
            .get()
            .cloned()
            .ok_or("no producer key installed")?
    };
    let provers = BlockProver::get_once_made();
    let proof = generate_block_proof(BlockParams {
        input: &input,
        block_step_prover: &provers.block_step_prover,
        block_wrap_prover: &provers.block_wrap_prover,
        tx_wrap_prover: &provers.tx_wrap_prover,
        only_verify_constraints: false,
        expected_step_proof: None,
        ocaml_wrap_witness: None,
    })
    // Display only: a kimchi error's Debug text carries witness data.
    .map_err(|error| format!("block proof failed: {error}"))?;
    drop(input);
    isolated::encode(&ProveResponse {
        version: BRIDGE_VERSION,
        request_id: request.request_id,
        block_hash: request.block_hash,
        proof: v2::MinaBaseProofStableV2((&proof).into()),
    })
}

/// Proves one encoded [`ProveRequest`] and returns the encoded
/// [`ProveResponse`]. An error leaves the instance usable.
#[wasm_bindgen]
pub async fn block_prover_realm_prove(request: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    realm_check()?;
    in_worker(move || prove_request(&request))
        .await?
        .map_err(js_error)
}

fn prove_work_request(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let request = isolated::decode_exact::<WorkRequest>(bytes, MAX_REQUEST_BYTES)?;
    if request.version != BRIDGE_VERSION {
        return Err(format!(
            "request version {} is not {BRIDGE_VERSION}",
            request.version
        ));
    }
    if !REALM_WORK.load(Ordering::Acquire) {
        return Err("this prover instance has no transaction circuits".to_owned());
    }
    let prover = mina_signer::CompressedPubKey::from_address(&request.prover.to_string())
        .map_err(|error| format!("prover key: {error}"))?;
    let message = SokMessage::create((&request.fee).into(), prover);
    // This instance loads no zkApp step circuits, and the web node's worker
    // selects no zkApp command job; one is refused before anything is
    // proved, rather than make those circuits here.
    if spec_jobs(&request.spec).into_iter().any(is_zkapp_command) {
        return Err("this prover instance has no zkApp circuits".to_owned());
    }
    // Made at setup: this returns the loaded transaction provers.
    let provers =
        WorkProvers::make(None).map_err(|error| format!("transaction provers: {error:#}"))?;
    // The worker core (P5.2): OCaml's `of_transition` for a base job,
    // `merge` for a merge.
    let proved = prove_work(&request.spec, &message, &provers)
        .map_err(|error| format!("SNARK work failed: {error}"))?;
    isolated::encode(&WorkResponse {
        version: BRIDGE_VERSION,
        request_id: request.request_id,
        proofs: proved.proofs,
        prove_ms: proved
            .prove_times
            .iter()
            .map(|time| time.as_millis() as u64)
            .collect(),
    })
}

/// Proves one encoded [`WorkRequest`] (P5.3) and returns the encoded
/// [`WorkResponse`]. The node checks the proofs before offering them. An
/// error leaves the instance usable.
#[wasm_bindgen]
pub async fn block_prover_realm_prove_work(request: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    realm_check()?;
    in_worker(move || prove_work_request(&request))
        .await?
        .map_err(js_error)
}

// Harness helpers: encoding only, callable from any realm. The measurement
// and gate harnesses build requests from OCaml's fixtures with them (P5.3).

/// An encoded [`WorkRequest`] for `spec`, a binprot SNARK work spec (one or
/// two jobs), proved for `fee` (in MINA) and the public key `prover`.
#[wasm_bindgen]
pub fn prover_bridge_work_request(
    request_id: f64,
    spec: &[u8],
    fee: &str,
    prover: &str,
) -> Result<Vec<u8>, JsValue> {
    let spec = isolated::decode_exact(spec, MAX_REQUEST_BYTES).map_err(js_error)?;
    let fee = mina_node::account::parse_snarker_fee(fee).map_err(js_error)?;
    let prover = mina_node::account::parse_snarker_public_key(prover).map_err(js_error)?;
    isolated::encode(&WorkRequest {
        version: BRIDGE_VERSION,
        request_id: request_id as u64,
        fee: v2::CurrencyFeeStableV1(v2::UnsignedExtendedUInt64Int64ForVersionTagsStableV1(
            fee.into(),
        )),
        prover: prover.into(),
        spec,
    })
    .map_err(js_error)
}

/// An encoded [`WorkRequest`] for one or two jobs given as P5.2's fixtures,
/// OCaml's own jobs: a `base` job is binprot `(statement, witness)`, a
/// `merge` job `(statement, proof, proof)`.
#[wasm_bindgen]
pub fn prover_bridge_work_request_from_jobs(
    request_id: f64,
    first_kind: &str,
    first: &[u8],
    second_kind: Option<String>,
    second: Option<Vec<u8>>,
    fee: &str,
    prover: &str,
) -> Result<Vec<u8>, JsValue> {
    use v2::SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single as Single;
    fn job(kind: &str, bytes: &[u8]) -> Result<Single, JsValue> {
        Ok(match kind {
            "base" => {
                let (statement, witness) = isolated::decode_exact::<(
                    v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement,
                    v2::TransactionWitnessStableV2,
                )>(bytes, MAX_REQUEST_BYTES)
                .map_err(js_error)?;
                Single::Transition(v2::MinaStateSnarkedLedgerStateStableV2(statement), witness)
            }
            "merge" => Single::Merge(Box::new(
                isolated::decode_exact(bytes, MAX_REQUEST_BYTES).map_err(js_error)?,
            )),
            other => return Err(js_error(format!("unknown job kind {other:?}"))),
        })
    }
    let first = job(first_kind, first)?;
    let spec = match (second_kind, second) {
        (Some(kind), Some(bytes)) => {
            mina_node::external_snark_worker::SnarkWorkSpec::Two((first, job(&kind, &bytes)?))
        }
        (None, None) => mina_node::external_snark_worker::SnarkWorkSpec::One(first),
        _ => return Err(js_error("a second job needs its kind and bytes")),
    };
    let encoded = isolated::encode(&spec).map_err(js_error)?;
    prover_bridge_work_request(request_id, &encoded, fee, prover)
}

/// The encoded producer key message for the public genesis winner's key,
/// the only key a harness may install (it proves the public genesis
/// transition with it).
#[wasm_bindgen]
pub fn prover_bridge_genesis_key_message() -> Vec<u8> {
    isolated::encode(&ProverKey::new(&AccountSecretKey::genesis_producer()))
        .expect("a key message has no cached proof")
}

/// An encoded block [`ProveRequest`] for `input`, a binprot prover input,
/// with a placeholder block hash, which the prover echoes.
#[wasm_bindgen]
pub fn prover_bridge_block_request(
    request_id: f64,
    input: &[u8],
    is_genesis: bool,
    deadline_ms: f64,
) -> Result<Vec<u8>, JsValue> {
    let mut input: v2::ProverExtendBlockchainInputStableV2 =
        isolated::decode_exact(input, MAX_REQUEST_BYTES).map_err(js_error)?;
    // As the bridge sends it: never with a key.
    input.prover_state.producer_private_key =
        v2::SignatureLibPrivateKeyStableV1(mina_p2p_messages::bigint::BigInt::one());
    isolated::encode(&ProveRequest {
        version: BRIDGE_VERSION,
        request_id: request_id as u64,
        block_hash: v2::DataHashLibStateHashStableV1(
            mina_p2p_messages::bigint::BigInt::from_bytes([0; 32]),
        )
        .into(),
        deadline_ms: deadline_ms as u64,
        is_genesis,
        input,
    })
    .map_err(js_error)
}
