//! WEB-NEAREST-RELAY: a web node with a near and a far relay sends its bulk
//! sync to the near one, and falls back to the far one when the near one
//! stalls, times out or drops. Scheduler tests as in
//! `webrtc_pipeline_sync_tests`.
use std::time::Duration;

use super::*;
use crate::{
    ledger::LedgerAddress,
    p2p::{
        channels::{
            rpc::{
                P2pChannelsRpcAction, P2pChannelsRpcState, P2pRpcLocalState, P2pRpcPipeline,
                P2pRpcRemoteState, P2pRpcRequest, P2pRpcResponse, P2pSyncPeerStats,
            },
            streaming_rpc::{
                P2pChannelsStreamingRpcState, P2pStreamingRpcLocalState, P2pStreamingRpcRemoteState,
            },
        },
        P2pPeerState, P2pPeerStatus, P2pPeerStatusReady, PeerId,
    },
    transition_frontier::sync::ledger::{
        snarked::{
            LedgerAddressQueryPending, PeerRpcState as LedgerRpc,
            TransitionFrontierSyncLedgerSnarkedAction as SnarkedAction,
        },
        staged::{
            TransitionFrontierSyncLedgerStagedAction as StagedAction,
            TransitionFrontierSyncLedgerStagedState,
        },
    },
};
use mina_p2p_messages::v2::MinaLedgerSyncLedgerAnswerStableV2;
use redux::{ActionMeta, Dispatcher, EnablingCondition};

pub(super) fn near() -> PeerId {
    PeerId::from_bytes([11; 32])
}

fn far() -> PeerId {
    PeerId::from_bytes([12; 32])
}

pub(super) fn at_ms(ms: u64) -> Timestamp {
    Timestamp::new(1_000_000_000_000u64.saturating_add(ms.saturating_mul(1_000_000)))
}

/// A peer that has answered sync requests in `fastest_ms` at best.
fn stats(fastest_ms: u64) -> P2pSyncPeerStats {
    let mut stats = P2pSyncPeerStats::default();
    stats.record_answer(at_ms(0), at_ms(fastest_ms));
    stats
}

pub(super) fn add_peer(
    state: &mut crate::State,
    peer: PeerId,
    is_libp2p: bool,
    root: &ArcBlockWithHash,
    stats: Option<P2pSyncPeerStats>,
) {
    let mut ready = P2pPeerStatusReady::new(false, Timestamp::ZERO, &Default::default());
    ready.best_tip = Some(root.clone());
    ready.channels.rpc = P2pChannelsRpcState::Ready {
        time: Timestamp::ZERO,
        local: P2pRpcLocalState::WaitingForRequest {
            time: Timestamp::ZERO,
        },
        remote: P2pRpcRemoteState {
            pending_requests: Default::default(),
            last_responded: Timestamp::ZERO,
        },
    };
    ready.channels.rpc_pipeline = P2pRpcPipeline::for_peer(is_libp2p);
    ready.channels.streaming_rpc = P2pChannelsStreamingRpcState::Ready {
        time: Timestamp::ZERO,
        local: P2pStreamingRpcLocalState::WaitingForRequest {
            time: Timestamp::ZERO,
        },
        remote: P2pStreamingRpcRemoteState::WaitingForRequest {
            time: Timestamp::ZERO,
        },
        remote_last_responded: Timestamp::ZERO,
    };
    ready.channels.sync_stats = stats.unwrap_or_default();
    state.p2p.ready_mut().unwrap().peers.insert(
        peer,
        P2pPeerState {
            is_libp2p,
            dial_opts: None,
            identify: None,
            status: P2pPeerStatus::Ready(Box::new(ready)),
        },
    );
}

pub(super) fn initialized_node() -> (crate::State, ArcBlockWithHash) {
    let root = block(&template_block(), 100, None, 7);
    let mut state = sync_test_node(vec![AppliedBlock {
        block: root.clone(),
        just_emitted_a_proof: false,
    }]);
    let chain_id = serde_json::from_str(
        r#""0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886""#,
    )
    .unwrap();
    state.p2p.initialize(&chain_id).unwrap();
    (state, root)
}

