#[cfg(not(target_arch = "wasm32"))]
pub use std::thread::*;
#[cfg(target_arch = "wasm32")]
pub use wasm_thread::*;

/// Where a rayon pool that its process builds after start is: the web node
/// builds its global pool on its own, without anyone waiting for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolState {
    Building,
    Ready,
    Failed,
}

/// Whether to stop waiting for a pool in `state`, `waited_ms` into a wait of
/// at most `limit_ms`: `Some(true)` when it is built, `Some(false)` when it
/// failed or still isn't built at the limit, `None` to wait on. A pool that is
/// built counts before the limit does.
pub fn pool_wait_outcome(state: PoolState, waited_ms: f64, limit_ms: f64) -> Option<bool> {
    match state {
        PoolState::Ready => Some(true),
        PoolState::Failed => Some(false),
        PoolState::Building if waited_ms >= limit_ms => Some(false),
        PoolState::Building => None,
    }
}

#[cfg(test)]
mod pool_wait_tests {
    use super::*;

    const LIMIT: f64 = 20_000.0;

    #[test]
    fn a_built_pool_is_ready_at_once() {
        assert_eq!(pool_wait_outcome(PoolState::Ready, 0.0, LIMIT), Some(true));
        // Even past the limit, or with none: a built pool is used.
        assert_eq!(
            pool_wait_outcome(PoolState::Ready, LIMIT * 2.0, LIMIT),
            Some(true)
        );
        assert_eq!(pool_wait_outcome(PoolState::Ready, 0.0, 0.0), Some(true));
    }

    #[test]
    fn a_failed_pool_is_given_up_at_once() {
        assert_eq!(
            pool_wait_outcome(PoolState::Failed, 0.0, LIMIT),
            Some(false)
        );
    }

    #[test]
    fn a_pool_still_building_is_waited_for_until_the_limit() {
        assert_eq!(pool_wait_outcome(PoolState::Building, 0.0, LIMIT), None);
        assert_eq!(
            pool_wait_outcome(PoolState::Building, LIMIT - 1.0, LIMIT),
            None
        );
        assert_eq!(
            pool_wait_outcome(PoolState::Building, LIMIT, LIMIT),
            Some(false)
        );
    }
}

#[cfg(target_family = "wasm")]
mod main_thread {
    use crate::channels::{mpsc, oneshot};
    use std::{future::Future, pin::Pin};

    pub type TaskForMainThread = Pin<Box<dyn 'static + Send + Future<Output = ()>>>;

    static MAIN_THREAD_TASK_SENDER: once_cell::sync::OnceCell<
        mpsc::UnboundedSender<TaskForMainThread>,
    > = once_cell::sync::OnceCell::new();

    pub fn main_thread_init() {
        assert!(
            !super::is_web_worker_thread(),
            "Must be called in the main thread!"
        );

        MAIN_THREAD_TASK_SENDER.get_or_init(|| {
            let (task_sender, mut task_receiver) = mpsc::unbounded_channel();
            wasm_bindgen_futures::spawn_local(async move {
                while let Some(task) = task_receiver.recv().await {
                    wasm_bindgen_futures::spawn_local(task);
                }
            });
            task_sender
        });
    }

    pub fn start_task_in_main_thread<F>(task: F)
    where
        F: 'static + Send + Future<Output = ()>,
    {
        let sender = MAIN_THREAD_TASK_SENDER
            .get()
            .expect("main thread not initialized");
        let _ = sender.send(Box::pin(task));
    }

    pub async fn run_task_in_main_thread<F, T>(task: F) -> Option<T>
    where
        T: 'static + Send,
        F: 'static + Send + Future<Output = T>,
    {
        let sender = MAIN_THREAD_TASK_SENDER
            .get()
            .expect("main thread not initialized");
        let (tx, rx) = oneshot::channel();
        let _ = sender.send(Box::pin(async move {
            let _ = tx.send(task.await);
        }));
        rx.await.ok()
    }

    pub async fn run_async_fn_in_main_thread<F, FU, T>(f: F) -> Option<T>
    where
        T: 'static + Send,
        FU: Future<Output = T>,
        F: 'static + Send + FnOnce() -> FU,
    {
        let sender = MAIN_THREAD_TASK_SENDER
            .get()
            .expect("main thread not initialized");
        let (tx, rx) = oneshot::channel();
        let _ = sender.send(Box::pin(async move {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = tx.send(f().await);
            })
        }));
        rx.await.ok()
    }

    pub fn run_async_fn_in_main_thread_blocking<F, FU, T>(f: F) -> Option<T>
    where
        T: 'static + Send,
        FU: Future<Output = T>,
        F: 'static + Send + FnOnce() -> FU,
    {
        let sender = MAIN_THREAD_TASK_SENDER
            .get()
            .expect("main thread not initialized");
        let (tx, rx) = oneshot::channel();
        let _ = sender.send(Box::pin(async move {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = tx.send(f().await);
            })
        }));
        rx.blocking_recv().ok()
    }
}
#[cfg(target_family = "wasm")]
pub use main_thread::*;
