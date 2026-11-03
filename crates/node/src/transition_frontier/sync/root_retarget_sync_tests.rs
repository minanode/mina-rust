//! WEB-ROOT-RETARGET: a best tip that moves the root by one block, with the
//! same snarked ledger, keeps the staged ledger target while a peer is still
//! sending its parts, as OCaml's bootstrap keeps the root whose snarked ledger
//! it synced. A request that ends without the parts then targets the current
//! root. The same for a web node's WebRTC relay and a native node's libp2p
//! peer.
use super::{
    nearest_peer_sync_tests::{add_peer, at_ms, initialized_node, near, run},
    *,
};
use crate::{
    p2p::{
        channels::{
            rpc::{P2pChannelsRpcAction, P2pRpcId},
            streaming_rpc::P2pChannelsStreamingRpcAction,
        },
        PeerId,
    },
    transition_frontier::sync::{
        ledger::staged::{
            PeerStagedLedgerPartsFetchError, PeerStagedLedgerPartsFetchState,
            TransitionFrontierSyncLedgerStagedAction as StagedAction,
            STAGED_LEDGER_PARTS_STALE_AFTER,
        },
        TransitionFrontierSyncAction,
    },
};
use redux::{ActionMeta, Dispatcher, EnablingCondition};

fn at(time: Timestamp, after: std::time::Duration) -> Timestamp {
    time.checked_add(after.as_nanos() as u64).unwrap()
}

/// A root ledger sync for `root`, its snarked ledger synced, fetching the
/// staged ledger parts with `attempt` from a peer, if any.
fn fetching_parts(
    root: &ArcBlockWithHash,
    best_tip: &ArcBlockWithHash,
    attempt: Option<PeerStagedLedgerPartsFetchState>,
) -> TransitionFrontierSyncLedgerPending {
    TransitionFrontierSyncLedgerPending {
        time: Timestamp::ZERO,
        best_tip: best_tip.clone(),
        root_block: root.clone(),
        blocks_inbetween: vec![],
        root_block_updates: vec![],
        ledger: TransitionFrontierSyncLedgerStagedState::PartsFetchPending {
            time: Timestamp::ZERO,
            target: SyncLedgerTarget::root(root).with_staged().unwrap(),
            attempts: attempt.map(|a| (near(), a)).into_iter().collect(),
            single_stream: false,
        }
        .into(),
    }
}

fn pending_since(time: Timestamp) -> PeerStagedLedgerPartsFetchState {
    PeerStagedLedgerPartsFetchState::Pending { time, rpc_id: 7 }
}

/// [`best_tip_update`] at `time`.
fn best_tip_update_at(
    sync: &mut TransitionFrontierSyncLedgerPending,
    root: &ArcBlockWithHash,
    time: Timestamp,
) {
    let old_root = std::mem::replace(&mut sync.root_block, root.clone());
    sync.update_root_ledger_target(time, old_root);
}

/// The block whose staged ledger parts are being fetched, with the attempts.
fn parts_target(
    sync: &TransitionFrontierSyncLedgerPending,
) -> Option<(&StateHash, Vec<&PeerStagedLedgerPartsFetchState>)> {
    match &sync.ledger {
        TransitionFrontierSyncLedgerState::Staged(
            TransitionFrontierSyncLedgerStagedState::PartsFetchPending {
                target, attempts, ..
            },
        ) => Some((&target.staged.block_hash, attempts.values().collect())),
        _ => None,
    }
}

/// The staged target after a change of target that keeps the snarked
/// ledger: the parts are fetched again.
fn snarked_kept_target(sync: &TransitionFrontierSyncLedgerPending) -> Option<&StateHash> {
    match &sync.ledger {
        TransitionFrontierSyncLedgerState::Snarked(
            TransitionFrontierSyncLedgerSnarkedState::Success { target, .. },
        ) => target.staged.as_ref().map(|staged| &staged.block_hash),
        _ => None,
    }
}

