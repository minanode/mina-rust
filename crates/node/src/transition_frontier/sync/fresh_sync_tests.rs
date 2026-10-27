use super::*;
use crate::{
    p2p::{
        channels::rpc::{P2pChannelsRpcState, P2pRpcLocalState, P2pRpcRemoteState},
        P2pPeerState, P2pPeerStatus, P2pPeerStatusReady, PeerId,
    },
    transition_frontier::sync::{
        PeerBlockFetchError, PeerRpcState, SyncPhase, TransitionFrontierSyncAction as SyncAction,
        TransitionFrontierSyncBlockState as BlockState,
    },
};
use redux::{ActionMeta, Dispatcher, EnablingCondition};
use std::time::Duration;

fn add_peers(state: &mut crate::State, tip: &ArcBlockWithHash) -> [PeerId; 2] {
    let chain_id = serde_json::from_str(
        r#""0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886""#,
    )
    .unwrap();
    state.p2p.initialize(&chain_id).unwrap();
    let ids = [PeerId::from_bytes([51; 32]), PeerId::from_bytes([52; 32])];
    for id in ids {
        let mut ready = P2pPeerStatusReady::new(false, Timestamp::ZERO, &Default::default());
        ready.best_tip = Some(tip.clone());
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
        state.p2p.ready_mut().unwrap().peers.insert(
            id,
            P2pPeerState {
                is_libp2p: true,
                dial_opts: None,
                identify: None,
                status: P2pPeerStatus::Ready(Box::new(ready)),
            },
        );
    }
    ids
}

fn reduce(
    state: &mut crate::State,
    action: impl Into<crate::Action>,
    now: Timestamp,
) -> Dispatcher<crate::Action, crate::State> {
    let mut dispatcher = Dispatcher::new();
    crate::reducer(
        state,
        &ActionMeta::zero_custom(now).with_action(action.into()),
        &mut dispatcher,
    );
    dispatcher
}

#[test]
fn block_refusals_back_off_across_hashes_fail_over_and_wake() {
    let template = template_block();
    let root = block(&template, 100, None, 7);
    let first = block(&template, 101, Some(&root), 7);
    let second = block(&template, 102, Some(&first), 7);
    let tip = block(&template, 103, Some(&second), 7);
    let mut state = sync_test_node(vec![AppliedBlock {
        block: root.clone(),
        just_emitted_a_proof: false,
    }]);
    let [bad, good] = add_peers(&mut state, &tip);
    let mut stale_hint = root.clone();
    Arc::make_mut(&mut stale_hint.block)
        .header
        .protocol_state
        .body
        .consensus_state
        .blockchain_length = 1u32.into();
    state
        .p2p
        .ready_mut()
        .unwrap()
        .get_ready_peer_mut(&good)
        .unwrap()
        .best_tip = Some(stale_hint);
    state.transition_frontier.sync = TransitionFrontierSyncState::BlocksPending {
        time: Timestamp::ZERO,
        chain: vec![
            BlockState::ApplySuccess {
                time: Timestamp::ZERO,
                block: AppliedBlock {
                    block: root,
                    just_emitted_a_proof: false,
                },
            },
            BlockState::FetchPending {
                time: Timestamp::ZERO,
                block_hash: first.hash().clone(),
                attempts: [(
                    bad,
                    PeerRpcState::Pending {
                        time: Timestamp::ZERO,
                        rpc_id: 7,
                    },
                )]
                .into(),
            },
            BlockState::FetchPending {
                time: Timestamp::ZERO,
                block_hash: second.hash().clone(),
                attempts: Default::default(),
            },
            BlockState::FetchSuccess {
                time: Timestamp::ZERO,
                block: tip,
            },
        ],
        root_snarked_ledger_updates: Default::default(),
        needed_protocol_states: Default::default(),
    };
    let now = Timestamp::ZERO + Duration::from_secs(1);
    reduce(
        &mut state,
        SyncAction::BlocksPeerQueryError {
            peer_id: bad,
            rpc_id: 7,
            error: PeerBlockFetchError::DataUnavailable,
        },
        now,
    );
    let retry = |peer_id| SyncAction::BlocksPeerQueryRetry {
        hash: first.hash().clone(),
        peer_id,
    };
    assert!(
        !retry(bad).is_enabled(&state, now),
        "a refusal must not immediately retry"
    );
    assert!(
        retry(good).is_enabled(&state, now),
        "another peer with a stale genesis hint must remain eligible"
    );
    assert!(
        !SyncAction::BlocksPeerQueryInit {
            hash: second.hash().clone(),
            peer_id: bad
        }
        .is_enabled(&state, now),
        "another block must not bypass the peer delay"
    );
    state.p2p.ready_mut().unwrap().peers.remove(&good);
    assert!(!SyncAction::BlocksPeersQuery.is_enabled(&state, now));
    assert!(!retry(bad).is_enabled(&state, now + Duration::from_millis(4999)));
    let later = now + Duration::from_secs(5);
    let mut wake = reduce(&mut state, crate::CheckTimeoutsAction {}, later);
    let mut found = false;
    while let Some(action) = wake.pop() {
        found |= matches!(
            action,
            crate::Action::TransitionFrontier(crate::TransitionFrontierAction::Sync(
                SyncAction::BlocksPeersQuery
            ))
        );
    }
    assert!(
        found,
        "the timer must wake the block scheduler without a network event"
    );
    assert!(
        retry(bad).is_enabled(&state, later),
        "a sole peer must recover after the bounded delay"
    );
    // A quiet sole peer repeatedly refuses for a minute. Sampling much more
    // frequently than the delay must still permit only twelve attempts.
    let mut attempts = 1u64;
    for millis in (0..60_000).step_by(100) {
        let time = now + Duration::from_millis(millis);
        if retry(bad).is_enabled(&state, time) {
            attempts += 1;
            reduce(&mut state, retry(bad), time);
            reduce(
                &mut state,
                SyncAction::BlocksPeerQueryPending {
                    hash: first.hash().clone(),
                    peer_id: bad,
                    rpc_id: 100 + attempts,
                },
                time,
            );
            reduce(
                &mut state,
                SyncAction::BlocksPeerQueryError {
                    peer_id: bad,
                    rpc_id: 100 + attempts,
                    error: PeerBlockFetchError::DataUnavailable,
                },
                time,
            );
        }
    }
    assert_eq!(attempts, 12);
    let later = now + Duration::from_secs(60);
    assert!(retry(bad).is_enabled(&state, later));
    reduce(&mut state, retry(bad), later);
    reduce(
        &mut state,
        SyncAction::BlocksPeerQueryPending {
            hash: first.hash().clone(),
            peer_id: bad,
            rpc_id: 8,
        },
        later,
    );
    reduce(
        &mut state,
        SyncAction::BlocksPeerQuerySuccess {
            peer_id: bad,
            rpc_id: 8,
            response: first.clone(),
        },
        later,
    );
    assert!(SyncAction::BlocksFetchSuccess {
        hash: first.hash().clone()
    }
    .is_enabled(&state, later));
}

