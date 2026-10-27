//! Taking RPC requests between events, see [`RpcEventInterleave`].

use std::time::Duration;

/// Chooses between a pending RPC request and the next queued event, for
/// `ProcessEvents`.
///
/// The run loop takes RPC requests in its `select!`, which it reaches only
/// once the event queue is empty. In the web node's snarked ledger sync over
/// loopback the queue stays non-empty for up to a minute at a time, so page
/// requests waited up to 221 s (R3). `ProcessEvents` therefore also takes
/// requests here, between two events, with two limits so that many clients,
/// such as a native node's HTTP clients, can't starve p2p and ledger events
/// instead:
///
/// - at most one request between two events;
/// - while events are queued, requests get no more of the node thread than
///   events do. The time from taking a request to the next pick is a debt
///   that time spent on events pays off, and no request is taken while it is
///   unpaid. Alternation alone bounds the share of picks, not of time: in R3's
///   run 7, requests that took 250 ms of the node thread each left events 2.6
///   ledger answers a second, against about 20 without them, and the peers
///   timed out.
///
/// When no event is queued, nothing waits, and the debt is dropped. After a
/// request, with no event queued, [`Self::next`] returns `None`, which ends
/// the `ProcessEvents` batch, so that `CheckTimeoutsAction` still runs
/// between requests, as it did when the run loop's `select!` took them.
#[derive(Debug, Default)]
pub struct RpcEventInterleave {
    /// Whether the last pick was a request, and when it was made. `None`
    /// after a pick that found no event, which ends a batch.
    last_pick: Option<(bool, redux::Instant)>,
    /// Node-thread time taken by requests and not yet matched by events.
    rpc_debt: Duration,
}

impl RpcEventInterleave {
    /// The next item for `ProcessEvents`: a request from `next_rpc`, an event
    /// from `next_event`, or `None` to end the batch. `now` is the time of
    /// this pick: the time since the previous one is charged to what that one
    /// picked.
    pub fn next<T>(
        &mut self,
        now: redux::Instant,
        next_rpc: impl FnOnce() -> Option<T>,
        next_event: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        let rpc_taken_last = match self.last_pick.take() {
            Some((true, at)) => {
                self.rpc_debt += now.saturating_duration_since(at);
                true
            }
            Some((false, at)) => {
                let paid = now.saturating_duration_since(at);
                self.rpc_debt = self.rpc_debt.saturating_sub(paid);
                false
            }
            None => false,
        };
        if !rpc_taken_last && self.rpc_debt.is_zero() {
            if let Some(rpc) = next_rpc() {
                self.last_pick = Some((true, now));
                return Some(rpc);
            }
        }
        match next_event() {
            Some(event) => {
                self.last_pick = Some((false, now));
                Some(event)
            }
            None => {
                self.rpc_debt = Duration::ZERO;
                None
            }
        }
    }

    /// The run loop found the event queue empty after `ProcessEvents`, and
    /// goes back to waiting. Nothing waits, so the debt is dropped, and the
    /// wait until the next pick isn't charged to the last one: a batch that
    /// ended on its time budget right after a request would otherwise charge
    /// the idle time to that request, and hold back the next requests until
    /// events had paid it off.
    pub fn events_drained(&mut self) {
        self.last_pick = None;
        self.rpc_debt = Duration::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, time::Duration};

    use mina_node::event_source::{PROCESS_EVENTS_MAX, PROCESS_EVENTS_TIME_BUDGET};

    use super::RpcEventInterleave;

    #[derive(Debug, PartialEq, Clone, Copy)]
    enum Picked {
        Rpc(u32),
        Event(u32),
    }

    const MS: Duration = Duration::from_millis(1);

    /// A simulated node thread: queues of requests and events, what each
    /// costs, and a clock that advances by the cost of what was handled.
    struct Sim {
        interleave: RpcEventInterleave,
        now: redux::Instant,
        rpcs: VecDeque<u32>,
        events: VecDeque<u32>,
        rpc_cost: Duration,
        event_cost: Duration,
        /// Node-thread time spent on requests and on events.
        rpc_time: Duration,
        event_time: Duration,
    }

