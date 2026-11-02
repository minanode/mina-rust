//! Which WebRTC peers a sync sends its bulk requests to (WEB-NEAREST-RELAY).
//!
//! A web node connects to every relay its page lists, and used to spread its
//! ledger queries, block requests and the staged ledger download across all
//! of them at random. A relay 190 ms away delivers about 0.5 MB/s, one nearby
//! 1 to 2 MB/s, so whatever went to the far one held the sync back: the
//! staged ledger, tens of megabytes in one request, took four times as long
//! when it landed there.
//!
//! Each WebRTC peer now keeps [`P2pSyncPeerStats`]: the time from sending a
//! ledger query or block request to its answer. The quickest answer within
//! [`SYNC_PEER_FASTEST_WINDOW`] stands for the peer's round trip. Once it is
//! that old, the next answer replaces it, quick or not, so until that answer
//! comes an old quick one still counts. A peer
//! whose link delivers little also answers slowly, as each answer waits
//! behind the ones before it. A peer with no useful answer within that
//! window counts as unmeasured again. [`plan_sync_peers`] ranks the peers by
//! it, against every connected WebRTC peer, including those with no room
//! left:
//!
//! - **near** peers, within [`SYNC_PEER_NEAR_FACTOR`] times the quickest
//!   peer's time plus [`SYNC_PEER_NEAR_SLACK`], get requests first, as many
//!   as they take;
//! - **far** peers, and peers with no answer yet while another has one, get
//!   what is left, one unanswered request at a time. That keeps them measured
//!   and ready to take over, at a few percent of the requests;
//! - a peer is **demoted** to far while a request to it has timed out within
//!   [`SYNC_PEER_TIMEOUT_PENALTY`], or its oldest unanswered request is older
//!   than [`SYNC_PEER_STALL_FACTOR`] times its usual answer time (and at least
//!   [`SYNC_PEER_STALL_MIN`]). The next best peer becomes near at once. A
//!   peer that drops is no longer a candidate at all;
//! - a peer whose answer the sync **rejected** (the wrong kind, hashes or
//!   accounts that don't check, an invalid block or staged ledger) loses its
//!   timings and counts as unmeasured for [`SYNC_PEER_REJECTED_PENALTY`]. An
//!   answer is timed when it arrives, before the sync checks it, so without
//!   this a peer answering junk at once would look nearest.
//!
//! If every measured peer is demoted, none is: with one relay, or with all
//! of them slow, the sync sends as it did before. libp2p peers are not
//! ranked: they keep their place in the order and their room, so a native
//! node's choice of OCaml peers is unchanged. The request timeouts are not
//! touched: a request still times out on silence, is retried elsewhere, and
//! the peer is dropped at the ceiling.
//!
//! A sharing browser is a WebRTC peer like a relay and is ranked the same
//! way. It enforces its own serving cap; a refusal is an empty answer, which
//! is not counted as an answer here, so a refusing peer never looks fast.
//! Only an answer of the kind asked for counts ([`sync_answer_matches`]).

use std::time::Duration;

use mina_p2p_messages::v2::{
    MinaLedgerSyncLedgerAnswerStableV2 as Answer, MinaLedgerSyncLedgerQueryStableV1 as Query,
};
use redux::Timestamp;
use serde::{Deserialize, Serialize};

use super::{P2pRpcLocalState, P2pRpcRequest, P2pRpcResponse};
use crate::channels::P2pChannelsState;

/// How long the quickest answer stands for a peer's round trip before a
/// slower one replaces it.
pub const SYNC_PEER_FASTEST_WINDOW: Duration = Duration::from_secs(30);
/// A peer is near if its quickest answer is within this many times the
/// quickest peer's, plus [`SYNC_PEER_NEAR_SLACK`].
pub const SYNC_PEER_NEAR_FACTOR: u32 = 2;
pub const SYNC_PEER_NEAR_SLACK: Duration = Duration::from_millis(50);
/// A peer whose request timed out is demoted for this long.
pub const SYNC_PEER_TIMEOUT_PENALTY: Duration = Duration::from_secs(30);
/// A peer whose answer the sync rejected counts as unmeasured for this long.
pub const SYNC_PEER_REJECTED_PENALTY: Duration = Duration::from_secs(30);
/// A peer is stalled when its oldest unanswered request is older than this
/// many times its smoothed answer time, and than [`SYNC_PEER_STALL_MIN`].
pub const SYNC_PEER_STALL_FACTOR: u32 = 4;
pub const SYNC_PEER_STALL_MIN: Duration = Duration::from_secs(2);

