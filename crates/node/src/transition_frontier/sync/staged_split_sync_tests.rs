//! STAGED-SPLIT: a web node with two relays that serve a staged ledger from
//! its last part fetches it from both ends at once, puts the two halves
//! together where they meet, and cancels the other stream. Without the
//! advertisement, or with one stream set, it fetches from one relay as
//! before.
use super::{
    nearest_peer_sync_tests::{add_peer, at_ms, initialized_node, near, run},
    *,
};
use crate::{
    p2p::{
        callbacks::P2pCallbacksAction,
        channels::{
            rpc::{P2pSyncPeerStats, StagedLedgerAuxAndPendingCoinbases},
            streaming_rpc::{
                staged_ledger_parts::{
                    from_first_part, from_last_part, parts_count, StagedLedgerPartsResponse,
                    StagedLedgerPartsSendProgress,
                },
                P2pChannelsStreamingRpcAction, P2pChannelsStreamingRpcState, P2pStreamingRpcId,
                P2pStreamingRpcLocalState, P2pStreamingRpcRemoteState, P2pStreamingRpcRequest,
                P2pStreamingRpcResponse, StreamingRpcChannelMsg,
            },
            P2pChannelsMessageReceivedAction,
        },
        PeerId,
    },
    transition_frontier::sync::ledger::staged::{
        PeerStagedLedgerPartsFetchState, TransitionFrontierSyncLedgerStagedAction as StagedAction,
        TransitionFrontierSyncLedgerStagedState,
    },
};
use mina_p2p_messages::{list::List, v2};
use redux::EnablingCondition;

pub(super) fn other() -> PeerId {
    PeerId::from_bytes([14; 32])
}

fn stats(fastest_ms: u64) -> P2pSyncPeerStats {
    let mut stats = P2pSyncPeerStats::default();
    stats.record_answer(at_ms(0), at_ms(fastest_ms));
    stats
}

/// A node at the staged ledger with two WebRTC relays, `advertised` or not,
/// fetching with `streams` at once.
pub(super) fn staged_node(streams: usize, advertised: bool) -> crate::State {
    let (mut state, root) = initialized_node();
    add_peer(&mut state, near(), false, &root, Some(stats(150)));
    add_peer(&mut state, other(), false, &root, Some(stats(170)));
    let p2p = state.p2p.ready_mut().unwrap();
    for peer in [near(), other()] {
        p2p.get_ready_peer_mut(&peer)
            .unwrap()
            .channels
            .staged_ledger_from_last = advertised;
    }
    p2p.config.limits = p2p
        .config
        .limits
        .clone()
        .with_staged_ledger_streams(crate::p2p::Limit::Some(streams));
    let target = SyncLedgerTarget::root(&root).with_staged().unwrap();
    state.transition_frontier.sync =
        TransitionFrontierSyncState::RootLedgerPending(TransitionFrontierSyncLedgerPending {
            time: Timestamp::ZERO,
            best_tip: root.clone(),
            root_block: root.clone(),
            blocks_inbetween: vec![],
            root_block_updates: vec![],
            ledger: TransitionFrontierSyncLedgerStagedState::PartsFetchPending {
                time: Timestamp::ZERO,
                target,
                attempts: Default::default(),
                single_stream: false,
            }
            .into(),
        });
    state
}

/// `Cancel(id)` went to `peer`.
fn assert_cancelled(ran: &[String], peer: PeerId, id: P2pStreamingRpcId) {
    assert!(
        ran.iter().any(|a| a.contains("MessageSend")
            && a.contains(&format!("Cancel({id})"))
            && a.contains(&format!("{peer:?}"))),
        "no Cancel({id}) to {peer:?}: {ran:#?}"
    );
}

pub(super) fn requests(ran: &[String]) -> Vec<&String> {
    ran.iter()
        .filter(|a| a.contains("RequestSend") && a.contains("StagedLedgerParts"))
        .collect()
}

pub(super) fn local_rpc(state: &crate::State, peer: &PeerId) -> Option<P2pStreamingRpcId> {
    state
        .p2p
        .get_ready_peer(peer)?
        .channels
        .streaming_rpc
        .pending_local_rpc_id()
}

pub(super) fn local(state: &crate::State, peer: &PeerId) -> P2pStreamingRpcLocalState {
    match &state
        .p2p
        .get_ready_peer(peer)
        .unwrap()
        .channels
        .streaming_rpc
    {
        P2pChannelsStreamingRpcState::Ready { local, .. } => local.clone(),
        other => panic!("{other:?}"),
    }
}

