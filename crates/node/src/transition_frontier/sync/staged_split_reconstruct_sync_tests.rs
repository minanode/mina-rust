//! STAGED-SPLIT and a failed rebuild: parts put together from two relays'
//! streams can make the staged ledger hash and still fail to rebuild the
//! staged ledger. Either relay may have sent the part that fails, and the
//! relay whose stream completed the parts need not be that one, so neither
//! is rejected. Both count as invalid, and the fetch starts over from one
//! relay at a time: a relay whose own parts then fail is rejected, as when
//! one relay sends them all, and the other relay's parts rebuild it.
use super::{
    nearest_peer_sync_tests::{at_ms, near, run},
    reconstruct_reject_sync_tests::{rebuilding, staged, Fixture, FEE},
    staged_split_sync_tests::{advance_root, local, local_rpc, other, requests, staged_node},
    *,
};
use crate::{
    p2p::{
        channels::{
            rpc::StagedLedgerAuxAndPendingCoinbases,
            streaming_rpc::{
                staged_ledger_parts::{
                    from_first_part, from_last_part, parts_count, StagedLedgerPartsResponse,
                },
                P2pChannelsStreamingRpcAction, P2pStreamingRpcId, P2pStreamingRpcLocalState,
                P2pStreamingRpcResponse,
            },
        },
        PeerId,
    },
    transition_frontier::sync::ledger::staged::{
        PeerStagedLedgerPartsFetchState, TransitionFrontierSyncLedgerStagedAction as StagedAction,
    },
};
use mina_p2p_messages::v2;
use redux::EnablingCondition;

/// Two scan state trees, so that each relay's stream can bring one.
const TREES: usize = 2;

/// The relays' parts: `honest` rebuilds the staged ledger, `tampered`
/// changes the done transfer's fee in the first tree and makes the same
/// staged ledger hash.
struct Parts {
    fixture: Fixture,
    honest: Arc<StagedLedgerAuxAndPendingCoinbases>,
    tampered: Arc<StagedLedgerAuxAndPendingCoinbases>,
    hash: v2::MinaBaseStagedLedgerHashStableV1,
}

impl Parts {
    fn new() -> Self {
        let fixture = Fixture::new();
        let (honest, hash) = fixture.parts_in_trees(FEE, TREES);
        let (tampered, tampered_hash) = fixture.parts_in_trees(FEE + 1, TREES);
        assert_eq!(tampered_hash, hash);
        Self {
            fixture,
            honest,
            tampered,
            hash,
        }
    }

    /// `peer`'s parts: `bad` sends the tampered ones.
    fn of(&self, peer: PeerId, bad: PeerId) -> &StagedLedgerAuxAndPendingCoinbases {
        if peer == bad {
            &*self.tampered
        } else {
            &*self.honest
        }
    }

    /// A web node fetching these parts from its two relays at once: the
    /// nearer from the first part, the other from the last.
    fn syncing(&self) -> crate::State {
        let mut state = staged_node(2, true);
        let TransitionFrontierSyncState::RootLedgerPending(pending) =
            &mut state.transition_frontier.sync
        else {
            unreachable!()
        };
        let TransitionFrontierSyncLedgerState::Staged(
            TransitionFrontierSyncLedgerStagedState::PartsFetchPending { target, .. },
        ) = &mut pending.ledger
        else {
            unreachable!()
        };
        target.snarked_ledger_hash = self.fixture.snarked_ledger_hash();
        target.staged.hashes = self.hash.clone();
        state
    }
}

fn part(
    state: &mut crate::State,
    peer: PeerId,
    id: P2pStreamingRpcId,
    response: Option<StagedLedgerPartsResponse>,
    now: Timestamp,
) -> Vec<String> {
    run(
        state,
        P2pChannelsStreamingRpcAction::ResponsePartReceived {
            peer_id: peer,
            id,
            response: P2pStreamingRpcResponse::StagedLedgerParts(response.unwrap()),
        },
        now,
    )
}

