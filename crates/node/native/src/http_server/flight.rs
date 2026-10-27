//! One run at a time of a costly answer (the bug report, the crash notice),
//! shared and bounded, never queued:
//!
//! - every request for the same thing while it runs gets that run's answer,
//!   up to [`MAX_WAITERS`] of them;
//! - a request for something else while it runs, one more than
//!   [`MAX_WAITERS`], or a new run while the last run's blocking work is
//!   still finishing gets [`FlightError::Busy`] at once;
//! - a run has a hard deadline, after which its requests get
//!   [`FlightError::Timeout`] and the next request may start another;
//! - an answer is kept for a moment after its run.
//!
//! The run is a task of its own, so it finishes (or times out) whether or
//! not its requests wait for it. The state is behind a plain mutex, held only
//! for a few instructions, never across an await.

use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

use tokio::sync::watch;

/// Requests that may wait for one run.
pub const MAX_WAITERS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlightError {
    /// Refused at once, with why.
    Busy(&'static str),
    /// The run took longer than this.
    Timeout(Duration),
    /// The run failed, with why.
    Failed(String),
}

type Outcome<V> = Option<Result<V, FlightError>>;

struct State<K, V> {
    cached: Option<(Instant, K, V)>,
    /// The run in flight: its id, what it makes, and where its answer comes.
    running: Option<(u64, K, watch::Receiver<Outcome<V>>)>,
    runs: u64,
    waiters: usize,
}

pub struct Flight<K, V> {
    state: Mutex<State<K, V>>,
    cache_for: Duration,
    deadline: Duration,
    blocking: Arc<AtomicBool>,
}

/// Marks a run's blocking work as running until it is dropped. A blocking
/// thread can't be stopped at the deadline: while it finishes, no new run
/// starts, so there is never more than one.
pub struct Blocking(Arc<AtomicBool>);

impl Blocking {
    /// Runs `work` on the blocking pool, marked as this flight's.
    pub async fn run<T: Send + 'static>(
        self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, String> {
        tokio::task::spawn_blocking(move || {
            let _marked = self;
            work()
        })
        .await
        .map_err(|error| error.to_string())
    }
}

impl Drop for Blocking {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Ends its run (the id) in the state when dropped, even if the run panicked:
/// a later run is never ended by an earlier one's guard.
struct Ended<K, V>(Arc<Flight<K, V>>, u64);

impl<K, V> Drop for Ended<K, V> {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        if state.running.as_ref().is_some_and(|(id, ..)| *id == self.1) {
            state.running = None;
        }
    }
}

/// Counts a waiting request until it is dropped (answered, or given up by
/// its client).
struct Waiting<'a, K, V>(&'a Flight<K, V>);

impl<K, V> Drop for Waiting<'_, K, V> {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.waiters = state.waiters.saturating_sub(1);
    }
}