#[test]
fn a_root_extension_keeps_the_target_while_its_parts_arrive() {
    let template = template_block();
    let root_0 = block(&template, 1, None, 7);
    let root_1 = block(&template, 2, Some(&root_0), 7);
    let root_2 = block(&template, 3, Some(&root_1), 7);
    let tip = block(&template, 10, None, 7);
    let mut sync = fetching_parts(&root_0, &tip, Some(pending_since(Timestamp::ZERO)));
    let now = at(Timestamp::ZERO, std::time::Duration::from_secs(60));

    best_tip_update_at(&mut sync, &root_1, now);
    best_tip_update_at(&mut sync, &root_2, now);
    assert_eq!(
        hashes(&sync.root_block_updates),
        vec![root_0.hash(), root_1.hash()]
    );
    let (target, attempts) = parts_target(&sync).expect("still fetching root 0's parts");
    assert_eq!(target, root_0.hash());
    assert!(matches!(
        attempts.as_slice(),
        [PeerStagedLedgerPartsFetchState::Pending { rpc_id: 7, .. }]
    ));
    assert!(!sync.is_kept_root_parts_request_lost(now));

    // The parts come and the staged ledger is rebuilt: the blocks after root 0
    // are applied on it, as when the root moved after the parts came.
    let target = SyncLedgerTarget::root(&root_0);
    sync.ledger = TransitionFrontierSyncLedgerState::Success {
        time: now,
        target,
        needed_protocol_states: Default::default(),
    };
    let mut state = sync_test_node(vec![]);
    state.transition_frontier.sync = TransitionFrontierSyncState::RootLedgerPending(sync);
    run(
        &mut state,
        TransitionFrontierSyncAction::LedgerRootSuccess,
        now,
    );
    run(&mut state, TransitionFrontierSyncAction::BlocksPending, now);
    let TransitionFrontierSyncState::BlocksPending { chain, .. } = &state.transition_frontier.sync
    else {
        panic!("{:?}", state.transition_frontier.sync);
    };
    let chain = chain
        .iter()
        .map(|b| (b.block_hash(), b.is_apply_success()))
        .collect::<Vec<_>>();
    assert_eq!(
        chain,
        vec![
            (root_0.hash(), true),
            (root_1.hash(), false),
            (root_2.hash(), false),
            (tip.hash(), false),
        ]
    );
}

/// Without a request that may still bring the parts, nothing is lost by
/// moving the target: as before.
#[test]
fn a_root_extension_moves_the_target_without_a_live_request() {
    let template = template_block();
    let root_0 = block(&template, 1, None, 7);
    let root_1 = block(&template, 2, Some(&root_0), 7);
    let tip = block(&template, 10, None, 7);
    let now = at(Timestamp::ZERO, STAGED_LEDGER_PARTS_STALE_AFTER);
    let failed = PeerStagedLedgerPartsFetchState::Error {
        time: Timestamp::ZERO,
        rpc_id: 7,
        error: PeerStagedLedgerPartsFetchError::DataUnavailable,
    };
    for attempt in [None, Some(failed), Some(pending_since(Timestamp::ZERO))] {
        let mut sync = fetching_parts(&root_0, &tip, attempt.clone());
        best_tip_update_at(&mut sync, &root_1, now);
        assert!(sync.root_block_updates.is_empty(), "{attempt:?}");
        assert_eq!(
            snarked_kept_target(&sync),
            Some(root_1.hash()),
            "{attempt:?}"
        );
    }
}

/// A new snarked ledger, or a root on another branch, moves the target as
/// before: the parts on the way are for a root the new one doesn't extend,
/// or one whose snarked ledger is no longer the target.
#[test]
fn a_new_snarked_ledger_or_branch_moves_the_target_while_parts_arrive() {
    let template = template_block();
    let root_0 = block(&template, 1, None, 7);
    let tip = block(&template, 10, None, 7);
    let now = at(Timestamp::ZERO, std::time::Duration::from_secs(60));

    let mut sync = fetching_parts(&root_0, &tip, Some(pending_since(Timestamp::ZERO)));
    best_tip_update_at(&mut sync, &block(&template, 2, Some(&root_0), 8), now);
    assert!(sync.root_block_updates.is_empty());
    let TransitionFrontierSyncLedgerState::Init { target, .. } = &sync.ledger else {
        panic!("expected a new root ledger sync, got {:?}", sync.ledger);
    };
    assert_eq!(target.snarked_ledger_hash, ledger_hash(8));

    let other_root = block(&template, 3, None, 7);
    let mut sync = fetching_parts(&root_0, &tip, Some(pending_since(Timestamp::ZERO)));
    best_tip_update_at(&mut sync, &other_root, now);
    assert!(sync.root_block_updates.is_empty());
    assert_eq!(snarked_kept_target(&sync), Some(other_root.hash()));
}