/// How a WebRTC peer has answered our sync requests (ledger queries and
/// blocks) on this connection.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct P2pSyncPeerStats {
    /// The quickest answer, and when it came. A slower answer replaces it
    /// once it is [`SYNC_PEER_FASTEST_WINDOW`] old.
    pub fastest: Option<(Duration, Timestamp)>,
    /// Answer time smoothed over answers, an eighth for each new one, as
    /// TCP smooths its round trip.
    pub smoothed: Option<Duration>,
    pub answers: u64,
    /// When the last answer came.
    pub answered: Option<Timestamp>,
    /// When one of our sync requests to the peer last timed out.
    pub timed_out: Option<Timestamp>,
    /// When the sync last rejected one of the peer's answers.
    #[serde(default)]
    pub rejected: Option<Timestamp>,
}

impl P2pSyncPeerStats {
    /// The answer to a request sent at `sent` came at `now`.
    pub fn record_answer(&mut self, sent: Timestamp, now: Timestamp) {
        let took = now.checked_sub(sent).unwrap_or_default();
        self.fastest = match self.fastest {
            Some((fastest, at))
                if fastest <= took
                    && now.checked_sub(at).unwrap_or_default() < SYNC_PEER_FASTEST_WINDOW =>
            {
                Some((fastest, at))
            }
            _ => Some((took, now)),
        };
        self.smoothed = Some(match self.smoothed {
            Some(smoothed) => (smoothed * 7 + took) / 8,
            None => took,
        });
        self.answers = self.answers.saturating_add(1);
        self.answered = Some(now);
    }

    pub fn record_timeout(&mut self, now: Timestamp) {
        self.timed_out = Some(now);
    }

    /// The sync rejected one of the peer's answers at `now`. Its timings
    /// go: they may have come from junk sent at once.
    pub fn record_rejected(&mut self, now: Timestamp) {
        self.fastest = None;
        self.smoothed = None;
        self.answered = None;
        self.rejected = Some(now);
    }
}

/// Whether `response` is the kind of answer `request` asked for: a block
/// for a block, and for a ledger query the answer to that query. Only such
/// an answer times a peer. An empty answer, an error or another kind comes
/// quickly and serves nothing. The sync checks a matching answer further,
/// down to the block's hash, and drops the timing if it rejects it
/// (`P2pChannelsRpcAction::SyncAnswerRejected`).
pub fn sync_answer_matches(request: &P2pRpcRequest, response: &P2pRpcResponse) -> bool {
    match (request, response) {
        (P2pRpcRequest::LedgerQuery(_, query), P2pRpcResponse::LedgerQuery(answer)) => matches!(
            (query, answer),
            (Query::NumAccounts, Answer::NumAccounts(..))
                | (Query::WhatChildHashes(..), Answer::ChildHashesAre(..))
                | (Query::WhatContents(..), Answer::ContentsAre(..))
        ),
        (P2pRpcRequest::Block(_), P2pRpcResponse::Block(_)) => true,
        _ => false,
    }
}

/// Where a peer stands for the sync's bulk requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2pSyncPeerScore {
    /// A libp2p peer: not ranked, keeps its place and its room.
    Unranked,
    /// A WebRTC peer that hasn't answered a sync request yet, or not within
    /// [`SYNC_PEER_FASTEST_WINDOW`], or whose answer the sync rejected
    /// within [`SYNC_PEER_REJECTED_PENALTY`].
    Unmeasured,
    Measured {
        /// Its quickest answer: about its round trip.
        fastest: Duration,
        /// A request to it timed out lately, or it has stalled.
        demoted: bool,
    },
}