    impl Sim {
        fn new(rpc_cost: Duration, event_cost: Duration) -> Self {
            Self {
                interleave: Default::default(),
                now: redux::Instant::now(),
                rpcs: Default::default(),
                events: Default::default(),
                rpc_cost,
                event_cost,
                rpc_time: Duration::ZERO,
                event_time: Duration::ZERO,
            }
        }

        /// Picks like one `ProcessEvents` dispatch: until a pick finds
        /// nothing, `PROCESS_EVENTS_MAX` picks, or the batch has run for
        /// `PROCESS_EVENTS_TIME_BUDGET`. `refill` runs before each pick, to
        /// model senders that keep a queue from draining.
        fn process_events(
            &mut self,
            mut refill: impl FnMut(usize, &mut VecDeque<u32>, &mut VecDeque<u32>),
        ) -> Vec<Picked> {
            let started = self.now;
            let mut picked = vec![];
            for i in 0..PROCESS_EVENTS_MAX {
                refill(i, &mut self.rpcs, &mut self.events);
                let (rpcs, events) = (&mut self.rpcs, &mut self.events);
                let next = self.interleave.next(
                    self.now,
                    || rpcs.pop_front().map(Picked::Rpc),
                    || events.pop_front().map(Picked::Event),
                );
                match next {
                    Some(item @ Picked::Rpc(_)) => {
                        self.now += self.rpc_cost;
                        self.rpc_time += self.rpc_cost;
                        picked.push(item);
                    }
                    Some(item @ Picked::Event(_)) => {
                        self.now += self.event_cost;
                        self.event_time += self.event_cost;
                        picked.push(item);
                    }
                    None => break,
                }
                if self.now.saturating_duration_since(started) >= PROCESS_EVENTS_TIME_BUDGET {
                    break;
                }
            }
            picked
        }

        /// The run loop after a batch that left no event queued: it tells
        /// the interleave, as `Node::run_forever` does, then waits `idle` for
        /// the next event or request.
        fn run_loop_waits(&mut self, idle: Duration) {
            assert!(self.events.is_empty());
            self.interleave.events_drained();
            self.now += idle;
        }
    }

    /// Keeps at least two events queued, as in snarked ledger sync from a
    /// fast peer.
    fn endless_events(next_id: &mut u32, events: &mut VecDeque<u32>) {
        while events.len() < 2 {
            events.push_back(*next_id);
            *next_id += 1;
        }
    }

    /// Keeps the RPC channel full, like many HTTP clients.
    fn endless_rpcs(next_id: &mut u32, rpcs: &mut VecDeque<u32>) {
        while rpcs.len() < 8 {
            rpcs.push_back(*next_id);
            *next_id += 1;
        }
    }

    fn count_rpcs(picked: &[Picked]) -> usize {
        picked
            .iter()
            .filter(|p| matches!(p, Picked::Rpc(_)))
            .count()
    }

    #[test]
    fn rpc_is_taken_while_events_never_drain() {
        let mut sim = Sim::new(Duration::ZERO, Duration::ZERO);
        let mut next_event = 0;
        let picked = sim.process_events(|i, rpcs, events| {
            endless_events(&mut next_event, events);
            if i == 100 {
                rpcs.push_back(7);
            }
        });
        // The queue never drained, so the whole batch ran, and the request
        // was taken at the first event boundary after it arrived.
        assert_eq!(picked.len(), PROCESS_EVENTS_MAX);
        assert_eq!(picked[100], Picked::Rpc(7));
        assert_eq!(count_rpcs(&picked), 1);
    }

    #[test]
    fn requests_arriving_together_wait_one_event_each() {
        let mut sim = Sim::new(MS, MS);
        sim.rpcs.extend([1, 2, 3]);
        let mut next_event = 0;
        let picked = sim.process_events(|_, _, events| endless_events(&mut next_event, events));
        assert_eq!(
            picked[..6],
            [
                Picked::Rpc(1),
                Picked::Event(0),
                Picked::Rpc(2),
                Picked::Event(1),
                Picked::Rpc(3),
                Picked::Event(2),
            ]
        );
    }