#[test]
fn genesis_hint_refresh_is_periodic_budgeted_and_stops_with_a_target() {
    let template = template_block();
    let mut genesis = block(&template, 100, None, 7);
    // A synthetic height-one block represents a custom genesis.
    Arc::make_mut(&mut genesis.block)
        .header
        .protocol_state
        .body
        .consensus_state
        .blockchain_length = 1u32.into();
    assert!(genesis.is_genesis());
    let mut state = sync_test_node(vec![AppliedBlock {
        block: genesis.clone(),
        just_emitted_a_proof: false,
    }]);
    let [peer, other] = add_peers(&mut state, &genesis);
    state.p2p.ready_mut().unwrap().peers.remove(&other);
    let requests = |state: &crate::State| crate::effects::bootstrap_best_tip_requests(state);
    assert_eq!(requests(&state).len(), 1);
    state
        .p2p
        .ready_mut()
        .unwrap()
        .best_tip_requests
        .record(peer, None, Timestamp::ZERO);
    let set_time = |state: &mut crate::State, seconds| {
        reduce(
            state,
            crate::CheckTimeoutsAction {},
            Timestamp::ZERO + Duration::from_secs(seconds),
        );
    };
    set_time(&mut state, 19);
    assert!(requests(&state).is_empty());
    set_time(&mut state, 20);
    assert_eq!(
        requests(&state).len(),
        1,
        "a successful genesis answer must not end discovery"
    );
    let mut sent = 1;
    for sec in 20..300 {
        set_time(&mut state, sec);
        if !requests(&state).is_empty() {
            let now = state.time();
            state
                .p2p
                .ready_mut()
                .unwrap()
                .best_tip_requests
                .record(peer, None, now);
            sent += 1;
        }
    }
    assert_eq!(
        sent, 12,
        "refresh shares the existing five-minute request budget"
    );
    set_time(&mut state, 300);
    assert_eq!(requests(&state).len(), 1, "exhausted budget must reopen");
    let tip = block(&template, 101, Some(&genesis), 7);
    state
        .p2p
        .ready_mut()
        .unwrap()
        .get_ready_peer_mut(&peer)
        .unwrap()
        .best_tip = Some(tip.clone());
    let window = Duration::from_millis(
        u64::from(tip.constants().k.as_u32())
            * mina_core::constants::constraint_constants().block_window_duration_ms,
    );
    reduce(
        &mut state,
        crate::CheckTimeoutsAction {},
        tip.timestamp() + window,
    );
    assert!(
        requests(&state).is_empty(),
        "a recent non-genesis hint needs no bootstrap refresh"
    );
    reduce(
        &mut state,
        crate::CheckTimeoutsAction {},
        tip.timestamp() + window + Duration::from_secs(1),
    );
    assert_eq!(
        requests(&state).len(),
        1,
        "a stale non-genesis hint can be refreshed without a target"
    );
    state.transition_frontier.sync = TransitionFrontierSyncState::Init {
        time: state.time(),
        best_tip: tip,
        root_block: genesis,
        blocks_inbetween: vec![],
    };
    assert!(
        requests(&state).is_empty(),
        "a useful target stops bootstrap polling"
    );
}