impl P2pChannelsState {
    /// Where this peer stands for the sync's bulk requests at `now`.
    pub fn sync_peer_score(&self, now: Timestamp) -> P2pSyncPeerScore {
        if !self.rpc_pipeline.enabled {
            return P2pSyncPeerScore::Unranked;
        }
        let stats = &self.sync_stats;
        let since = |t: Timestamp| now.checked_sub(t).unwrap_or_default();
        if stats
            .rejected
            .is_some_and(|t| since(t) < SYNC_PEER_REJECTED_PENALTY)
        {
            return P2pSyncPeerScore::Unmeasured;
        }
        let (Some((fastest, _)), Some(answered)) = (stats.fastest, stats.answered) else {
            return P2pSyncPeerScore::Unmeasured;
        };
        if since(answered) >= SYNC_PEER_FASTEST_WINDOW {
            return P2pSyncPeerScore::Unmeasured;
        }
        let timed_out = stats
            .timed_out
            .is_some_and(|t| since(t) < SYNC_PEER_TIMEOUT_PENALTY)
            || self.rpc.timed_out_local_rpc().is_some()
            || self
                .rpc_pipeline
                .requests
                .iter()
                .any(|r| r.timed_out.is_some());
        let stall_after = stats
            .smoothed
            .unwrap_or(fastest)
            .saturating_mul(SYNC_PEER_STALL_FACTOR)
            .max(SYNC_PEER_STALL_MIN);
        let stalled = self
            .oldest_unanswered_rpc_sent()
            .is_some_and(|sent| since(sent) > stall_after);
        P2pSyncPeerScore::Measured {
            fastest,
            demoted: timed_out || stalled,
        }
    }

    /// When the oldest of our unanswered requests to the peer was sent.
    fn oldest_unanswered_rpc_sent(&self) -> Option<Timestamp> {
        let slot = match &self.rpc {
            super::P2pChannelsRpcState::Ready {
                local:
                    P2pRpcLocalState::Requested { time: sent, .. }
                    | P2pRpcLocalState::TimedOut { sent, .. },
                ..
            } => Some(*sent),
            _ => None,
        };
        slot.into_iter()
            .chain(self.rpc_pipeline.requests.iter().map(|r| r.sent))
            .min()
    }

    /// Our unanswered requests to the peer, in the slot and the pipeline.
    pub fn unanswered_rpc_count(&self) -> usize {
        self.unanswered_rpc_ids().count()
    }
}

/// A peer the sync may send to, with `item` whatever the caller tracks it
/// by.
#[derive(Debug, Clone)]
pub struct P2pSyncPeerCandidate<T> {
    pub item: T,
    pub score: P2pSyncPeerScore,
    /// How many more requests it takes now.
    pub room: usize,
    /// How many of ours it hasn't answered.
    pub unanswered: usize,
}

/// The order to send in: every peer of `first` before any of `then`, each
/// with the room it may be given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P2pSyncPeerPlan<T> {
    pub first: Vec<(T, usize)>,
    pub then: Vec<(T, usize)>,
}

impl<T> P2pSyncPeerPlan<T> {
    /// Every peer, in order, with its room.
    pub fn into_ordered(self) -> impl Iterator<Item = (T, usize)> {
        self.first.into_iter().chain(self.then)
    }
}