impl<K, V> Flight<K, V> {
    fn lock(&self) -> MutexGuard<'_, State<K, V>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl<K, V> Flight<K, V>
where
    K: Clone + PartialEq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub fn new(cache_for: Duration, deadline: Duration) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                cached: None,
                running: None,
                runs: 0,
                waiters: 0,
            }),
            cache_for,
            deadline,
            blocking: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The answer for `key`: kept, in flight, or from a new run of `run`,
    /// which gets the [`Blocking`] mark for any blocking work it does.
    pub async fn get<F>(
        self: &Arc<Self>,
        key: K,
        run: impl FnOnce(Blocking) -> F,
    ) -> Result<V, FlightError>
    where
        F: Future<Output = Result<V, String>> + Send + 'static,
    {
        let (mut answer, _waiting) = {
            let mut state = self.lock();
            if let Some((at, cached, value)) = &state.cached {
                if *cached == key && at.elapsed() < self.cache_for {
                    return Ok(value.clone());
                }
            }
            let answer = match &state.running {
                Some((_, running, answer)) if *running == key => answer.clone(),
                Some(_) => return Err(FlightError::Busy("another one is being made")),
                None => {
                    if self.blocking.swap(true, Ordering::SeqCst) {
                        return Err(FlightError::Busy("the last one is still finishing"));
                    }
                    let work = run(Blocking(self.blocking.clone()));
                    let (sender, answer) = watch::channel(None);
                    state.runs += 1;
                    let id = state.runs;
                    state.running = Some((id, key.clone(), answer.clone()));
                    let flight = self.clone();
                    tokio::spawn(async move {
                        let ended = Ended(flight.clone(), id);
                        let outcome = match tokio::time::timeout(flight.deadline, work).await {
                            Ok(Ok(value)) => Ok(value),
                            Ok(Err(error)) => Err(FlightError::Failed(error)),
                            Err(_) => Err(FlightError::Timeout(flight.deadline)),
                        };
                        if let Ok(value) = &outcome {
                            flight.lock().cached = Some((Instant::now(), key, value.clone()));
                        }
                        drop(ended);
                        let _ = sender.send(Some(outcome));
                    });
                    answer
                }
            };
            if state.waiters >= MAX_WAITERS {
                return Err(FlightError::Busy("too many requests wait for it"));
            }
            state.waiters += 1;
            (answer, Waiting(self.as_ref()))
        };
        let outcome = answer
            .wait_for(Option::is_some)
            .await
            .map(|outcome| outcome.clone());
        match outcome {
            Ok(Some(result)) => result,
            _ => Err(FlightError::Failed("the run stopped".to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    const LONG: Duration = Duration::from_secs(60);

    #[tokio::test]
    async fn concurrent_requests_share_one_run() {
        let flight = Flight::<u8, u64>::new(LONG, LONG);
        let runs = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Notify::new());
        let requests = (0..10)
            .map(|_| {
                let (flight, runs, gate) = (flight.clone(), runs.clone(), gate.clone());
                tokio::spawn(async move {
                    flight
                        .get(1, move |_| async move {
                            runs.fetch_add(1, Ordering::SeqCst);
                            gate.notified().await;
                            Ok(42)
                        })
                        .await
                })
            })
            .collect::<Vec<_>>();
        tokio::time::sleep(Duration::from_millis(50)).await;
        gate.notify_waiters();
        for request in requests {
            assert_eq!(request.await.unwrap(), Ok(42));
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        // Kept: the next request runs nothing.
        let again = flight.get(1, |_| async { Ok(7) }).await;
        assert_eq!(again, Ok(42));
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_different_request_or_one_too_many_is_refused_not_queued() {
        let flight = Flight::<u8, u64>::new(LONG, LONG);
        let gate = Arc::new(tokio::sync::Notify::new());
        let held = gate.clone();
        let first = {
            let flight = flight.clone();
            tokio::spawn(async move {
                flight
                    .get(1, move |_| async move {
                        held.notified().await;
                        Ok(1)
                    })
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            flight.get(2, |_| async { Ok(2) }).await,
            Err(FlightError::Busy("another one is being made"))
        );
        // MAX_WAITERS wait in all (the first among them); the rest are told.
        let more = (0..MAX_WAITERS + 3)
            .map(|_| {
                let flight = flight.clone();
                tokio::spawn(async move { flight.get(1, |_| async { Ok(9) }).await })
            })
            .collect::<Vec<_>>();
        tokio::time::sleep(Duration::from_millis(50)).await;
        gate.notify_waiters();
        let mut busy = 0;
        for request in more {
            match request.await.unwrap() {
                Ok(1) => {}
                Err(FlightError::Busy(_)) => busy += 1,
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(busy, 4, "{} wait, the first included", MAX_WAITERS);
        assert_eq!(first.await.unwrap(), Ok(1));
    }

    #[tokio::test]
    async fn a_run_past_its_deadline_fails_and_frees_the_flight() {
        let flight = Flight::<u8, u64>::new(LONG, Duration::from_millis(50));
        let started = Instant::now();
        let slow = flight
            .get(1, |_| async {
                tokio::time::sleep(LONG).await;
                Ok(1)
            })
            .await;
        assert_eq!(slow, Err(FlightError::Timeout(Duration::from_millis(50))));
        assert!(started.elapsed() < Duration::from_secs(5));
        // Nothing kept, nothing left running: the next request runs anew.
        assert_eq!(flight.get(1, |_| async { Ok(2) }).await, Ok(2));
    }

    #[tokio::test]
    async fn blocking_work_past_the_deadline_holds_off_the_next_run() {
        let flight = Flight::<u8, u64>::new(LONG, Duration::from_millis(50));
        let release = Arc::new(std::sync::Barrier::new(2));
        let held = release.clone();
        let slow = flight
            .get(1, move |blocking| {
                blocking.run(move || {
                    held.wait();
                    1
                })
            })
            .await;
        assert_eq!(slow, Err(FlightError::Timeout(Duration::from_millis(50))));
        assert_eq!(
            flight.get(1, |_| async { Ok(2) }).await,
            Err(FlightError::Busy("the last one is still finishing"))
        );
        release.wait();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(flight.get(1, |_| async { Ok(3) }).await, Ok(3));
    }

    #[tokio::test]
    async fn a_failure_is_not_kept_and_answers_expire() {
        let flight = Flight::<u8, u64>::new(Duration::from_millis(50), LONG);
        assert_eq!(
            flight.get(1, |_| async { Err("no".to_owned()) }).await,
            Err(FlightError::Failed("no".to_owned()))
        );
        assert_eq!(flight.get(1, |_| async { Ok(1) }).await, Ok(1));
        assert_eq!(flight.get(1, |_| async { Ok(2) }).await, Ok(1));
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(flight.get(1, |_| async { Ok(3) }).await, Ok(3));
    }
}
