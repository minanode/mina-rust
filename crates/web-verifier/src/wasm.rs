//! JavaScript bindings.
//!
//! The browser's main thread may not block, and rayon (used by the verifier) blocks
//! while waiting for its workers. So every expensive call runs on a spawned Web
//! Worker thread and returns a `Promise`, and the rayon pool runs on further
//! workers. The module must be built with shared memory (see `build-wasm.sh`) and
//! served cross-origin isolated (COOP/COEP) for `SharedArrayBuffer` to exist.

use std::sync::{Arc, Mutex};

use js_sys::Promise;
use mina_core::thread;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use crate::{Error, ForkChoice, VerifiedBlock, Verifier};

#[wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
}

/// Run `f` on a new worker thread and wait for it without blocking the caller.
async fn off_main_thread<T, F>(f: F) -> Result<T, JsValue>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    thread::spawn(f)
        .join_async()
        .await
        .map_err(|_| JsValue::from_str("worker thread panicked"))
}

/// Start the global rayon pool with `num_threads` workers. Call once, before
/// creating a verifier.
#[wasm_bindgen(js_name = initThreadPool)]
pub async fn init_thread_pool(num_threads: usize) -> Result<(), JsValue> {
    off_main_thread(move || {
        rayon::ThreadPoolBuilder::new()
            .spawn_handler(|worker| {
                thread::spawn(move || worker.run());
                Ok(())
            })
            .num_threads(num_threads.max(1))
            .build_global()
            .map_err(|e| e.to_string())
    })
    .await?
    .map_err(|e| JsValue::from_str(&e))
}

/// Build a verifier from a verification-key JSON (the GraphQL
/// `blockchainVerificationKey` form or a mina-tree verifier index).
#[wasm_bindgen(js_name = createVerifier)]
pub async fn create_verifier(vk_json: String) -> Result<WebVerifier, JsValue> {
    let inner = off_main_thread(move || Verifier::from_vk_json(&vk_json))
        .await?
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(WebVerifier {
        inner,
        fork_choice: Arc::default(),
    })
}

/// A verifier plus fork choice over every block it has verified.
#[wasm_bindgen]
pub struct WebVerifier {
    inner: Verifier,
    fork_choice: Arc<Mutex<ForkChoice>>,
}

/// Offer a verified block to fork choice and describe both as JSON: the block's
/// facts plus `best_tip` (whether it became the best tip) and
/// `best_tip_state_hash`. Runs on a worker, so it returns strings, not `JsValue`s.
fn offer(
    verified: Result<VerifiedBlock, Error>,
    fork_choice: &Mutex<ForkChoice>,
) -> Result<String, String> {
    let block = verified.map_err(|e| e.to_string())?;
    let mut fork_choice = fork_choice.lock().unwrap();
    let best_tip = fork_choice.consider(&block);
    let mut json = serde_json::to_value(&block.facts).map_err(|e| e.to_string())?;
    json["best_tip"] = best_tip.into();
    json["best_tip_state_hash"] = fork_choice
        .best()
        .map(|best| best.facts.state_hash.clone())
        .into();
    Ok(json.to_string())
}

fn to_js(result: Result<String, String>) -> Result<JsValue, JsValue> {
    result
        .map(|json| JsValue::from_str(&json))
        .map_err(|e| JsValue::from_str(&e))
}

#[wasm_bindgen]
impl WebVerifier {
    /// Build the global SRS used when checking proofs. Resolves when done.
    #[wasm_bindgen(js_name = warmUp)]
    pub fn warm_up(&self) -> Promise {
        let verifier = self.inner.clone();
        future_to_promise(async move {
            off_main_thread(move || verifier.warm_up()).await?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Verify a precomputed block's proof. Resolves with the block's proof-attested
    /// facts as a JSON string, or rejects with the reason it failed.
    #[wasm_bindgen(js_name = verifyPrecomputed)]
    pub fn verify_precomputed(&self, block_json: String) -> Promise {
        let verifier = self.inner.clone();
        let fork_choice = self.fork_choice.clone();
        future_to_promise(async move {
            to_js(
                off_main_thread(move || {
                    offer(verifier.verify_precomputed(&block_json), &fork_choice)
                })
                .await?,
            )
        })
    }

    /// Verify a block received over libp2p gossip (the raw payload, length prefix
    /// included). Resolves and rejects like `verifyPrecomputed`.
    #[wasm_bindgen(js_name = verifyGossipBlock)]
    pub fn verify_gossip_block(&self, payload: Vec<u8>) -> Promise {
        let verifier = self.inner.clone();
        let fork_choice = self.fork_choice.clone();
        future_to_promise(async move {
            to_js(
                off_main_thread(move || offer(verifier.verify_gossip(&payload), &fork_choice))
                    .await?,
            )
        })
    }
}