/// Ranks the candidates for the sync's bulk requests (see the module docs).
/// `others` are the connected peers that can't take a request now, such as
/// a near relay with five out: they count for who is quickest. The input
/// order is kept among peers of the same standing, and libp2p peers keep
/// their exact places in `first`.
pub fn plan_sync_peers<T>(
    candidates: Vec<P2pSyncPeerCandidate<T>>,
    others: &[P2pSyncPeerScore],
) -> P2pSyncPeerPlan<T> {
    let measured = |demoted_ok: bool| {
        candidates
            .iter()
            .map(|c| &c.score)
            .chain(others)
            .filter_map(|score| match *score {
                P2pSyncPeerScore::Measured { fastest, demoted } if demoted_ok || !demoted => {
                    Some(fastest)
                }
                _ => None,
            })
            .min()
    };
    // If every measured peer is demoted, none is.
    let (best, honour_demotion) = match measured(false) {
        Some(best) => (Some(best), true),
        None => (measured(true), false),
    };
    let Some(best) = best else {
        // Nothing measured yet: everyone as before.
        return P2pSyncPeerPlan {
            first: candidates.into_iter().map(|c| (c.item, c.room)).collect(),
            then: Vec::new(),
        };
    };
    let near_limit = best
        .saturating_mul(SYNC_PEER_NEAR_FACTOR)
        .saturating_add(SYNC_PEER_NEAR_SLACK);

    let mut first = Vec::new();
    // Places in `first` held by ranked (WebRTC) peers, and those peers.
    let mut ranked_places = Vec::new();
    let mut ranked_first = Vec::new();
    let mut then = Vec::new();
    for c in candidates {
        match c.score {
            P2pSyncPeerScore::Unranked => first.push(Some((c.item, c.room))),
            P2pSyncPeerScore::Measured { fastest, demoted }
                if fastest <= near_limit && !(honour_demotion && demoted) =>
            {
                ranked_places.push(first.len());
                first.push(None);
                ranked_first.push((fastest, c.item, c.room));
            }
            P2pSyncPeerScore::Measured { fastest, .. } => then.push((
                Some(fastest),
                c.item,
                c.room.min(1usize.saturating_sub(c.unanswered)),
            )),
            P2pSyncPeerScore::Unmeasured => then.push((
                None,
                c.item,
                c.room.min(1usize.saturating_sub(c.unanswered)),
            )),
        }
    }
    // Quickest first; stable, so ties keep the caller's order.
    ranked_first.sort_by_key(|(fastest, ..)| *fastest);
    for (place, (_, item, room)) in ranked_places.into_iter().zip(ranked_first) {
        first[place] = Some((item, room));
    }
    // Measured far peers by their time, then the unmeasured.
    then.sort_by_key(|(fastest, ..)| (fastest.is_none(), *fastest));

    P2pSyncPeerPlan {
        first: first.into_iter().flatten().collect(),
        then: then
            .into_iter()
            .map(|(_, item, room)| (item, room))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::rpc::{
        P2pChannelsRpcState, P2pRpcPipeline, P2pRpcPipelined, P2pRpcRemoteState, P2pRpcRequest,
    };

    fn rank<T>(candidates: Vec<P2pSyncPeerCandidate<T>>) -> P2pSyncPeerPlan<T> {
        plan_sync_peers(candidates, &[])
    }

    fn at_ms(ms: u64) -> Timestamp {
        Timestamp::new(1_000_000_000_000 + ms * 1_000_000)
    }

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    fn measured(
        name: &'static str,
        fastest_ms: u64,
        room: usize,
    ) -> P2pSyncPeerCandidate<&'static str> {
        P2pSyncPeerCandidate {
            item: name,
            score: P2pSyncPeerScore::Measured {
                fastest: ms(fastest_ms),
                demoted: false,
            },
            room,
            unanswered: 5 - room,
        }
    }

    fn demoted(mut c: P2pSyncPeerCandidate<&'static str>) -> P2pSyncPeerCandidate<&'static str> {
        if let P2pSyncPeerScore::Measured { demoted, .. } = &mut c.score {
            *demoted = true;
        }
        c
    }

    fn other(
        name: &'static str,
        score: P2pSyncPeerScore,
        room: usize,
        unanswered: usize,
    ) -> P2pSyncPeerCandidate<&'static str> {
        P2pSyncPeerCandidate {
            item: name,
            score,
            room,
            unanswered,
        }
    }

    /// A relay 15 ms away and one 200 ms away: the near one gets all its
    /// room first, the far one one request at a time after it.
    #[test]
    fn the_near_relay_comes_first_and_the_far_one_gets_one_request() {
        let plan = rank(vec![measured("far", 200, 5), measured("near", 15, 5)]);
        assert_eq!(plan.first, vec![("near", 5)]);
        assert_eq!(plan.then, vec![("far", 1)]);

        // With a request already unanswered, the far one gets no more.
        let plan = rank(vec![measured("far", 200, 4), measured("near", 15, 3)]);
        assert_eq!(plan.first, vec![("near", 3)]);
        assert_eq!(plan.then, vec![("far", 0)]);
    }

    /// The near relay with five out can't take another, but still counts
    /// as quickest: the far one gets one, not five.
    #[test]
    fn a_full_near_relay_still_counts_as_quickest() {
        let near = P2pSyncPeerScore::Measured {
            fastest: ms(15),
            demoted: false,
        };
        let plan = plan_sync_peers(vec![measured("far", 200, 5)], &[near]);
        assert!(plan.first.is_empty());
        assert_eq!(plan.then, vec![("far", 1)]);

        // Once it stalls, the far one takes all it can.
        let stalled = P2pSyncPeerScore::Measured {
            fastest: ms(15),
            demoted: true,
        };
        let plan = plan_sync_peers(vec![measured("far", 200, 5)], &[stalled]);
        assert_eq!(plan.first, vec![("far", 5)]);
    }

    /// Two relays both nearby are both near, quickest first.
    #[test]
    fn relays_within_twice_the_best_plus_slack_are_all_near() {
        let plan = rank(vec![
            measured("b", 60, 5),
            measured("a", 30, 5),
            measured("c", 111, 5),
        ]);
        // 2 * 30 + 50 = 110 ms: c is just over.
        assert_eq!(plan.first, vec![("a", 5), ("b", 5)]);
        assert_eq!(plan.then, vec![("c", 1)]);
    }

    /// The near relay timing out or stalling hands its place to the next
    /// best at once.
    #[test]
    fn a_demoted_near_relay_falls_back_to_the_far_one() {
        let plan = rank(vec![
            demoted(measured("near", 15, 2)),
            measured("far", 200, 5),
        ]);
        assert_eq!(plan.first, vec![("far", 5)]);
        assert_eq!(plan.then, vec![("near", 0)]);
    }

    /// With every relay demoted, or only one relay, nothing is held back.
    #[test]
    fn one_relay_or_all_demoted_sends_as_before() {
        let plan = rank(vec![demoted(measured("only", 200, 5))]);
        assert_eq!(plan.first, vec![("only", 5)]);
        assert!(plan.then.is_empty());

        let plan = rank(vec![
            demoted(measured("far", 200, 5)),
            demoted(measured("near", 15, 5)),
        ]);
        assert_eq!(plan.first, vec![("near", 5)]);
        assert_eq!(plan.then, vec![("far", 1)]);
    }

    /// Before any answer everyone gets their room, as before; once one
    /// peer has answered, an unmeasured one gets a single probe.
    #[test]
    fn unmeasured_peers_get_a_probe_once_another_is_measured() {
        let plan = rank(vec![
            other("x", P2pSyncPeerScore::Unmeasured, 5, 0),
            other("y", P2pSyncPeerScore::Unmeasured, 4, 1),
        ]);
        assert_eq!(plan.first, vec![("x", 5), ("y", 4)]);
        assert!(plan.then.is_empty());

        let plan = rank(vec![
            other("new", P2pSyncPeerScore::Unmeasured, 5, 0),
            measured("far", 300, 5),
            measured("near", 20, 5),
        ]);
        assert_eq!(plan.first, vec![("near", 5)]);
        assert_eq!(plan.then, vec![("far", 1), ("new", 1)]);
    }

    /// libp2p peers keep their places and room; WebRTC peers are ranked
    /// only among the places they held.
    #[test]
    fn libp2p_peers_keep_their_places() {
        let plan = rank(vec![
            other("ocaml1", P2pSyncPeerScore::Unranked, 1, 0),
            measured("b", 40, 5),
            other("ocaml2", P2pSyncPeerScore::Unranked, 1, 0),
            measured("a", 20, 5),
            measured("far", 400, 5),
            other("ocaml3", P2pSyncPeerScore::Unranked, 0, 1),
        ]);
        assert_eq!(
            plan.first,
            vec![
                ("ocaml1", 1),
                ("a", 5),
                ("ocaml2", 1),
                ("b", 5),
                ("ocaml3", 0)
            ]
        );
        assert_eq!(plan.then, vec![("far", 1)]);

        let only_libp2p = vec![
            other("o1", P2pSyncPeerScore::Unranked, 1, 0),
            other("o2", P2pSyncPeerScore::Unranked, 0, 1),
        ];
        let plan = rank(only_libp2p);
        assert_eq!(plan.first, vec![("o1", 1), ("o2", 0)]);
        assert!(plan.then.is_empty());
    }

    #[test]
    fn the_quickest_answer_stands_for_its_window() {
        let mut stats = P2pSyncPeerStats::default();
        stats.record_answer(at_ms(0), at_ms(40));
        assert_eq!(stats.fastest, Some((ms(40), at_ms(40))));
        assert_eq!(stats.smoothed, Some(ms(40)));

        // A slower answer within the window doesn't replace it.
        stats.record_answer(at_ms(100), at_ms(220));
        assert_eq!(stats.fastest, Some((ms(40), at_ms(40))));
        assert_eq!(stats.smoothed, Some(ms(50)));

        // A quicker one does.
        stats.record_answer(at_ms(300), at_ms(320));
        assert_eq!(stats.fastest, Some((ms(20), at_ms(320))));

        // After the window a slower one does too: the path got longer.
        let later = 320 + SYNC_PEER_FASTEST_WINDOW.as_millis() as u64;
        stats.record_answer(at_ms(later), at_ms(later + 190));
        assert_eq!(stats.fastest, Some((ms(190), at_ms(later + 190))));
        assert_eq!(stats.answers, 4);
        assert_eq!(stats.answered, Some(at_ms(later + 190)));
    }

    fn webrtc_channels(stats: P2pSyncPeerStats) -> P2pChannelsState {
        let mut channels = P2pChannelsState::new(&Default::default());
        channels.rpc = P2pChannelsRpcState::Ready {
            time: at_ms(0),
            local: P2pRpcLocalState::WaitingForRequest { time: at_ms(0) },
            remote: P2pRpcRemoteState {
                pending_requests: Default::default(),
                last_responded: at_ms(0),
            },
        };
        channels.rpc_pipeline = P2pRpcPipeline::for_peer(false);
        channels.sync_stats = stats;
        channels
    }

    fn sent_at(channels: &mut P2pChannelsState, id: u64, sent: Timestamp, timed_out: bool) {
        let request = Box::new(P2pRpcRequest::BestTipWithProof);
        if id == 0 {
            if let P2pChannelsRpcState::Ready { local, .. } = &mut channels.rpc {
                *local = P2pRpcLocalState::Requested {
                    time: sent,
                    id,
                    request,
                };
            }
        } else {
            channels.rpc_pipeline.requests.push(P2pRpcPipelined {
                sent,
                id,
                request,
                timed_out: timed_out.then_some(sent),
            });
        }
    }

    /// A peer's standing: unranked if libp2p, unmeasured until it answers
    /// and again 30 s after its last answer, demoted after a timeout or
    /// while stalled.
    #[test]
    fn a_peers_standing() {
        let now = at_ms(10_000);
        let mut stats = P2pSyncPeerStats::default();
        let fresh = |stats: &P2pSyncPeerStats| webrtc_channels(stats.clone());
        assert_eq!(
            fresh(&stats).sync_peer_score(now),
            P2pSyncPeerScore::Unmeasured
        );
        let mut libp2p = fresh(&stats);
        libp2p.rpc_pipeline = P2pRpcPipeline::for_peer(true);
        assert_eq!(libp2p.sync_peer_score(now), P2pSyncPeerScore::Unranked);

        stats.record_answer(at_ms(9_000), at_ms(9_050));
        let measured = |demoted| P2pSyncPeerScore::Measured {
            fastest: ms(50),
            demoted,
        };
        assert_eq!(fresh(&stats).sync_peer_score(now), measured(false));
        assert_eq!(
            fresh(&stats).sync_peer_score(at_ms(9_050 + 30_000)),
            P2pSyncPeerScore::Unmeasured
        );

        // Five out, the oldest sent 1.9 s ago: fine. 2.1 s: stalled.
        let mut busy = fresh(&stats);
        for id in 0..5 {
            sent_at(&mut busy, id, at_ms(8_100 + id * 100), false);
        }
        assert_eq!(busy.sync_peer_score(at_ms(10_000)), measured(false));
        assert_eq!(busy.sync_peer_score(at_ms(10_201)), measured(true));

        // A pipelined request that timed out demotes it while it's out.
        let mut late = fresh(&stats);
        sent_at(&mut late, 0, at_ms(9_900), false);
        sent_at(&mut late, 1, at_ms(9_900), true);
        assert_eq!(late.sync_peer_score(now), measured(true));

        // A timeout demotes it for 30 s.
        stats.record_timeout(at_ms(9_500));
        assert_eq!(fresh(&stats).sync_peer_score(now), measured(true));
        stats.record_answer(at_ms(30_000), at_ms(30_060));
        assert_eq!(fresh(&stats).sync_peer_score(at_ms(39_499)), measured(true));
        assert_eq!(
            fresh(&stats).sync_peer_score(at_ms(39_500)),
            measured(false)
        );
    }

    /// Only the kind of answer asked for counts.
    #[test]
    fn only_the_answer_asked_for_counts() {
        use mina_p2p_messages::v2::LedgerHash;
        let ledger = LedgerHash::zero();
        let query = |q| P2pRpcRequest::LedgerQuery(ledger.clone(), q);
        let num_accounts =
            P2pRpcResponse::LedgerQuery(Answer::NumAccounts(0u64.into(), LedgerHash::zero()));
        let contents =
            P2pRpcResponse::LedgerQuery(Answer::ContentsAre(std::iter::empty().collect()));
        let address = || {
            mina_p2p_messages::v2::MerkleAddressBinableArgStableV1(0u64.into(), Vec::new().into())
        };
        assert!(sync_answer_matches(
            &query(Query::NumAccounts),
            &num_accounts
        ));
        assert!(sync_answer_matches(
            &query(Query::WhatContents(address())),
            &contents
        ));
        assert!(!sync_answer_matches(
            &query(Query::WhatContents(address())),
            &num_accounts
        ));
        assert!(!sync_answer_matches(&query(Query::NumAccounts), &contents));
        assert!(!sync_answer_matches(
            &query(Query::NumAccounts),
            &P2pRpcResponse::LedgerQueryError(LedgerHash::zero(), "no".into())
        ));
        assert!(!sync_answer_matches(
            &P2pRpcRequest::Block(mina_p2p_messages::v2::StateHash::zero()),
            &contents
        ));
        assert!(!sync_answer_matches(
            &query(Query::NumAccounts),
            &P2pRpcResponse::InitialPeers(std::iter::empty().collect())
        ));
    }

    /// A rejected answer drops the peer's timing: it is unmeasured for 30 s
    /// whatever it answers, then has to answer again to be measured.
    #[test]
    fn a_rejected_answer_drops_the_timing() {
        let mut stats = P2pSyncPeerStats::default();
        stats.record_answer(at_ms(9_000), at_ms(9_010));
        stats.record_rejected(at_ms(9_010));
        assert_eq!(
            (stats.fastest, stats.smoothed, stats.answered),
            (None, None, None)
        );
        let score =
            |stats: &P2pSyncPeerStats, now| webrtc_channels(stats.clone()).sync_peer_score(now);
        assert_eq!(score(&stats, at_ms(10_000)), P2pSyncPeerScore::Unmeasured);

        // Quick answers within the 30 s don't count.
        stats.record_answer(at_ms(20_000), at_ms(20_005));
        assert_eq!(score(&stats, at_ms(39_009)), P2pSyncPeerScore::Unmeasured);
        // After them, the answers since count.
        assert_eq!(
            score(&stats, at_ms(39_010)),
            P2pSyncPeerScore::Measured {
                fastest: ms(5),
                demoted: false,
            }
        );
    }
}