#[test]
fn established_genesis_waits_but_a_recent_custom_genesis_is_synced() {
    let template = template_block();
    let mut genesis = block(&template, 100, None, 7);
    Arc::make_mut(&mut genesis.block)
        .header
        .protocol_state
        .body
        .consensus_state
        .blockchain_length = 1u32.into();
    assert!(genesis.is_genesis());
    let ours = [AppliedBlock {
        block: genesis.clone(),
        just_emitted_a_proof: false,
    }];
    let recent = genesis.timestamp();
    let phase = |now| {
        super::super::reported_sync_phase(
            SyncPhase::Synced,
            &ours,
            2,
            true,
            [&genesis, &genesis],
            None,
            now,
        )
    };
    assert_eq!(phase(recent), SyncPhase::Synced);
    let window = Duration::from_millis(
        u64::from(genesis.constants().k.as_u32())
            * mina_core::constants::constraint_constants().block_window_duration_ms,
    );
    assert_eq!(phase(recent + window), SyncPhase::Synced);
    assert_eq!(
        phase(recent + window + Duration::from_secs(1)),
        SyncPhase::Listening
    );
}

#[test]
fn future_peer_and_custom_genesis_timestamps_do_not_overflow() {
    let template = template_block();
    let mut genesis = block(&template, 100, None, 7);
    Arc::make_mut(&mut genesis.block)
        .header
        .protocol_state
        .body
        .consensus_state
        .blockchain_length = 1u32.into();
    let mut state = sync_test_node(vec![AppliedBlock {
        block: genesis.clone(),
        just_emitted_a_proof: false,
    }]);
    let [peer, other] = add_peers(&mut state, &genesis);
    state.p2p.ready_mut().unwrap().peers.remove(&other);
    let mut future = block(&template, 101, Some(&genesis), 7);
    Arc::make_mut(&mut future.block)
        .header
        .protocol_state
        .body
        .blockchain_state
        .timestamp = mina_p2p_messages::v2::BlockTimeTimeStableV1(u64::MAX.into());
    state
        .p2p
        .ready_mut()
        .unwrap()
        .get_ready_peer_mut(&peer)
        .unwrap()
        .best_tip = Some(future);
    let now = Timestamp::new(1_000_000_000_000_000_000);
    reduce(&mut state, crate::CheckTimeoutsAction {}, now);
    assert!(
        crate::effects::bootstrap_best_tip_requests(&state).is_empty(),
        "a future peer timestamp must not wrap into a stale hint"
    );
    Arc::make_mut(&mut genesis.block)
        .header
        .protocol_state
        .body
        .blockchain_state
        .timestamp = mina_p2p_messages::v2::BlockTimeTimeStableV1(u64::MAX.into());
    let ours = [AppliedBlock {
        block: genesis.clone(),
        just_emitted_a_proof: false,
    }];
    assert_eq!(
        super::super::reported_sync_phase(SyncPhase::Synced, &ours, 2, true, [&genesis], None, now),
        SyncPhase::Synced
    );
}