/// A node syncing the staking ledger with `queued` account subtrees left to
/// ask for, and no peers yet.
fn ledger_node(queued: usize) -> (crate::State, ArcBlockWithHash) {
    let (mut state, root) = initialized_node();
    let mut address = LedgerAddress::first(29);
    let mut queue = std::collections::BTreeMap::new();
    for n in 0..queued {
        queue.insert(
            address.clone(),
            ledger_hash(100u64.saturating_add(n as u64)),
        );
        address = address.next().unwrap();
    }
    state.transition_frontier.sync =
        TransitionFrontierSyncState::StakingLedgerPending(TransitionFrontierSyncLedgerPending {
            time: Timestamp::ZERO,
            best_tip: root.clone(),
            root_block: root.clone(),
            blocks_inbetween: vec![],
            root_block_updates: vec![],
            ledger: TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncPending {
                time: Timestamp::ZERO,
                target: SyncLedgerTarget::staking_epoch(&root),
                total_accounts_expected: 64u64.saturating_mul(queued as u64),
                synced_accounts_count: 0,
                synced_hashes_count: 0,
                queue,
                pending_addresses: Default::default(),
                unavailable_peers: Default::default(),
                peer_query_errors: Default::default(),
            }
            .into(),
        });
    (state, root)
}

/// Runs `action` and what it queues, reducers only, as in
/// `webrtc_pipeline_sync_tests`. Returns what ran.
pub(super) fn run(
    state: &mut crate::State,
    action: impl Into<crate::Action>,
    now: Timestamp,
) -> Vec<String> {
    fn go(state: &mut crate::State, action: crate::Action, now: Timestamp, ran: &mut Vec<String>) {
        if !action.is_enabled(state, now) {
            return;
        }
        let mut dispatcher = Dispatcher::new();
        crate::reducer(
            state,
            &ActionMeta::zero_custom(now).with_action(action.clone()),
            &mut dispatcher,
        );
        ran.push(format!("{action:?}"));
        assert!(ran.len() < 10_000);
        while let Some(next) = dispatcher.pop() {
            go(state, next, now, ran);
        }
    }
    let mut ran = vec![];
    let action = action.into();
    assert!(action.is_enabled(state, now), "{action:?}");
    go(state, action, now, &mut ran);
    ran
}

/// How many ledger queries each peer has pending.
fn pending(state: &crate::State, peer: &PeerId) -> usize {
    let Some(TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncPending {
        pending_addresses,
        ..
    }) = state
        .transition_frontier
        .sync
        .ledger()
        .and_then(|l| l.snarked())
    else {
        panic!("not syncing addresses");
    };
    pending_addresses
        .values()
        .filter(|p: &&LedgerAddressQueryPending| {
            matches!(p.attempts.get(peer), Some(LedgerRpc::Pending { .. }))
        })
        .count()
}

fn pending_rpc_ids(state: &crate::State, peer: &PeerId) -> Vec<u64> {
    state
        .p2p
        .get_ready_peer(peer)
        .unwrap()
        .channels
        .unanswered_rpc_ids()
        .collect()
}

/// A near relay (20 ms) and a far one (200 ms): the near one gets five
/// ledger queries, all it takes, and the far one one.
#[test]
fn the_near_relay_takes_the_ledger_queries() {
    let (mut state, root) = ledger_node(20);
    add_peer(&mut state, far(), false, &root, Some(stats(200)));
    add_peer(&mut state, near(), false, &root, Some(stats(20)));
    run(&mut state, SnarkedAction::PeersQuery, at_ms(1_000));
    assert_eq!(pending(&state, &near()), 5);
    assert_eq!(pending(&state, &far()), 1);

    // With few queries left, they all go to the near one.
    let (mut state, root) = ledger_node(3);
    add_peer(&mut state, far(), false, &root, Some(stats(200)));
    add_peer(&mut state, near(), false, &root, Some(stats(20)));
    run(&mut state, SnarkedAction::PeersQuery, at_ms(1_000));
    assert_eq!(pending(&state, &near()), 3);
    assert_eq!(pending(&state, &far()), 0);

    // Before either has answered, both get five, as before.
    let (mut state, root) = ledger_node(20);
    add_peer(&mut state, far(), false, &root, None);
    add_peer(&mut state, near(), false, &root, None);
    run(&mut state, SnarkedAction::PeersQuery, at_ms(1_000));
    assert_eq!(pending(&state, &near()), 5);
    assert_eq!(pending(&state, &far()), 5);
}