/// A staged ledger of `trees` scan state trees.
fn staged_ledger(trees: u64) -> StagedLedgerAuxAndPendingCoinbases {
    let tree = |i: u64| {
        v2::TransactionSnarkScanStateStableV2ScanStateTreesA::Leaf(vec![(
            v2::ParallelScanWeightStableV1 {
                base: i.into(),
                merge: 0.into(),
            },
            v2::TransactionSnarkScanStateStableV2ScanStateTreesABaseT1::Empty,
        )])
    };
    StagedLedgerAuxAndPendingCoinbases {
        scan_state: v2::TransactionSnarkScanStateStableV2 {
            scan_state: v2::TransactionSnarkScanStateStableV2ScanState {
                trees: (tree(0), (1..trees).map(tree).collect()),
                acc: None,
                curr_job_seq_no: 0.into(),
                max_base_jobs: 1.into(),
                delay: 2.into(),
            },
            previous_incomplete_zkapp_updates: (
                List::new(),
                v2::TransactionSnarkScanStateStableV2PreviousIncompleteZkappUpdates1::Border_block_continued_in_the_next_tree(false),
            ),
        },
        staged_ledger_hash: v2::LedgerHash::zero(),
        pending_coinbase: v2::MinaBasePendingCoinbaseStableV2 {
            tree: v2::MinaBasePendingCoinbaseMerkleTreeVersionedStableV2 {
                indexes: List::new(),
                depth: 5.into(),
                tree: v2::MinaBasePendingCoinbaseMerkleTreeVersionedStableV2Tree::Hash(
                    v2::MinaBasePendingCoinbaseHashVersionedStableV1(
                        v2::MinaBasePendingCoinbaseHashBuilderStableV1(
                            mina_p2p_messages::bigint::BigInt::zero(),
                        ),
                    )
                    .into(),
                ),
            },
            pos_list: List::new(),
            new_pos: v2::MinaBasePendingCoinbaseStackIdStableV1(0.into()),
        },
        needed_blocks: List::new(),
    }
}

fn part(
    state: &mut crate::State,
    peer: PeerId,
    id: P2pStreamingRpcId,
    response: Option<StagedLedgerPartsResponse>,
) -> Vec<String> {
    run(
        state,
        P2pChannelsStreamingRpcAction::ResponsePartReceived {
            peer_id: peer,
            id,
            response: P2pStreamingRpcResponse::StagedLedgerParts(response.unwrap()),
        },
        at_ms(2_000),
    )
}

/// Both relays advertised it: the nearer sends the staged ledger from the
/// first part, the other from the last, at once.
#[test]
fn two_relays_send_the_staged_ledger_from_both_ends() {
    let mut state = staged_node(2, true);
    let ran = run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let asked = requests(&ran);
    assert_eq!(asked.len(), 2, "{ran:#?}");
    let from_first = asked
        .iter()
        .find(|a| !a.contains("FromLast"))
        .expect("from the first");
    let from_last = asked
        .iter()
        .find(|a| a.contains("FromLast"))
        .expect("from the last");
    assert!(
        from_first.contains(&format!("{:?}", near())),
        "{from_first}"
    );
    assert!(from_last.contains(&format!("{:?}", other())), "{from_last}");

    // Both live: no more on the next tick.
    assert!(!StagedAction::PartsPeerFetchInit.is_enabled(&state, at_ms(1_500)));
}

/// Not advertised, or one stream set: one request, as before.
#[test]
fn one_relay_sends_it_without_the_advertisement_or_with_one_stream() {
    for (streams, advertised) in [(1, true), (2, false)] {
        let mut state = staged_node(streams, advertised);
        let ran = run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
        assert_eq!(requests(&ran).len(), 1, "{ran:#?}");
        assert!(
            !StagedAction::PartsPeerFetchInit.is_enabled(&state, at_ms(1_500)),
            "a second stream with streams={streams} advertised={advertised}"
        );
    }
}