/// A request for a kept root's parts that fails, or goes stale, leaves the
/// current root to be targeted. A root moved back to the kept one keeps
/// asking for it.
#[test]
fn a_lost_request_for_a_kept_root_targets_the_current_root() {
    let template = template_block();
    let root_0 = block(&template, 1, None, 7);
    let root_1 = block(&template, 2, Some(&root_0), 7);
    let root_2 = block(&template, 3, Some(&root_1), 7);
    let tip = block(&template, 10, None, 7);
    let now = at(Timestamp::ZERO, std::time::Duration::from_secs(60));

    let mut sync = fetching_parts(&root_0, &tip, Some(pending_since(Timestamp::ZERO)));
    best_tip_update_at(&mut sync, &root_1, now);
    best_tip_update_at(&mut sync, &root_2, now);
    let stale = at(Timestamp::ZERO, STAGED_LEDGER_PARTS_STALE_AFTER);
    assert!(sync.is_kept_root_parts_request_lost(stale));
    sync.retarget_root_ledger(stale);
    assert!(sync.root_block_updates.is_empty());
    assert_eq!(snarked_kept_target(&sync), Some(root_2.hash()));

    let mut sync = fetching_parts(&root_0, &tip, Some(pending_since(Timestamp::ZERO)));
    best_tip_update_at(&mut sync, &root_1, now);
    best_tip_update_at(&mut sync, &root_0, now);
    assert!(sync.root_block_updates.is_empty());
    assert!(!sync.is_kept_root_parts_request_lost(stale));
}

/// The parts request a node sends: over the streaming RPC to a WebRTC peer,
/// over the RPC channel to a libp2p one. Its block, if one was sent.
fn parts_request(ran: &[String], libp2p: bool) -> Option<&str> {
    let kind = if libp2p {
        "StagedLedgerAuxAndPendingCoinbasesAtBlock"
    } else {
        "StagedLedgerParts"
    };
    ran.iter()
        .rev()
        .find(|a| a.contains("RequestSend") && a.contains(kind))
        .map(String::as_str)
}

/// A node fetching root 0's parts from one peer, with the request sent.
fn node_fetching_parts(
    libp2p: bool,
    root_0: &ArcBlockWithHash,
    tip: &ArcBlockWithHash,
) -> (crate::State, PeerId, P2pRpcId) {
    let (mut state, _) = initialized_node();
    add_peer(&mut state, near(), libp2p, tip, None);
    state.transition_frontier.sync =
        TransitionFrontierSyncState::RootLedgerPending(fetching_parts(root_0, tip, None));
    let ran = run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(0));
    let asked = parts_request(&ran, libp2p).unwrap_or_else(|| panic!("{ran:#?}"));
    assert!(asked.contains(&root_0.hash().to_string()), "{asked}");
    let rpc_id = state
        .transition_frontier
        .sync
        .ledger()
        .and_then(|l| l.staged()?.parts_fetch_rpc_id(&near()))
        .expect("a pending parts request");
    (state, near(), rpc_id)
}

/// The reducer's `BestTipUpdate`, which the effects follow. Its enabling
/// condition compares the consensus states, which the test blocks share.
fn reduce_best_tip_update(
    state: &mut crate::State,
    best_tip: &ArcBlockWithHash,
    root_block: &ArcBlockWithHash,
    now: Timestamp,
) {
    let action = TransitionFrontierSyncAction::BestTipUpdate {
        previous_root_snarked_ledger_hash: None,
        best_tip: best_tip.clone(),
        root_block: root_block.clone(),
        blocks_inbetween: vec![],
        on_success: None,
    };
    crate::reducer(
        state,
        &ActionMeta::zero_custom(now).with_action(action.into()),
        &mut Dispatcher::new(),
    );
}