/// The near relay goes quiet with five queries out: once the oldest is
/// older than four times its usual answer time and 2 s, it is stalled and
/// the far relay gets all it takes. A timeout demotes it the same way, for
/// 30 s; a dropped relay is simply gone.
#[test]
fn the_far_relay_takes_over_when_the_near_one_stalls_times_out_or_drops() {
    // The near relay has had five queries out since 1 s; the far one
    // has none out.
    let stalled_at = |now: u64| {
        let (mut state, root) = ledger_node(40);
        add_peer(&mut state, near(), false, &root, Some(stats(20)));
        run(&mut state, SnarkedAction::PeersQuery, at_ms(1_000));
        assert_eq!(pending(&state, &near()), 5);
        add_peer(&mut state, far(), false, &root, Some(stats(200)));
        run(&mut state, SnarkedAction::PeersQuery, at_ms(now));
        pending(&state, &far())
    };
    // Not stalled at 2.9 s: the far one gets its one.
    assert_eq!(stalled_at(2_900), 1);
    // At 3.1 s the queries are 2.1 s old: stalled, and the far one gets five.
    assert_eq!(stalled_at(3_100), 5);

    // A near relay whose query timed out 10 s ago, with nothing pending.
    let (mut state, root) = ledger_node(40);
    let mut timed_out = stats(20);
    timed_out.record_timeout(at_ms(0));
    add_peer(&mut state, far(), false, &root, Some(stats(200)));
    add_peer(&mut state, near(), false, &root, Some(timed_out.clone()));
    run(&mut state, SnarkedAction::PeersQuery, at_ms(10_000));
    assert_eq!((pending(&state, &near()), pending(&state, &far())), (1, 5));
    // 30 s after the timeout it is near again.
    let (mut state, root) = ledger_node(40);
    add_peer(&mut state, far(), false, &root, Some(stats(200)));
    add_peer(&mut state, near(), false, &root, Some(timed_out));
    run(&mut state, SnarkedAction::PeersQuery, at_ms(30_001));
    assert_eq!((pending(&state, &near()), pending(&state, &far())), (5, 1));

    // Only the far relay left: it gets all it takes.
    let (mut state, root) = ledger_node(40);
    add_peer(&mut state, far(), false, &root, Some(stats(200)));
    run(&mut state, SnarkedAction::PeersQuery, at_ms(1_000));
    assert_eq!(pending(&state, &far()), 5);
}

/// A useful answer to a ledger query times the peer; an empty one, as a
/// sharing browser's refusal, or an error does not.
#[test]
fn answers_time_the_peer_and_refusals_do_not() {
    let (mut state, root) = ledger_node(8);
    add_peer(&mut state, near(), false, &root, None);
    run(&mut state, SnarkedAction::PeersQuery, at_ms(0));
    let ids = pending_rpc_ids(&state, &near());
    assert_eq!(ids.len(), 5);
    let id = |n: usize| ids.get(n).copied().unwrap();
    let sync_stats = |state: &crate::State| {
        state
            .p2p
            .get_ready_peer(&near())
            .unwrap()
            .channels
            .sync_stats
            .clone()
    };

    run(
        &mut state,
        P2pChannelsRpcAction::ResponseReceived {
            peer_id: near(),
            id: id(0),
            response: None,
        },
        at_ms(10),
    );
    run(
        &mut state,
        P2pChannelsRpcAction::ResponseReceived {
            peer_id: near(),
            id: id(1),
            response: Some(Box::new(P2pRpcResponse::LedgerQueryError(
                ledger_hash(1),
                "no".into(),
            ))),
        },
        at_ms(12),
    );
    assert_eq!(sync_stats(&state), P2pSyncPeerStats::default());

    run(
        &mut state,
        P2pChannelsRpcAction::ResponseReceived {
            peer_id: near(),
            id: id(2),
            response: Some(Box::new(P2pRpcResponse::LedgerQuery(
                MinaLedgerSyncLedgerAnswerStableV2::ContentsAre(Vec::new().into_iter().collect()),
            ))),
        },
        at_ms(30),
    );
    let measured = sync_stats(&state);
    assert_eq!(measured.answers, 1);
    assert_eq!(
        measured.fastest.map(|(d, _)| d),
        Some(Duration::from_millis(30))
    );
}