/// The two streams meet: the parts are put together as the second's
/// answer, the first is cancelled and its part on the way dropped. The
/// parts here don't hash to the target, so both peers count as invalid,
/// neither is rejected, and the next fetch is from one at a time.
#[test]
fn the_two_halves_are_put_together_and_the_other_cancelled() {
    let mut state = staged_node(2, true);
    run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let first = local_rpc(&state, &near()).unwrap();
    let last = local_rpc(&state, &other()).unwrap();
    let data = staged_ledger(5);

    // From the first: base, scan state base, zkapp updates, trees 0 and 1.
    for sent in 0..5 {
        part(&mut state, near(), first, from_first_part(&data, sent));
    }
    // From the last: scan state base, trees 4 and 3.
    for sent in 0..3 {
        part(&mut state, other(), last, from_last_part(&data, sent));
    }
    assert!(local_rpc(&state, &near()).is_some());
    // Tree 2 from the last completes it.
    let ran = part(&mut state, other(), last, from_last_part(&data, 3));
    assert_cancelled(&ran, near(), first);
    assert_cancelled(&ran, other(), last);
    assert!(
        ran.iter().any(|a| a.contains("PartsPeerFetchSuccess")),
        "{ran:#?}"
    );
    assert!(
        ran.iter().any(|a| a.contains("PartsPeerInvalid")),
        "{ran:#?}"
    );
    assert!(
        !ran.iter().any(|a| a.contains("SyncAnswerRejected")),
        "{ran:#?}"
    );
    assert!(
        matches!(local(&state, &near()), P2pStreamingRpcLocalState::Cancelled { id, .. } if id == first)
    );
    // The tree the first was sending comes, and is dropped.
    part(&mut state, near(), first, from_first_part(&data, 5));
    assert!(
        matches!(local(&state, &near()), P2pStreamingRpcLocalState::Responded { id, .. } if id == first)
    );

    // Fake parts: both invalid, one stream from now on.
    let staged = state
        .transition_frontier
        .sync
        .ledger()
        .and_then(|l| l.staged())
        .unwrap();
    let TransitionFrontierSyncLedgerStagedState::PartsFetchPending {
        attempts,
        single_stream,
        ..
    } = staged
    else {
        panic!("{staged:?}");
    };
    assert!(*single_stream);
    for peer in [near(), other()] {
        assert!(
            matches!(
                attempts.get(&peer),
                Some(PeerStagedLedgerPartsFetchState::Invalid { .. })
            ),
            "{attempts:?}"
        );
    }
}

/// Parts from one relay alone that fail the hash check: that relay is
/// rejected.
#[test]
fn parts_from_one_relay_that_fail_the_hash_reject_it() {
    let mut state = staged_node(1, true);
    run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let id = local_rpc(&state, &near()).unwrap();
    let data = staged_ledger(5);
    let mut ran = vec![];
    for sent in 0..parts_count(&data) {
        ran = part(&mut state, near(), id, from_first_part(&data, sent));
    }
    assert!(
        ran.iter().any(|a| a.contains("PartsPeerInvalid")),
        "{ran:#?}"
    );
    let rejected: Vec<_> = ran
        .iter()
        .filter(|a| a.contains("SyncAnswerRejected"))
        .collect();
    assert_eq!(rejected.len(), 1, "{ran:#?}");
    assert!(rejected[0].contains(&format!("{:?}", near())), "{ran:#?}");
}

/// The same, the first completing it: the last had asked for its next tree,
/// which comes after the cancel and is dropped, with no disconnection.
#[test]
fn the_first_completing_it_drops_the_lasts_part_on_its_way() {
    let mut state = staged_node(2, true);
    run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let first = local_rpc(&state, &near()).unwrap();
    let last = local_rpc(&state, &other()).unwrap();
    let data = staged_ledger(5);

    // From the last: scan state base, trees 4 and 3; tree 2 asked for.
    for sent in 0..3 {
        part(&mut state, other(), last, from_last_part(&data, sent));
    }
    // From the first: base, scan state base, zkapp updates, trees 0, 1, 2.
    for sent in 0..5 {
        part(&mut state, near(), first, from_first_part(&data, sent));
    }
    let ran = part(&mut state, near(), first, from_first_part(&data, 5));
    assert!(
        ran.iter().any(|a| a.contains("PartsPeerFetchSuccess")),
        "{ran:#?}"
    );
    assert_cancelled(&ran, near(), first);
    assert_cancelled(&ran, other(), last);
    assert!(
        matches!(local(&state, &other()), P2pStreamingRpcLocalState::Cancelled { id, .. } if id == last)
    );
    let ran = part(&mut state, other(), last, from_last_part(&data, 3));
    assert!(!ran.iter().any(|a| a.contains("Disconnection")), "{ran:#?}");
    assert!(
        matches!(local(&state, &other()), P2pStreamingRpcLocalState::Responded { id, .. } if id == last)
    );
}