    #[test]
    fn events_are_taken_while_requests_never_drain() {
        let mut sim = Sim::new(MS, MS);
        sim.events.extend(0..10);
        let mut next_rpc = 0;
        let picked = sim.process_events(|_, rpcs, _| endless_rpcs(&mut next_rpc, rpcs));
        // Requests and events alternate until the events are gone, then the
        // batch ends after one more request, so that timeouts get checked.
        let expected = (0..10)
            .flat_map(|i| [Picked::Rpc(i), Picked::Event(i)])
            .chain([Picked::Rpc(10)])
            .collect::<Vec<_>>();
        assert_eq!(picked, expected);
    }

    #[test]
    fn without_events_a_batch_takes_one_request() {
        let mut sim = Sim::new(MS, MS);
        sim.rpcs.extend([1, 2]);
        for id in [1, 2] {
            assert_eq!(sim.process_events(|_, _, _| {}), [Picked::Rpc(id)]);
        }
        assert!(sim.process_events(|_, _, _| {}).is_empty());
    }

    #[test]
    fn expensive_requests_get_at_most_half_the_thread() {
        // As in R3's run 7: each request takes far longer than an event, and
        // neither queue drains.
        let mut sim = Sim::new(25 * MS, MS);
        let (mut next_rpc, mut next_event) = (0, 0);
        let mut batches = 0;
        while sim.rpc_time + sim.event_time < Duration::from_secs(10) {
            sim.process_events(|_, rpcs, events| {
                endless_rpcs(&mut next_rpc, rpcs);
                endless_events(&mut next_event, events);
            });
            batches += 1;
        }
        // Events pay for each request before the next is taken: 25 events
        // between two requests.
        assert!(sim.rpc_time <= sim.event_time + 25 * MS);
        assert!(sim.rpc_time >= sim.event_time - 25 * MS);
        // And every batch ends within its time budget and one more item, so
        // that `CheckTimeoutsAction` runs about ten times a second.
        assert!(batches >= 10 * 9, "{batches} batches in 10 s");
    }

    #[test]
    fn cheap_requests_alternate_with_events() {
        // The page's `status()` costs less than a ledger answer: requests and
        // events alternate, as without the debt.
        let mut sim = Sim::new(MS, 5 * MS);
        let (mut next_rpc, mut next_event) = (0, 0);
        let picked = sim.process_events(|_, rpcs, events| {
            endless_rpcs(&mut next_rpc, rpcs);
            endless_events(&mut next_event, events);
        });
        assert_eq!(
            picked[..4],
            [
                Picked::Rpc(0),
                Picked::Event(0),
                Picked::Rpc(1),
                Picked::Event(1)
            ]
        );
    }

    #[test]
    fn debt_is_dropped_when_no_event_waits() {
        let mut sim = Sim::new(50 * MS, MS);
        sim.rpcs.extend([1, 2]);
        sim.events.push_back(0);
        // A request that costs 50 events' time, then the one queued event,
        // which doesn't pay it off, then nothing: the batch ends.
        assert_eq!(
            sim.process_events(|_, _, _| {}),
            [Picked::Rpc(1), Picked::Event(0)]
        );
        // Nothing waited, so the next request is taken at once, even with an
        // event queued behind it.
        sim.events.push_back(1);
        assert_eq!(
            sim.process_events(|_, _, _| {}),
            [Picked::Rpc(2), Picked::Event(1)]
        );
    }

    #[test]
    fn idle_time_is_not_charged_to_a_request() {
        // A request that alone outlasts the batch's time budget, with no
        // event queued: the batch ends on the budget, right after it.
        let mut sim = Sim::new(150 * MS, MS);
        sim.rpcs.push_back(1);
        assert_eq!(sim.process_events(|_, _, _| {}), [Picked::Rpc(1)]);
        // The run loop finds no event and waits 10 s.
        sim.run_loop_waits(Duration::from_secs(10));
        // Then events stop draining and a request arrives: it is taken at
        // once, not after 10 s of events.
        sim.rpcs.push_back(2);
        let mut next_event = 0;
        let picked = sim.process_events(|_, _, events| endless_events(&mut next_event, events));
        assert_eq!(picked[0], Picked::Rpc(2));
    }
}