/// BP-SAFE: a fetched block whose body this node can't check, because a
/// cached proof can't be read back here, doesn't back its sole peer off. The
/// block is asked for again at once and accepted. After a bound, a failure
/// counts as the peer's, so the fetch can't loop without a delay.
#[test]
fn a_local_proof_read_failure_does_not_back_off_a_sole_peer() {
    use crate::transition_frontier::LOCAL_BLOCK_CHECK_RETRIES;
    use mina_core::block::prevalidate::BlockPrevalidationError;

    let local =
        PeerBlockFetchError::from_block_check(BlockPrevalidationError::BodyProofUnavailable {
            error: "No such file or directory".to_owned(),
        });
    assert!(matches!(
        local,
        PeerBlockFetchError::LocalProofUnavailable(_)
    ));
    assert!(matches!(
        PeerBlockFetchError::from_block_check(BlockPrevalidationError::InvalidBodyReference {
            expected: String::new(),
            got: String::new(),
        }),
        PeerBlockFetchError::Invalid(_)
    ));

    let template = template_block();
    let root = block(&template, 100, None, 7);
    let first = block(&template, 101, Some(&root), 7);
    let tip = block(&template, 102, Some(&first), 7);
    let mut state = sync_test_node(vec![AppliedBlock {
        block: root.clone(),
        just_emitted_a_proof: false,
    }]);
    let [peer, other] = add_peers(&mut state, &tip);
    state.p2p.ready_mut().unwrap().peers.remove(&other);
    state.transition_frontier.sync = TransitionFrontierSyncState::BlocksPending {
        time: Timestamp::ZERO,
        chain: vec![
            BlockState::ApplySuccess {
                time: Timestamp::ZERO,
                block: AppliedBlock {
                    block: root,
                    just_emitted_a_proof: false,
                },
            },
            BlockState::FetchPending {
                time: Timestamp::ZERO,
                block_hash: first.hash().clone(),
                attempts: [(
                    peer,
                    PeerRpcState::Pending {
                        time: Timestamp::ZERO,
                        rpc_id: 7,
                    },
                )]
                .into(),
            },
            BlockState::FetchSuccess {
                time: Timestamp::ZERO,
                block: tip,
            },
        ],
        root_snarked_ledger_updates: Default::default(),
        needed_protocol_states: Default::default(),
    };
    let retry = SyncAction::BlocksPeerQueryRetry {
        hash: first.hash().clone(),
        peer_id: peer,
    };
    let now = Timestamp::ZERO + Duration::from_secs(1);
    let fail = |state: &mut crate::State, rpc_id| {
        reduce(
            state,
            SyncAction::BlocksPeerQueryError {
                peer_id: peer,
                rpc_id,
                error: local.clone(),
            },
            now,
        );
    };
    let ask_again = |state: &mut crate::State, rpc_id| {
        reduce(state, retry.clone(), now);
        reduce(
            state,
            SyncAction::BlocksPeerQueryPending {
                hash: first.hash().clone(),
                peer_id: peer,
                rpc_id,
            },
            now,
        );
    };

    // The read fails once: the peer is asked again at once, then the block
    // is accepted in the same instant.
    fail(&mut state, 7);
    assert!(state.transition_frontier.block_peer_query_ready(&peer, now));
    assert!(state.transition_frontier.block_peer_query_errors.is_empty());
    assert!(SyncAction::BlocksPeersQuery.is_enabled(&state, now));
    assert!(retry.is_enabled(&state, now));
    ask_again(&mut state, 8);
    reduce(
        &mut state,
        SyncAction::BlocksPeerQuerySuccess {
            peer_id: peer,
            rpc_id: 8,
            response: first.clone(),
        },
        now,
    );
    assert!(SyncAction::BlocksFetchSuccess {
        hash: first.hash().clone()
    }
    .is_enabled(&state, now));
    assert!(state
        .transition_frontier
        .block_local_check_errors
        .is_empty());

    // A read that keeps failing is retried at once only a bounded number of
    // times; then the peer is backed off as for any failed fetch.
    let BlockState::FetchPending { attempts, .. } = state
        .transition_frontier
        .sync
        .block_state_mut(first.hash())
        .unwrap()
    else {
        panic!("the block is still fetched in this state");
    };
    *attempts = [(
        peer,
        PeerRpcState::Pending {
            time: now,
            rpc_id: 20,
        },
    )]
    .into();
    let mut rpc_id = 20;
    for _ in 0..LOCAL_BLOCK_CHECK_RETRIES {
        fail(&mut state, rpc_id);
        assert!(retry.is_enabled(&state, now));
        rpc_id += 1;
        ask_again(&mut state, rpc_id);
    }
    fail(&mut state, rpc_id);
    assert!(!state.transition_frontier.block_peer_query_ready(&peer, now));
    assert!(!retry.is_enabled(&state, now));
}