/// A relay told to stop mid-stream drops it, and takes the browser's next
/// request instead of dropping the browser.
#[test]
fn a_relay_cancelled_mid_stream_takes_the_next_request() {
    let request = P2pStreamingRpcRequest::StagedLedgerPartsFromLast(v2::StateHash::zero());
    let relay_mid_stream = || {
        let mut relay = staged_node(1, true);
        let p2p = relay.p2p.ready_mut().unwrap();
        let channel = &mut p2p
            .get_ready_peer_mut(&near())
            .unwrap()
            .channels
            .streaming_rpc;
        if let P2pChannelsStreamingRpcState::Ready { remote, .. } = channel {
            *remote = P2pStreamingRpcRemoteState::Requested {
                time: at_ms(1_000),
                id: 0,
                request: Box::new(request.clone()),
                progress: StagedLedgerPartsSendProgress::FromLastSending {
                    time: at_ms(2_000),
                    data: staged_ledger(5).into(),
                    sent: 4,
                }
                .into(),
            };
        }
        relay
    };
    let message = |relay: &mut crate::State, msg: StreamingRpcChannelMsg, now| {
        run(
            relay,
            P2pChannelsMessageReceivedAction {
                peer_id: near(),
                message: Box::new(msg.into()),
            },
            now,
        )
    };

    // Without a cancel, a request mid-stream is unexpected.
    let mut relay = relay_mid_stream();
    let ran = message(
        &mut relay,
        StreamingRpcChannelMsg::Request(1, request.clone()),
        at_ms(10_000),
    );
    assert!(ran.iter().any(|a| a.contains("Disconnection")), "{ran:#?}");

    let mut relay = relay_mid_stream();
    let ran = message(&mut relay, StreamingRpcChannelMsg::Cancel(0), at_ms(9_000));
    // Mid-stream, the part asked for has gone already: no empty answer.
    assert!(
        !ran.iter().any(|a| a.contains("Response(0, None)")),
        "{ran:#?}"
    );
    let ran = message(
        &mut relay,
        StreamingRpcChannelMsg::Request(1, request.clone()),
        at_ms(10_000),
    );
    assert!(!ran.iter().any(|a| a.contains("Disconnection")), "{ran:#?}");
    assert!(
        ran.iter().any(|a| a.contains("RequestReceived")),
        "{ran:#?}"
    );
}

fn new_request(
    state: &crate::State,
    peer: PeerId,
    request: P2pStreamingRpcRequest,
) -> P2pChannelsStreamingRpcAction {
    P2pChannelsStreamingRpcAction::RequestSend {
        peer_id: peer,
        id: state
            .p2p
            .get_ready_peer(&peer)
            .unwrap()
            .channels
            .next_local_rpc_id(),
        request: Box::new(request),
        on_init: None,
    }
}

/// After the two meet, and the part on its way is dropped, both channels
/// take a new request.
#[test]
fn both_channels_take_a_new_request_after_the_meet() {
    let mut state = staged_node(2, true);
    run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let first = local_rpc(&state, &near()).unwrap();
    let last = local_rpc(&state, &other()).unwrap();
    let data = staged_ledger(5);
    for sent in 0..5 {
        part(&mut state, near(), first, from_first_part(&data, sent));
    }
    for sent in 0..3 {
        part(&mut state, other(), last, from_last_part(&data, sent));
    }
    part(&mut state, other(), last, from_last_part(&data, 3));
    part(&mut state, near(), first, from_first_part(&data, 5));
    for peer in [near(), other()] {
        let request = P2pStreamingRpcRequest::StagedLedgerParts(v2::StateHash::zero());
        let send = new_request(&state, peer, request);
        assert!(send.is_enabled(&state, at_ms(2_100)), "{peer:?}");
    }
}

/// A part that never comes after a cancel frees the channel after a part's
/// timeout, not before.
#[test]
fn a_cancelled_stream_whose_part_never_comes_frees_the_channel() {
    let mut state = staged_node(2, true);
    run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let id = local_rpc(&state, &near()).unwrap();
    run(
        &mut state,
        P2pChannelsStreamingRpcAction::RequestCancel {
            peer_id: near(),
            id,
        },
        at_ms(1_001),
    );
    assert!(matches!(
        local(&state, &near()),
        P2pStreamingRpcLocalState::Cancelled { .. }
    ));
    let request = P2pStreamingRpcRequest::StagedLedgerParts(v2::StateHash::zero());
    let send = new_request(&state, near(), request);
    assert!(!send.is_enabled(&state, at_ms(30_000)));
    assert!(send.is_enabled(&state, at_ms(31_001)));
}