fn rejected(ran: &[String]) -> Vec<&String> {
    ran.iter()
        .filter(|a| a.contains("SyncAnswerRejected"))
        .collect()
}

fn ever_rejected(state: &crate::State, peer: &PeerId) -> bool {
    state
        .p2p
        .get_ready_peer(peer)
        .unwrap()
        .channels
        .sync_stats
        .rejected
        .is_some()
}

/// Where the union came from: `bad` sent the tampered tree, and `sender`
/// sent the part that completed the parts, the other relay's stream being
/// cancelled with a part on its way.
struct Union {
    bad: PeerId,
    sender: PeerId,
}

/// When the two streams meet, and when the retry after a failed rebuild
/// goes: the retry delay of invalid parts later.
fn union_at() -> Timestamp {
    at_ms(2_000)
}

fn retry_at() -> Timestamp {
    at_ms(7_000)
}

/// The two streams meet, and the parts are taken for the staged ledger
/// rebuild.
///
/// - The nearer relay honest: the other sends the scan state base and both
///   trees, last first, then the nearer the base, the scan state base and
///   the zkapp updates, which completes the parts.
/// - The nearer relay bad: it sends the bases, the zkapp updates and the
///   first tree, then the other the scan state base and the last tree,
///   which completes the parts.
fn put_together(parts: &Parts, bad: PeerId) -> (crate::State, Union) {
    let mut state = parts.syncing();
    run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let first = local_rpc(&state, &near()).unwrap();
    let last = local_rpc(&state, &other()).unwrap();
    let from_first = |sent| from_first_part(parts.of(near(), bad), sent);
    let from_last = |sent| from_last_part(parts.of(other(), bad), sent);
    let (sender, ran, cancelled) = if bad == other() {
        for sent in 0..3 {
            part(&mut state, other(), last, from_last(sent), union_at());
        }
        for sent in 0..2 {
            part(&mut state, near(), first, from_first(sent), union_at());
        }
        let ran = part(&mut state, near(), first, from_first(2), union_at());
        (near(), ran, (other(), last, from_last(3)))
    } else {
        for sent in 0..4 {
            part(&mut state, near(), first, from_first(sent), union_at());
        }
        part(&mut state, other(), last, from_last(0), union_at());
        let ran = part(&mut state, other(), last, from_last(1), union_at());
        (other(), ran, (near(), first, from_first(4)))
    };
    assert!(
        ran.iter()
            .any(|a| a.contains("PartsFetchSuccess") && a.contains(&format!("{sender:?}"))),
        "{ran:#?}"
    );
    // The part on its way to the cancelled stream comes, and is dropped:
    // that relay takes a new request.
    let (peer, id, on_its_way) = cancelled;
    assert!(matches!(
        local(&state, &peer),
        P2pStreamingRpcLocalState::Cancelled { .. }
    ));
    part(&mut state, peer, id, on_its_way, union_at());
    assert!(matches!(
        local(&state, &peer),
        P2pStreamingRpcLocalState::Responded { .. }
    ));
    run(&mut state, StagedAction::ReconstructPending, union_at());
    (state, Union { bad, sender })
}

/// The rebuild of the parts pending, as the ledger thread runs it, and its
/// outcome given to the sync.
fn rebuild(state: &mut crate::State, parts: &Parts, now: Timestamp) -> (bool, Vec<String>) {
    match parts.fixture.rebuild(rebuilding(state)) {
        Ok(()) => {
            let ledger_hash = parts.honest.staged_ledger_hash.clone();
            // `Success` follows.
            run(state, StagedAction::ReconstructSuccess { ledger_hash }, now);
            assert!(staged(state).is_success());
            (true, vec![])
        }
        Err(error) => {
            assert!(error.contains("Mismatching merkle root"), "{error}");
            (
                false,
                run(state, StagedAction::ReconstructError { error }, now),
            )
        }
    }
}

