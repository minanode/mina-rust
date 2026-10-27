//! Clean shutdown during a frontier store restore (RC7-FIXES).
//!
//! The loader counts a restore attempt before it reads the store, so that a
//! store that crashes the node on every restore is given up after a few
//! tries. `docker stop` (SIGTERM, then SIGKILL 10 seconds later) or Ctrl-C
//! during a restore isn't the store's fault and must not count. A thread
//! waits for SIGTERM and SIGINT, takes back the attempt of a restore in
//! progress, then ends the process with the signal's default action, as if
//! the thread weren't there: same exit status, no other shutdown work.
//! Outside a restore it stops at once. During one, the take-back has
//! [`TAKE_BACK_DEADLINE`], and a second TERM or INT stops the node at once
//! (RC7-FIXES review).

use std::{
    sync::{Mutex, Once},
    time::Duration,
};

use mina_node::persist::load::RestoreGuard;
use nix::sys::signal::{raise, signal as set_handler, SigHandler, Signal};
use tokio::signal::unix::{signal, Signal as SignalStream, SignalKind};

static GUARDS: Mutex<Vec<RestoreGuard>> = Mutex::new(Vec::new());

/// Takes back the counted attempt of `guard`'s restore, if one is in
/// progress, when the process gets SIGTERM or SIGINT.
pub fn take_back_restore_on_shutdown(guard: RestoreGuard) {
    GUARDS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push(guard);
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("shutdown-signals".into())
            .spawn(wait_for_shutdown);
        if let Err(error) = spawned {
            mina_core::warn!(
                mina_core::log::system_time();
                kind = "ShutdownSignals",
                summary = "no shutdown signal thread: a stop during a restore counts as a failed restore",
                error = error.to_string(),
            );
        }
    });
}

/// How long a take-back may take before the node stops anyway: well inside
/// `docker stop`'s 10 seconds. A disk that stalls longer leaves the attempt
/// counted.
const TAKE_BACK_DEADLINE: Duration = Duration::from_millis(2500);

fn wait_for_shutdown() {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            mina_core::warn!(
                mina_core::log::system_time();
                kind = "ShutdownSignals",
                summary = "no shutdown signal runtime: a stop during a restore counts as a failed restore",
                error = error.to_string(),
            );
            return;
        }
    };
    runtime.block_on(async {
        // A signal whose handler isn't installed keeps its default action.
        let mut term = signal(SignalKind::terminate()).ok();
        let mut int = signal(SignalKind::interrupt()).ok();
        let Some(received) = next_signal(&mut term, &mut int).await else {
            return;
        };
        let guards = GUARDS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        // Outside a restore there is nothing to take back: stop at once, as
        // the default action would. This thread never waits for a guard.
        let idle = guards
            .iter()
            .fold(true, |idle, guard| guard.shutdown_if_idle() && idle);
        if !idle {
            take_back_bounded(guards, received, &mut term, &mut int).await;
        }
        terminate(received);
    });
}

/// Takes the restore attempts back on their own thread, so that this one
/// keeps receiving signals: a second TERM or INT, or the deadline, ends the
/// wait at once, whatever the disk is doing.
async fn take_back_bounded(
    guards: Vec<RestoreGuard>,
    received: Signal,
    term: &mut Option<SignalStream>,
    int: &mut Option<SignalStream>,
) {
    let (done, taken_back) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("shutdown-take-back".into())
        .spawn(move || {
            let _ = done.send(take_back(guards, received));
        });
    if spawned.is_err() {
        return;
    }
    let wait = async {
        match tokio::time::timeout(TAKE_BACK_DEADLINE, taken_back).await {
            // The file logger writes on its own thread.
            Ok(Ok(true)) => tokio::time::sleep(Duration::from_millis(100)).await,
            Ok(_) => {}
            Err(_) => mina_core::warn!(
                mina_core::log::system_time();
                kind = "PersistRestoreStopped",
                summary = format!(
                    "{received} during a frontier store restore: the attempt wasn't taken back within {} s, stopping",
                    TAKE_BACK_DEADLINE.as_secs_f32()
                ),
            ),
        }
    };
    tokio::select! {
        () = wait => {}
        Some(_) = next_signal(term, int) => {}
    }
}

/// Runs every armed take-back and logs each; whether one succeeded.
fn take_back(guards: Vec<RestoreGuard>, received: Signal) -> bool {
    let mut took_back = false;
    for guard in guards {
        match guard.take_back() {
            None => {}
            Some(Ok(())) => {
                took_back = true;
                mina_core::info!(
                    mina_core::log::system_time();
                    kind = "PersistRestoreStopped",
                    summary = format!("{received} during a frontier store restore: the attempt doesn't count"),
                );
            }
            Some(Err(error)) => {
                mina_core::warn!(
                    mina_core::log::system_time();
                    kind = "PersistRestoreStopped",
                    summary = format!("{received} during a frontier store restore: the attempt wasn't taken back"),
                    error = error.to_string(),
                );
            }
        }
    }
    took_back
}

/// The next TERM or INT; `None` if neither handler is installed.
async fn next_signal(
    term: &mut Option<SignalStream>,
    int: &mut Option<SignalStream>,
) -> Option<Signal> {
    tokio::select! {
        Some(()) = async { term.as_mut()?.recv().await } => Some(Signal::SIGTERM),
        Some(()) = async { int.as_mut()?.recv().await } => Some(Signal::SIGINT),
        else => None,
    }
}

/// Ends the process by `received`'s default action: the exit status says
/// which signal stopped the node, as without this thread.
fn terminate(received: Signal) -> ! {
    // SAFETY: restoring the default disposition installs no handler.
    let _ = unsafe { set_handler(received, SigHandler::SigDfl) };
    let _ = raise(received);
    // Not reached: the default action of TERM and INT ends the process.
    std::process::exit(128 + received as i32)
}