/// A stream from the last goes only to a peer that advertised it.
#[test]
fn a_stream_from_the_last_goes_only_to_a_peer_that_advertised_it() {
    for advertised in [false, true] {
        let state = staged_node(2, advertised);
        let request = P2pStreamingRpcRequest::StagedLedgerPartsFromLast(v2::StateHash::zero());
        let send = new_request(&state, near(), request);
        assert_eq!(send.is_enabled(&state, at_ms(1_000)), advertised);
    }
}

/// Either relay dropping mid-way: the other finishes alone.
#[test]
fn the_other_stream_finishes_when_either_relay_drops() {
    for lose_first in [true, false] {
        let mut state = staged_node(2, true);
        run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
        let first = local_rpc(&state, &near()).unwrap();
        let last = local_rpc(&state, &other()).unwrap();
        let data = staged_ledger(5);
        for sent in 0..4 {
            part(&mut state, near(), first, from_first_part(&data, sent));
        }
        for sent in 0..3 {
            part(&mut state, other(), last, from_last_part(&data, sent));
        }
        let lost = if lose_first { near() } else { other() };
        state.p2p.ready_mut().unwrap().peers.remove(&lost);
        run(
            &mut state,
            P2pCallbacksAction::P2pDisconnection { peer_id: lost },
            at_ms(2_001),
        );
        let mut ran = vec![];
        if lose_first {
            for sent in 3..8 {
                ran.extend(part(&mut state, other(), last, from_last_part(&data, sent)));
            }
        } else {
            for sent in 4..8 {
                ran.extend(part(
                    &mut state,
                    near(),
                    first,
                    from_first_part(&data, sent),
                ));
            }
        }
        assert!(
            ran.iter().any(|a| a.contains("PartsPeerFetchSuccess")),
            "{ran:#?}"
        );
    }
}

/// A cancel before the relay sent its first part: the relay answers empty,
/// and both ends are free.
#[test]
fn a_cancel_before_the_first_part_frees_both_ends() {
    let mut requester = staged_node(2, true);
    run(
        &mut requester,
        StagedAction::PartsPeerFetchInit,
        at_ms(1_000),
    );
    let id = local_rpc(&requester, &near()).unwrap();
    run(
        &mut requester,
        P2pChannelsStreamingRpcAction::RequestCancel {
            peer_id: near(),
            id,
        },
        at_ms(1_001),
    );
    let mut relay = staged_node(1, true);
    let channel = &mut relay
        .p2p
        .ready_mut()
        .unwrap()
        .get_ready_peer_mut(&near())
        .unwrap()
        .channels
        .streaming_rpc;
    if let P2pChannelsStreamingRpcState::Ready { remote, .. } = channel {
        *remote = P2pStreamingRpcRemoteState::Requested {
            time: at_ms(1_000),
            id,
            request: Box::new(P2pStreamingRpcRequest::StagedLedgerParts(
                v2::StateHash::zero(),
            )),
            progress: StagedLedgerPartsSendProgress::LedgerGetPending { time: at_ms(1_000) }.into(),
        };
    }
    let ran = run(
        &mut relay,
        P2pChannelsStreamingRpcAction::CancelReceived {
            peer_id: near(),
            id,
        },
        at_ms(1_002),
    );
    assert!(
        ran.iter()
            .any(|a| a.contains("MessageSend") && a.contains(&format!("Response({id}, None)"))),
        "{ran:#?}"
    );
    run(
        &mut requester,
        P2pChannelsStreamingRpcAction::ResponseReceived {
            peer_id: near(),
            id,
            response: None,
        },
        at_ms(1_003),
    );
    assert!(matches!(
        local(&requester, &near()),
        P2pStreamingRpcLocalState::Responded { .. }
    ));
}