/// The staged ledger, one large download, goes to the near relay.
#[test]
fn the_staged_ledger_comes_from_the_near_relay() {
    for (near_first, libp2p_too) in [(true, false), (false, false), (false, true)] {
        let (mut state, root) = initialized_node();
        let ocaml = PeerId::from_bytes([13; 32]);
        if near_first {
            add_peer(&mut state, near(), false, &root, Some(stats(20)));
            add_peer(&mut state, far(), false, &root, Some(stats(200)));
        } else {
            add_peer(&mut state, far(), false, &root, Some(stats(200)));
            add_peer(&mut state, near(), false, &root, Some(stats(20)));
        }
        if libp2p_too {
            add_peer(&mut state, ocaml, true, &root, None);
        }
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
                }
                .into(),
            });
        let ran = run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
        let asked = ran
            .iter()
            .filter(|a| {
                a.contains("RequestSend")
                    && (a.contains("StagedLedgerParts") || a.contains("StagedLedgerAux"))
            })
            .collect::<Vec<_>>();
        let [asked] = asked.as_slice() else {
            panic!("{ran:#?}");
        };
        if libp2p_too {
            // A libp2p peer keeps its shuffled place: it may come first.
            assert!(
                asked.contains(&format!("{:?}", near())) || asked.contains(&format!("{ocaml:?}")),
                "{asked:?}"
            );
        } else {
            assert!(asked.contains(&format!("{:?}", near())), "{asked:?}");
        }
    }
}

/// libp2p peers aren't ranked: each takes one query, timed or not.
#[test]
fn libp2p_peers_are_not_ranked() {
    let (mut state, root) = ledger_node(20);
    add_peer(&mut state, far(), true, &root, Some(stats(200)));
    add_peer(&mut state, near(), true, &root, Some(stats(20)));
    run(&mut state, SnarkedAction::PeersQuery, at_ms(1_000));
    assert_eq!((pending(&state, &near()), pending(&state, &far())), (1, 1));
    // A libp2p peer's answer doesn't time it.
    let id = pending_rpc_ids(&state, &near()).first().copied().unwrap();
    let before = state
        .p2p
        .get_ready_peer(&near())
        .unwrap()
        .channels
        .sync_stats
        .clone();
    run(
        &mut state,
        P2pChannelsRpcAction::ResponseReceived {
            peer_id: near(),
            id,
            response: Some(Box::new(P2pRpcResponse::LedgerQuery(
                MinaLedgerSyncLedgerAnswerStableV2::NumAccounts(0u64.into(), ledger_hash(1)),
            ))),
        },
        at_ms(1_100),
    );
    let after = &state
        .p2p
        .get_ready_peer(&near())
        .unwrap()
        .channels
        .sync_stats;
    assert_eq!(&before, after);
}

