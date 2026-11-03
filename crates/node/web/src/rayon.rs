use std::sync::atomic::{AtomicU8, Ordering};

use mina_node::core::thread::{self, PoolState};
use wasm_bindgen::{JsCast, JsValue};

const PENDING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;

/// Where `init_rayon` is: building the global pool, done, or failed.
static POOL: AtomicU8 = AtomicU8::new(PENDING);

/// How long `pool_ready` waits for the pool. Building it takes well under a
/// second on a desktop.
pub const POOL_WAIT_MS: f64 = 20_000.0;

/// Whether `init_rayon` has built the global pool, which it starts building
/// when the module loads, without anyone waiting for it. Before that, a
/// parallel iterator would make rayon build its default pool, whose threads
/// can't be spawned here, and `init_rayon` would then fail. False when the
/// pool failed, or isn't built after `limit_ms`: the caller then does its work
/// without the pool, so that the node starts as it did before the pool was
/// waited for.
pub async fn pool_ready(limit_ms: f64) -> bool {
    let start = monotonic_ms();
    loop {
        let waited = monotonic_ms() - start;
        let state = match POOL.load(Ordering::Acquire) {
            READY => PoolState::Ready,
            FAILED => PoolState::Failed,
            _ => PoolState::Building,
        };
        if let Some(ready) = thread::pool_wait_outcome(state, waited, limit_ms) {
            return ready;
        }
        gloo_timers::future::TimeoutFuture::new(5).await;
    }
}

/// The global scope's `performance.now()`, in a window or a worker: unlike
/// `Date.now()`, it doesn't move with the wall clock. `Date.now()` where
/// there is none.
fn monotonic_ms() -> f64 {
    let performance = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("performance"));
    performance
        .ok()
        .and_then(|performance| {
            let now = js_sys::Reflect::get(&performance, &JsValue::from_str("now")).ok()?;
            let now: js_sys::Function = now.dyn_into().ok()?;
            now.call0(&performance).ok()?.as_f64()
        })
        .unwrap_or_else(js_sys::Date::now)
}

/// This method must be called to initialize rayon.
/// This is an async function, and the verification code must be called only after `init_rayon` returned.
/// This must not be called from the main thread.
pub async fn init_rayon() -> Result<(), JsValue> {
    let built = build_pool().await;
    POOL.store(
        if built.is_ok() { READY } else { FAILED },
        Ordering::Release,
    );
    built
}

async fn build_pool() -> Result<(), JsValue> {
    let num_cpus = thread::available_parallelism()
        .map_err(|err| format!("failed to get available parallelism: {err}"))?
        .get();

    thread::spawn(move || {
        rayon::ThreadPoolBuilder::new()
            .spawn_handler(|thread| {
                thread::spawn(move || thread.run());
                Ok(())
            })
            .num_threads(num_cpus.max(2) - 1)
            .build_global()
            .map_err(|e| format!("{:?}", e))
    })
    .join_async()
    .await
    .map_err(|_| JsValue::from_str("the thread that builds the rayon pool panicked"))??;

    Ok(())
}