/// Parts put together that fail the hash check: the retry is one stream
/// from the first.
#[test]
fn after_a_bad_union_the_retry_is_one_stream() {
    let mut state = staged_node(2, true);
    run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let first = local_rpc(&state, &near()).unwrap();
    let last = local_rpc(&state, &other()).unwrap();
    let data = staged_ledger(5);
    for sent in 0..5 {
        part(&mut state, near(), first, from_first_part(&data, sent));
    }
    for sent in 0..4 {
        part(&mut state, other(), last, from_last_part(&data, sent));
    }
    part(&mut state, near(), first, from_first_part(&data, 5));
    let ran = run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(10_000));
    let asked = requests(&ran);
    assert_eq!(asked.len(), 1, "{ran:#?}");
    assert!(!asked[0].contains("FromLast"), "{asked:?}");
}

fn channel_message(
    state: &mut crate::State,
    peer: PeerId,
    msg: StreamingRpcChannelMsg,
    now: Timestamp,
) -> Vec<String> {
    run(
        state,
        P2pChannelsMessageReceivedAction {
            peer_id: peer,
            message: Box::new(msg.into()),
        },
        now,
    )
}

/// A request cancelled with its first part on its way, then, after a part's
/// timeout, a new request on the same channel. Returns the two ids.
fn reused_after_a_cancel(state: &mut crate::State) -> (P2pStreamingRpcId, P2pStreamingRpcId) {
    run(state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let old = local_rpc(state, &near()).unwrap();
    run(
        state,
        P2pChannelsStreamingRpcAction::RequestCancel {
            peer_id: near(),
            id: old,
        },
        at_ms(1_001),
    );
    let request = P2pStreamingRpcRequest::StagedLedgerParts(v2::StateHash::zero());
    let send = new_request(state, near(), request);
    run(state, send, at_ms(31_001));
    let new = local_rpc(state, &near()).unwrap();
    assert_ne!(old, new);
    (old, new)
}

/// The cancelled request's part, or its empty answer, comes after the
/// channel took a new request: it is dropped, and the new request goes on.
#[test]
fn a_late_message_for_a_cancelled_request_is_dropped_after_reuse() {
    let data = staged_ledger(5);
    for empty in [false, true] {
        let mut state = staged_node(2, true);
        let (old, new) = reused_after_a_cancel(&mut state);
        let late = (!empty).then(|| {
            P2pStreamingRpcResponse::StagedLedgerParts(from_first_part(&data, 0).unwrap())
        });
        let ran = channel_message(
            &mut state,
            near(),
            StreamingRpcChannelMsg::Response(old, late),
            at_ms(31_002),
        );
        assert!(!ran.iter().any(|a| a.contains("Disconnection")), "{ran:#?}");
        assert!(
            !ran.iter().any(|a| a.contains("PartsPeerFetch")),
            "{ran:#?}"
        );
        assert_eq!(local_rpc(&state, &near()), Some(new));
        // The new request's first part is taken as usual.
        let ran = channel_message(
            &mut state,
            near(),
            StreamingRpcChannelMsg::Response(
                new,
                from_first_part(&data, 0).map(P2pStreamingRpcResponse::StagedLedgerParts),
            ),
            at_ms(31_003),
        );
        assert!(!ran.iter().any(|a| a.contains("Disconnection")), "{ran:#?}");
        assert!(
            ran.iter().any(|a| a.contains("ResponseNextPartGet")),
            "{ran:#?}"
        );
    }
}

/// Only one cancelled request is kept track of: the channel takes no third
/// request while the first one's part may still come. Once it comes, it
/// does.
#[test]
fn a_second_reuse_waits_for_the_first_cancelled_part() {
    let data = staged_ledger(5);
    let mut state = staged_node(2, true);
    let (old, new) = reused_after_a_cancel(&mut state);
    run(
        &mut state,
        P2pChannelsStreamingRpcAction::RequestCancel {
            peer_id: near(),
            id: new,
        },
        at_ms(31_002),
    );
    let request = P2pStreamingRpcRequest::StagedLedgerParts(v2::StateHash::zero());
    let third = new_request(&state, near(), request);
    assert!(!third.is_enabled(&state, at_ms(70_000)));
    channel_message(
        &mut state,
        near(),
        StreamingRpcChannelMsg::Response(
            old,
            from_first_part(&data, 0).map(P2pStreamingRpcResponse::StagedLedgerParts),
        ),
        at_ms(70_001),
    );
    assert!(third.is_enabled(&state, at_ms(70_002)));
}

/// Parts put together that fail the hash check leave the first stream
/// cancelled with a tree on its way. The retry, a part's timeout later,
/// reuses that channel, and the tree then comes: dropped, no disconnection.
#[test]
fn the_retry_after_a_bad_union_drops_the_cancelled_tree() {
    let mut state = staged_node(2, true);
    run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let first = local_rpc(&state, &near()).unwrap();
    let last = local_rpc(&state, &other()).unwrap();
    let data = staged_ledger(5);
    for sent in 0..5 {
        part(&mut state, near(), first, from_first_part(&data, sent));
    }
    for sent in 0..4 {
        part(&mut state, other(), last, from_last_part(&data, sent));
    }
    assert!(matches!(
        local(&state, &near()),
        P2pStreamingRpcLocalState::Cancelled { .. }
    ));
    state.p2p.ready_mut().unwrap().peers.remove(&other());
    let ran = run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(32_001));
    assert_eq!(requests(&ran).len(), 1, "{ran:#?}");
    let retry = local_rpc(&state, &near()).unwrap();
    assert_ne!(retry, first);
    let ran = channel_message(
        &mut state,
        near(),
        StreamingRpcChannelMsg::Response(
            first,
            from_first_part(&data, 5).map(P2pStreamingRpcResponse::StagedLedgerParts),
        ),
        at_ms(32_002),
    );
    assert!(!ran.iter().any(|a| a.contains("Disconnection")), "{ran:#?}");
    assert_eq!(local_rpc(&state, &near()), Some(retry));
}