/// Each relay sending the tree that fails, the honest one completing the
/// parts: neither relay is rejected, both count as invalid, and the next
/// fetch is from one relay at a time.
#[test]
fn parts_put_together_that_fail_to_rebuild_reject_neither_relay() {
    let parts = Parts::new();
    for bad in [other(), near()] {
        let (mut state, union) = put_together(&parts, bad);
        assert_ne!(union.sender, union.bad);
        let (rebuilt, ran) = rebuild(&mut state, &parts, union_at());
        assert!(!rebuilt, "bad {bad:?}");
        assert!(rejected(&ran).is_empty(), "bad {bad:?}: {ran:#?}");
        assert!(requests(&ran).is_empty(), "bad {bad:?}: {ran:#?}");
        for peer in [near(), other()] {
            assert!(!ever_rejected(&state, &peer), "{peer:?}, bad {bad:?}");
        }
        let TransitionFrontierSyncLedgerStagedState::PartsFetchPending {
            attempts,
            single_stream,
            ..
        } = staged(&state)
        else {
            panic!("{:?}", staged(&state));
        };
        assert!(*single_stream);
        assert_eq!(attempts.len(), 2, "{attempts:?}");
        for peer in [near(), other()] {
            assert!(
                matches!(
                    attempts.get(&peer),
                    Some(PeerStagedLedgerPartsFetchState::Invalid { .. })
                ),
                "{peer:?}: {attempts:?}"
            );
        }
    }
}

/// Streams the whole staged ledger from the relay asked in `ran`, which must
/// be one request from the first part, and returns that relay.
fn stream_alone(state: &mut crate::State, parts: &Parts, bad: PeerId, ran: &[String]) -> PeerId {
    let asked = requests(ran);
    assert_eq!(asked.len(), 1, "{ran:#?}");
    assert!(!asked[0].contains("FromLast"), "{asked:?}");
    let peer = [near(), other()]
        .into_iter()
        .find(|peer| asked[0].contains(&format!("{peer:?}")))
        .unwrap();
    let id = local_rpc(state, &peer).unwrap();
    let data = parts.of(peer, bad);
    for sent in 0..parts_count(data) {
        if sent == 1 {
            // One stream: no second one while it goes.
            assert!(!StagedAction::PartsPeerFetchInit.is_enabled(state, retry_at()));
        }
        part(state, peer, id, from_first_part(data, sent), retry_at());
    }
    run(state, StagedAction::ReconstructPending, retry_at());
    peer
}

/// After the union fails to rebuild, one relay at a time. With the nearer
/// relay honest, its parts rebuild the staged ledger. With it bad, its parts
/// fail again and it alone is rejected, and the other's rebuild it. Neither
/// loop: each relay is asked once.
#[test]
fn after_a_failed_union_one_relay_at_a_time_rebuilds_the_staged_ledger() {
    let parts = Parts::new();
    for bad in [other(), near()] {
        let (mut state, _) = put_together(&parts, bad);
        rebuild(&mut state, &parts, union_at());

        // Both relays wait out the retry delay of invalid parts.
        let ran = run(&mut state, StagedAction::PartsPeerFetchInit, retry_at());
        let mut asked = vec![stream_alone(&mut state, &parts, bad, &ran)];
        assert_eq!(asked[0], near(), "the nearer first");
        let (rebuilt, ran) = rebuild(&mut state, &parts, retry_at());
        if bad == near() {
            assert!(!rebuilt);
            let rejected = rejected(&ran);
            assert_eq!(rejected.len(), 1, "{ran:#?}");
            assert!(rejected[0].contains(&format!("{:?}", near())), "{ran:#?}");
            asked.push(stream_alone(&mut state, &parts, bad, &ran));
            let (rebuilt, ran) = rebuild(&mut state, &parts, retry_at());
            assert!(rebuilt, "{ran:#?}");
            assert!(ever_rejected(&state, &near()));
            assert_eq!(asked, [near(), other()]);
        } else {
            assert!(rebuilt, "{ran:#?}");
            assert!(!ever_rejected(&state, &near()));
            assert_eq!(asked, [near()]);
        }
        assert!(!ever_rejected(&state, &other()), "bad {bad:?}");
    }
}