/// An answer the sync rejects drops the peer's timing, as it was timed
/// before it was checked: a wrong kind of answer, or accounts that don't
/// check. The far relay then takes the queries.
#[test]
fn a_rejected_answer_drops_the_peers_timing() {
    let sync_stats = |state: &crate::State| {
        state
            .p2p
            .get_ready_peer(&near())
            .unwrap()
            .channels
            .sync_stats
            .clone()
    };

    // An account subtree asked for, and the number of accounts sent back at
    // once.
    let (mut state, root) = ledger_node(8);
    add_peer(&mut state, near(), false, &root, None);
    run(&mut state, SnarkedAction::PeersQuery, at_ms(0));
    let ids = pending_rpc_ids(&state, &near());
    let ran = run(
        &mut state,
        P2pChannelsRpcAction::ResponseReceived {
            peer_id: near(),
            id: ids.first().copied().unwrap(),
            response: Some(Box::new(P2pRpcResponse::LedgerQuery(
                MinaLedgerSyncLedgerAnswerStableV2::NumAccounts(0u64.into(), ledger_hash(1)),
            ))),
        },
        at_ms(5),
    );
    assert!(
        ran.iter().any(|a| a.contains("SyncAnswerRejected")),
        "{ran:#?}"
    );
    let rejected = sync_stats(&state);
    assert_eq!(rejected.fastest, None);
    assert_eq!(rejected.rejected, Some(at_ms(5)));

    // Accounts that don't check, rejected as the effect does.
    let (mut state, root) = ledger_node(8);
    add_peer(&mut state, near(), false, &root, None);
    run(&mut state, SnarkedAction::PeersQuery, at_ms(0));
    let ids = pending_rpc_ids(&state, &near());
    run(
        &mut state,
        P2pChannelsRpcAction::ResponseReceived {
            peer_id: near(),
            id: ids.first().copied().unwrap(),
            response: Some(Box::new(P2pRpcResponse::LedgerQuery(
                MinaLedgerSyncLedgerAnswerStableV2::ContentsAre(Vec::new().into_iter().collect()),
            ))),
        },
        at_ms(5),
    );
    assert!(sync_stats(&state).fastest.is_some());
    let Some(TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncPending {
        pending_addresses,
        ..
    }) = state
        .transition_frontier
        .sync
        .ledger()
        .and_then(|l| l.snarked())
    else {
        panic!("not syncing addresses");
    };
    let address = pending_addresses
        .iter()
        .find(|(_, p)| p.attempts.get(&near()).is_some_and(|a| a.is_success()))
        .map(|(address, _)| address.clone())
        .unwrap();
    run(
        &mut state,
        SnarkedAction::ChildAccountsRejected {
            address,
            sender: near(),
        },
        at_ms(6),
    );
    assert_eq!(sync_stats(&state).fastest, None);

    // The far relay takes the queries while the near one is unmeasured.
    let mut rejected = stats(20);
    rejected.record_rejected(at_ms(0));
    let (mut state, root) = ledger_node(40);
    add_peer(&mut state, far(), false, &root, Some(stats(200)));
    add_peer(&mut state, near(), false, &root, Some(rejected));
    run(&mut state, SnarkedAction::PeersQuery, at_ms(10_000));
    assert_eq!((pending(&state, &near()), pending(&state, &far())), (1, 5));
}

/// A block other than the one asked for, sent at once, drops the peer's
/// timing: the sync won't take it. The block asked for is timed.
#[test]
fn another_block_than_the_one_asked_for_drops_the_timing() {
    // A real block: the test chain's blocks carry made-up hashes.
    let sent = BlockWithHash::try_new(Arc::new(template_block())).unwrap();
    let answer = |asked_for_other: bool| {
        let (mut state, root) = initialized_node();
        add_peer(&mut state, near(), false, &root, None);
        let asked = if asked_for_other {
            StateHash::from_fp(Fp::from(1))
        } else {
            sent.hash().clone()
        };
        let id = state
            .p2p
            .get_ready_peer(&near())
            .unwrap()
            .channels
            .next_local_rpc_id();
        run(
            &mut state,
            P2pChannelsRpcAction::RequestSend {
                peer_id: near(),
                id,
                request: Box::new(P2pRpcRequest::Block(asked)),
                on_init: None,
            },
            at_ms(0),
        );
        let ran = run(
            &mut state,
            P2pChannelsRpcAction::ResponseReceived {
                peer_id: near(),
                id,
                response: Some(Box::new(P2pRpcResponse::Block(sent.block.clone()))),
            },
            at_ms(5),
        );
        let stats = state
            .p2p
            .get_ready_peer(&near())
            .unwrap()
            .channels
            .sync_stats
            .clone();
        (
            ran.iter().any(|a| a.contains("SyncAnswerRejected")),
            stats.fastest.map(|(took, _)| took),
        )
    };
    assert_eq!(answer(false), (false, Some(Duration::from_millis(5))));
    assert_eq!(answer(true), (true, None));
}