fn sync_pending(state: &crate::State) -> &TransitionFrontierSyncLedgerPending {
    let TransitionFrontierSyncState::RootLedgerPending(sync) = &state.transition_frontier.sync
    else {
        panic!("{:?}", state.transition_frontier.sync);
    };
    sync
}

/// Root 0's parts are on the way from a WebRTC relay or a libp2p peer when
/// two blocks move the root to root 2. The request stays, and no new one is
/// sent. The peer then answers that it doesn't have them: root 2's parts are
/// asked for.
#[test]
fn the_parts_request_survives_root_moves_and_its_failure_targets_the_current_root() {
    for libp2p in [false, true] {
        let template = template_block();
        let root_0 = block(&template, 1, None, 7);
        let root_1 = block(&template, 2, Some(&root_0), 7);
        let root_2 = block(&template, 3, Some(&root_1), 7);
        let tips = [10, 11, 12].map(|id| block(&template, id, None, 7));
        let (mut state, peer, rpc_id) = node_fetching_parts(libp2p, &root_0, &tips[0]);

        reduce_best_tip_update(&mut state, &tips[1], &root_1, at_ms(1_000));
        reduce_best_tip_update(&mut state, &tips[2], &root_2, at_ms(2_000));
        let sync = sync_pending(&state);
        assert_eq!(sync.root_block.hash(), root_2.hash());
        assert_eq!(
            hashes(&sync.root_block_updates),
            vec![root_0.hash(), root_1.hash()]
        );
        let (target, _) = parts_target(sync).unwrap();
        assert_eq!(target, root_0.hash());
        assert_eq!(
            sync.ledger.staged().unwrap().parts_fetch_rpc_id(&peer),
            Some(rpc_id)
        );
        // The periodic check sends nothing while the request is live.
        assert!(!StagedAction::PartsPeerFetchInit.is_enabled(&state, at_ms(3_000)));

        let ran = if libp2p {
            run(
                &mut state,
                P2pChannelsRpcAction::ResponseReceived {
                    peer_id: peer,
                    id: rpc_id,
                    response: None,
                },
                at_ms(4_000),
            )
        } else {
            run(
                &mut state,
                P2pChannelsStreamingRpcAction::ResponseReceived {
                    peer_id: peer,
                    id: rpc_id,
                    response: None,
                },
                at_ms(4_000),
            )
        };
        assert!(
            ran.iter().any(|a| a.contains("LedgerRootRetarget")),
            "{ran:#?}"
        );
        let asked = parts_request(&ran, libp2p).unwrap_or_else(|| panic!("{ran:#?}"));
        assert!(asked.contains(&root_2.hash().to_string()), "{asked}");
        let sync = sync_pending(&state);
        assert!(sync.root_block_updates.is_empty());
        let (target, attempts) = parts_target(sync).unwrap();
        assert_eq!(target, root_2.hash());
        assert!(
            matches!(
                attempts.as_slice(),
                [PeerStagedLedgerPartsFetchState::Pending { .. }]
            ),
            "{attempts:?}"
        );
    }
}

/// Without a kept root, a failed request is asked again for the same root,
/// as before.
#[test]
fn a_failed_request_without_a_kept_root_asks_again() {
    for libp2p in [false, true] {
        let template = template_block();
        let root_0 = block(&template, 1, None, 7);
        let tip = block(&template, 10, None, 7);
        let (mut state, peer, rpc_id) = node_fetching_parts(libp2p, &root_0, &tip);
        let ran = run(
            &mut state,
            StagedAction::PartsPeerFetchError {
                peer_id: peer,
                rpc_id,
                error: PeerStagedLedgerPartsFetchError::DataUnavailable,
            },
            at_ms(4_000),
        );
        assert!(
            !ran.iter().any(|a| a.contains("LedgerRootRetarget")),
            "{ran:#?}"
        );
        assert!(!TransitionFrontierSyncAction::LedgerRootRetarget.is_enabled(&state, at_ms(4_000)));
        assert_eq!(parts_target(sync_pending(&state)).unwrap().0, root_0.hash());
    }
}