/// Parts put together that fail to rebuild after the root moved: the fetch
/// retargets to the new root, and the new fetch still waits the retry
/// delay for both relays and then asks one, from the first. The bad relay,
/// asked first as the nearer, is then rejected alone, and the other asked.
#[test]
fn a_failed_union_rebuild_after_the_root_moved_retries_one_stream_after_the_delay() {
    let parts = Parts::new();
    let bad = near();
    let (mut state, _) = put_together(&parts, bad);
    advance_root(&mut state);
    let (rebuilt, ran) = rebuild(&mut state, &parts, union_at());
    assert!(!rebuilt);
    assert!(
        ran.iter().any(|a| a.contains("LedgerRootRetarget")),
        "{ran:#?}"
    );
    assert!(requests(&ran).is_empty(), "asked at once: {ran:#?}");
    assert!(rejected(&ran).is_empty(), "{ran:#?}");
    assert!(
        matches!(
            staged(&state),
            TransitionFrontierSyncLedgerStagedState::PartsFetchPending {
                single_stream: true,
                ..
            }
        ),
        "single-stream fallback lost on root retarget: {:?}",
        staged(&state)
    );

    // One stream, from the first, to the nearer relay. Its parts are not
    // the new root's, so they are refused, from it alone: it is rejected,
    // and the other relay is asked, alone too.
    let ran = run(&mut state, StagedAction::PartsPeerFetchInit, retry_at());
    let asked = requests(&ran);
    assert_eq!(asked.len(), 1, "{ran:#?}");
    assert!(!asked[0].contains("FromLast"), "{asked:?}");
    assert!(asked[0].contains(&format!("{bad:?}")), "{asked:?}");
    let id = local_rpc(&state, &bad).unwrap();
    let data = parts.of(bad, bad);
    let mut ran = vec![];
    for sent in 0..parts_count(data) {
        ran = part(&mut state, bad, id, from_first_part(data, sent), retry_at());
    }
    let rejected = rejected(&ran);
    assert_eq!(rejected.len(), 1, "{ran:#?}");
    assert!(rejected[0].contains(&format!("{bad:?}")), "{ran:#?}");
    let asked = requests(&ran);
    assert_eq!(asked.len(), 1, "{ran:#?}");
    assert!(!asked[0].contains("FromLast"), "{asked:?}");
    assert!(asked[0].contains(&format!("{:?}", other())), "{asked:?}");
    assert!(!ever_rejected(&state, &other()));
}

/// The frontier keeps both relays of a failed union, across targets, until
/// a staged ledger is rebuilt.
#[test]
fn refusals_are_kept_until_a_staged_ledger_is_rebuilt() {
    let parts = Parts::new();
    let bad = other();
    let (mut state, _) = put_together(&parts, bad);
    rebuild(&mut state, &parts, union_at());
    let refusals = &state.transition_frontier.staged_parts_refusals;
    assert_eq!(
        refusals.keys().copied().collect::<Vec<_>>(),
        {
            let mut both = vec![near(), other()];
            both.sort();
            both
        },
        "{refusals:?}"
    );
    assert!(refusals.values().all(|at| *at == union_at()));

    let ran = run(&mut state, StagedAction::PartsPeerFetchInit, retry_at());
    stream_alone(&mut state, &parts, bad, &ran);
    let (rebuilt, _) = rebuild(&mut state, &parts, retry_at());
    assert!(rebuilt);
    assert!(state.transition_frontier.staged_parts_refusals.is_empty());
}