/// The root moves while the two streams run, to a root of the same snarked
/// ledger: the staged ledger target is kept, the new root counted as an
/// update (WEB-ROOT-RETARGET).
pub(super) fn advance_root(state: &mut crate::State) {
    let TransitionFrontierSyncState::RootLedgerPending(pending) =
        &mut state.transition_frontier.sync
    else {
        panic!("not syncing the root ledger");
    };
    let snarked = pending
        .ledger
        .staged()
        .unwrap()
        .target()
        .snarked_ledger_hash
        .clone();
    let old = pending.root_block.clone();
    let mut next = block(&template_block(), 101, Some(&old), 7);
    Arc::make_mut(&mut next.block)
        .header
        .protocol_state
        .body
        .blockchain_state
        .ledger_proof_statement
        .target
        .first_pass_ledger = snarked;
    pending.root_block = next;
    pending.update_root_ledger_target(at_ms(2_000), old);
    assert_eq!(pending.root_block_updates.len(), 1);
}

/// Parts put together that fail the hash check after the root moved: the
/// fetch retargets to the new root, and the new fetch still waits the
/// retry delay for both relays and then asks one, from the first, with no
/// relay rejected.
#[test]
fn a_bad_union_after_the_root_moved_retries_one_stream_after_the_delay() {
    let mut state = staged_node(2, true);
    run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let first = local_rpc(&state, &near()).unwrap();
    let last = local_rpc(&state, &other()).unwrap();
    advance_root(&mut state);
    let data = staged_ledger(5);
    for sent in 0..5 {
        part(&mut state, near(), first, from_first_part(&data, sent));
    }
    for sent in 0..3 {
        part(&mut state, other(), last, from_last_part(&data, sent));
    }
    let mut ran = part(&mut state, other(), last, from_last_part(&data, 3));
    // The cancelled stream's part on its way comes, and is dropped.
    part(&mut state, near(), first, from_first_part(&data, 5));
    ran.extend(run(
        &mut state,
        StagedAction::PartsPeerFetchInit,
        at_ms(2_000),
    ));
    assert!(
        ran.iter().any(|a| a.contains("LedgerRootRetarget")),
        "{ran:#?}"
    );
    assert!(requests(&ran).is_empty(), "asked at once: {ran:#?}");
    assert!(
        !ran.iter().any(|a| a.contains("SyncAnswerRejected")),
        "{ran:#?}"
    );
    let staged = state
        .transition_frontier
        .sync
        .ledger()
        .and_then(|l| l.staged())
        .unwrap();
    assert!(
        matches!(
            staged,
            TransitionFrontierSyncLedgerStagedState::PartsFetchPending {
                single_stream: true,
                ..
            }
        ),
        "single-stream fallback lost on root retarget: {staged:?}"
    );

    let ran = run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(7_000));
    let asked = requests(&ran);
    assert_eq!(asked.len(), 1, "{ran:#?}");
    assert!(!asked[0].contains("FromLast"), "{asked:?}");
    assert!(!StagedAction::PartsPeerFetchInit.is_enabled(&state, at_ms(7_500)));
}
