//! Lost RPC outcomes must not strand the last sync item. These are scheduler
//! tests with synthetic accepted ledger inputs, as in SY3, not hash oracles.
use super::*;
use crate::{
    p2p::{
        channels::rpc::{
            P2pChannelsRpcAction, P2pChannelsRpcState, P2pRpcLocalState, P2pRpcRemoteState,
        },
        P2pPeerState, P2pPeerStatus, P2pPeerStatusReady, PeerId,
    },
    transition_frontier::sync::{
        ledger::{
            snarked::{
                LedgerNumAccountsQueryPending, PeerLedgerQueryResponse, PeerRpcState as LedgerRpc,
                TransitionFrontierSyncLedgerSnarkedAction as SnarkedAction,
                LEDGER_QUERY_STALE_AFTER,
            },
            staged::{
                PeerStagedLedgerPartsFetchState as PartsRpc,
                StagedLedgerAuxAndPendingCoinbasesValid,
                StagedLedgerAuxAndPendingCoinbasesValidated as Validated,
                TransitionFrontierSyncLedgerStagedAction as StagedAction,
                STAGED_LEDGER_PARTS_STALE_AFTER,
            },
            SyncLedgerTargetKind,
        },
        PeerRpcState as BlockRpc, TransitionFrontierSyncAction as SyncAction,
        TransitionFrontierSyncBlockState as BlockState,
    },
};
use redux::{ActionMeta, Dispatcher, EnablingCondition};
use std::time::Duration;

fn node(root: &ArcBlockWithHash) -> (crate::State, PeerId) {
    let mut state = sync_test_node(vec![AppliedBlock {
        block: root.clone(),
        just_emitted_a_proof: false,
    }]);
    let chain_id = serde_json::from_str(
        r#""0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886""#,
    )
    .unwrap();
    state.p2p.initialize(&chain_id).unwrap();
    let peer = PeerId::from_bytes([93; 32]);
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
    state.p2p.ready_mut().unwrap().peers.insert(
        peer,
        P2pPeerState {
            is_libp2p: true,
            dial_opts: None,
            identify: None,
            status: P2pPeerStatus::Ready(Box::new(ready)),
        },
    );
    (state, peer)
}

fn reduce(
    state: &mut crate::State,
    action: impl Into<crate::Action>,
    now: Timestamp,
) -> Vec<crate::Action> {
    let action = action.into();
    assert!(action.is_enabled(state, now), "{action:?}");
    let mut dispatcher = Dispatcher::new();
    crate::reducer(
        state,
        &ActionMeta::zero_custom(now).with_action(action),
        &mut dispatcher,
    );
    std::iter::from_fn(|| dispatcher.pop()).collect()
}

fn set_ledger(
    state: &mut crate::State,
    root: &ArcBlockWithHash,
    kind: SyncLedgerTargetKind,
    ledger: TransitionFrontierSyncLedgerState,
) {
    let pending = TransitionFrontierSyncLedgerPending {
        time: Timestamp::ZERO,
        best_tip: root.clone(),
        root_block: root.clone(),
        blocks_inbetween: vec![],
        root_block_updates: vec![],
        ledger,
    };
    state.transition_frontier.sync = match kind {
        SyncLedgerTargetKind::StakingEpoch => {
            TransitionFrontierSyncState::StakingLedgerPending(pending)
        }
        SyncLedgerTargetKind::NextEpoch => {
            TransitionFrontierSyncState::NextEpochLedgerPending(pending)
        }
        SyncLedgerTargetKind::Root => TransitionFrontierSyncState::RootLedgerPending(pending),
    };
}

fn wake(state: &mut crate::State, now: Timestamp, scheduler: impl Into<crate::Action>) {
    let kind = scheduler.into().kind();
    assert!(reduce(state, crate::CheckTimeoutsAction {}, now)
        .iter()
        .any(|a| a.kind() == kind));
}

#[test]
fn num_accounts_without_an_outcome_is_retried_once_stale() {
    let root = block(&template_block(), 100, None, 7);
    let start = Timestamp::ZERO + Duration::from_secs(1000);
    let stale = start + LEDGER_QUERY_STALE_AFTER;
    for kind in [
        SyncLedgerTargetKind::StakingEpoch,
        SyncLedgerTargetKind::NextEpoch,
        SyncLedgerTargetKind::Root,
    ] {
        for attempt in [
            LedgerRpc::Init { time: start },
            LedgerRpc::Pending {
                time: start,
                rpc_id: 1,
            },
            LedgerRpc::Success {
                time: start,
                rpc_id: 1,
            },
        ] {
            let (mut state, peer_id) = node(&root);
            let target = SyncLedgerTarget {
                kind,
                snarked_ledger_hash: root.snarked_ledger_hash().clone(),
                staged: None,
            };
            set_ledger(
                &mut state,
                &root,
                kind,
                TransitionFrontierSyncLedgerSnarkedState::NumAccountsPending {
                    time: start,
                    target,
                    pending_num_accounts: LedgerNumAccountsQueryPending {
                        attempts: [(peer_id, attempt)].into(),
                    },
                }
                .into(),
            );
            let retry = SnarkedAction::PeerQueryNumAccountsRetry { peer_id };
            assert!(!retry.is_enabled(&state, Timestamp::ZERO));
            assert!(!retry.is_enabled(&state, Timestamp::new(u64::from(stale) - 1)));
            assert!(reduce(
                &mut state,
                SnarkedAction::PeersQuery,
                Timestamp::new(u64::from(stale) - 1)
            )
            .is_empty());
            wake(&mut state, stale, SnarkedAction::PeersQuery);
            let scheduled = reduce(&mut state, SnarkedAction::PeersQuery, stale);
            assert_eq!(scheduled.len(), 1);
            assert!(matches!(&scheduled[0], crate::Action::TransitionFrontier(crate::TransitionFrontierAction::Sync(SyncAction::Ledger(crate::transition_frontier::sync::ledger::TransitionFrontierSyncLedgerAction::Snarked(SnarkedAction::PeerQueryNumAccountsRetry { .. }))))));
            reduce(&mut state, retry.clone(), stale);
            assert!(!retry.is_enabled(&state, stale));
            assert!(!retry.is_enabled(
                &state,
                Timestamp::new(u64::from(stale + LEDGER_QUERY_STALE_AFTER) - 1)
            ));
            reduce(
                &mut state,
                SnarkedAction::PeerQueryNumAccountsPending { peer_id, rpc_id: 2 },
                stale,
            );
            let reply = |rpc_id| SnarkedAction::PeerQueryNumAccountsSuccess {
                peer_id,
                rpc_id,
                response: PeerLedgerQueryResponse::NumAccounts(
                    1,
                    root.snarked_ledger_hash().clone(),
                ),
            };
            assert!(!reply(1).is_enabled(&state, stale));
            reduce(&mut state, reply(2), stale);
            assert!(!reply(2).is_enabled(&state, stale));
            // Accepted reducer inputs: this test checks scheduling/completion,
            // while NumAccountsReceived owns the cryptographic validation.
            reduce(
                &mut state,
                SnarkedAction::NumAccountsAccepted {
                    num_accounts: 1,
                    contents_hash: root.snarked_ledger_hash().clone(),
                    sender: peer_id,
                },
                stale,
            );
            reduce(
                &mut state,
                SnarkedAction::NumAccountsSuccess {
                    num_accounts: 1,
                    contents_hash: root.snarked_ledger_hash().clone(),
                },
                stale,
            );
            reduce(&mut state, SnarkedAction::MerkleTreeSyncPending, stale);
            let (address, expected_hash) = state
                .transition_frontier
                .sync
                .ledger()
                .unwrap()
                .snarked()
                .unwrap()
                .sync_address_query_iter()
                .next()
                .unwrap();
            reduce(
                &mut state,
                SnarkedAction::PeerQueryAddressInit {
                    peer_id,
                    address: address.clone(),
                    expected_hash,
                },
                stale,
            );
            reduce(
                &mut state,
                SnarkedAction::PeerQueryAddressPending {
                    peer_id,
                    address: address.clone(),
                    rpc_id: 3,
                },
                stale,
            );
            reduce(
                &mut state,
                SnarkedAction::PeerQueryAddressSuccess {
                    peer_id,
                    rpc_id: 3,
                    response: PeerLedgerQueryResponse::ChildAccounts(vec![]),
                },
                stale,
            );
            reduce(
                &mut state,
                SnarkedAction::ChildAccountsAccepted {
                    address,
                    count: 1,
                    sender: peer_id,
                },
                stale,
            );
            reduce(&mut state, SnarkedAction::MerkleTreeSyncSuccess, stale);
            reduce(&mut state, SnarkedAction::Success, stale);
            assert!(matches!(
                state.transition_frontier.sync.ledger().unwrap().snarked(),
                Some(TransitionFrontierSyncLedgerSnarkedState::Success { .. })
            ));
            assert!(!reply(1).is_enabled(&state, stale));
        }
    }
}

#[test]
fn staged_parts_without_an_outcome_are_retried_once_stale() {
    use ledger::{
        scan_state::{pending_coinbase::PendingCoinbase, scan_state::ScanState},
        staged_ledger::hash::StagedLedgerHash,
    };
    let constants = mina_core::constants::constraint_constants();
    // Synthetic self-consistent input parts, not an expected hash oracle.
    let scan_state = ScanState::empty(constants);
    let mut pending_coinbase = PendingCoinbase::create(constants.pending_coinbase_depth);
    let root = block(&template_block(), 100, None, 7);
    let hash = StagedLedgerHash::of_aux_ledger_and_coinbase_hash(
        scan_state.hash(),
        root.snarked_ledger_hash().to_field().unwrap(),
        &mut pending_coinbase,
    );
    let valid = Arc::new(StagedLedgerAuxAndPendingCoinbasesValid {
        scan_state,
        pending_coinbase,
        staged_ledger_hash: root.snarked_ledger_hash().clone(),
        needed_blocks: List::new(),
    });
    let wire = Arc::new(valid.to_wire());
    let mut target = SyncLedgerTarget::root(&root).with_staged().unwrap();
    target.staged.hashes = (&hash).into();
    let start = Timestamp::ZERO + Duration::from_secs(1000);
    let stale = start + STAGED_LEDGER_PARTS_STALE_AFTER;
    for attempt in [
        PartsRpc::Pending {
            time: start,
            rpc_id: 1,
        },
        PartsRpc::Success {
            time: start,
            parts: Validated::Valid(valid.clone()),
        },
        PartsRpc::Success {
            time: start,
            parts: Validated::Invalid(wire.clone()),
        },
        PartsRpc::Valid {
            time: start,
            parts: valid.clone(),
        },
    ] {
        let (mut state, peer_id) = node(&root);
        set_ledger(
            &mut state,
            &root,
            SyncLedgerTargetKind::Root,
            TransitionFrontierSyncLedgerStagedState::PartsFetchPending {
                time: start,
                target: target.clone(),
                attempts: [(peer_id, attempt)].into(),
            }
            .into(),
        );
        assert!(!StagedAction::PartsPeerFetchInit.is_enabled(&state, Timestamp::ZERO));
        assert!(!StagedAction::PartsPeerFetchInit
            .is_enabled(&state, Timestamp::new(u64::from(stale) - 1)));
        // A normal heavy RPC has 180 s to answer; do not race it.
        assert!(
            !StagedAction::PartsPeerFetchInit.is_enabled(&state, start + Duration::from_secs(180))
        );
        wake(&mut state, stale, StagedAction::PartsPeerFetchInit);
        let scheduled = reduce(&mut state, StagedAction::PartsPeerFetchInit, stale);
        assert_eq!(scheduled.len(), 1);
        assert!(
            matches!(&scheduled[0], crate::Action::P2p(crate::p2p::P2pAction::Channels(crate::p2p::channels::P2pChannelsAction::Rpc(P2pChannelsRpcAction::RequestSend { peer_id: id, .. }))) if *id == peer_id)
        );
        reduce(
            &mut state,
            StagedAction::PartsPeerFetchPending { peer_id, rpc_id: 2 },
            stale,
        );
        assert!(!StagedAction::PartsPeerFetchInit.is_enabled(&state, stale));
        assert!(!StagedAction::PartsPeerFetchInit.is_enabled(
            &state,
            Timestamp::new(u64::from(stale + STAGED_LEDGER_PARTS_STALE_AFTER) - 1)
        ));
        let reply = |rpc_id| StagedAction::PartsPeerFetchSuccess {
            peer_id,
            rpc_id,
            parts: wire.clone(),
        };
        assert!(!reply(1).is_enabled(&state, stale));
        reduce(&mut state, reply(2), stale);
        assert!(!reply(2).is_enabled(&state, stale));
        reduce(
            &mut state,
            StagedAction::PartsPeerValid { sender: peer_id },
            stale,
        );
        reduce(
            &mut state,
            StagedAction::PartsFetchSuccess { sender: peer_id },
            stale,
        );
        assert!(!StagedAction::PartsFetchSuccess { sender: peer_id }.is_enabled(&state, stale));
        reduce(&mut state, StagedAction::ReconstructPending, stale);
        reduce(
            &mut state,
            StagedAction::ReconstructSuccess {
                ledger_hash: target.staged.hashes.non_snark.ledger_hash.clone(),
            },
            stale,
        );
        reduce(&mut state, StagedAction::Success, stale);
        assert!(state
            .transition_frontier
            .sync
            .ledger()
            .unwrap()
            .staged()
            .unwrap()
            .is_success());
        assert!(!reply(1).is_enabled(&state, stale));
    }
}

#[test]
fn block_fetch_without_an_outcome_is_retried_once_stale() {
    let template = template_block();
    let root = block(&template, 100, None, 7);
    let missing = block(&template, 101, Some(&root), 7);
    let tip = block(&template, 102, Some(&missing), 7);
    let start = Timestamp::ZERO + Duration::from_secs(1000);
    let stale = start + LEDGER_QUERY_STALE_AFTER;
    for attempt in [
        BlockRpc::Init { time: start },
        BlockRpc::Pending {
            time: start,
            rpc_id: 1,
        },
        BlockRpc::Success {
            time: start,
            block: missing.clone(),
        },
    ] {
        let (mut state, peer_id) = node(&tip);
        state.transition_frontier.sync = TransitionFrontierSyncState::BlocksPending {
            time: start,
            chain: vec![
                BlockState::ApplySuccess {
                    time: start,
                    block: AppliedBlock {
                        block: root.clone(),
                        just_emitted_a_proof: false,
                    },
                },
                BlockState::FetchPending {
                    time: start,
                    block_hash: missing.hash().clone(),
                    attempts: [(peer_id, attempt)].into(),
                },
                BlockState::FetchSuccess {
                    time: start,
                    block: tip.clone(),
                },
            ],
            root_snarked_ledger_updates: Default::default(),
            needed_protocol_states: Default::default(),
        };
        let retry = SyncAction::BlocksPeerQueryRetry {
            peer_id,
            hash: missing.hash().clone(),
        };
        assert!(!retry.is_enabled(&state, Timestamp::ZERO));
        assert!(
            !SyncAction::BlocksPeersQuery.is_enabled(&state, Timestamp::new(u64::from(stale) - 1))
        );
        assert!(!retry.is_enabled(&state, Timestamp::new(u64::from(stale) - 1)));
        wake(&mut state, stale, SyncAction::BlocksPeersQuery);
        assert!(SyncAction::BlocksPeersQuery.is_enabled(&state, stale));
        assert_eq!(
            state
                .transition_frontier
                .sync
                .blocks_fetch_retry_iter(stale)
                .collect::<Vec<_>>(),
            vec![missing.hash().clone()]
        );
        reduce(&mut state, retry.clone(), stale);
        assert!(!retry.is_enabled(&state, stale));
        assert!(!retry.is_enabled(
            &state,
            Timestamp::new(u64::from(stale + LEDGER_QUERY_STALE_AFTER) - 1)
        ));
        assert!(!SyncAction::BlocksPeersQuery.is_enabled(&state, stale));
        reduce(
            &mut state,
            SyncAction::BlocksPeerQueryPending {
                peer_id,
                hash: missing.hash().clone(),
                rpc_id: 2,
            },
            stale,
        );
        let reply = |rpc_id| SyncAction::BlocksPeerQuerySuccess {
            peer_id,
            rpc_id,
            response: missing.clone(),
        };
        assert!(!reply(1).is_enabled(&state, stale));
        reduce(&mut state, reply(2), stale);
        assert!(!reply(2).is_enabled(&state, stale));
        reduce(
            &mut state,
            SyncAction::BlocksFetchSuccess {
                hash: missing.hash().clone(),
            },
            stale,
        );
        for b in [&missing, &tip] {
            reduce(
                &mut state,
                SyncAction::BlocksNextApplyPending {
                    hash: b.hash().clone(),
                },
                stale,
            );
            reduce(
                &mut state,
                SyncAction::BlocksNextApplySuccess {
                    hash: b.hash().clone(),
                    just_emitted_a_proof: false,
                },
                stale,
            );
        }
        reduce(&mut state, SyncAction::BlocksSuccess, stale);
        assert!(matches!(
            state.transition_frontier.sync,
            TransitionFrontierSyncState::BlocksSuccess { .. }
        ));
        assert!(!reply(1).is_enabled(&state, stale));
    }
}

/// A consumed answer can free the peer's RPC slot while its old block attempt
/// is still Pending. An error for a newer request must terminate that request.
#[test]
fn block_error_after_slot_reuse_marks_only_the_matching_attempt() {
    use crate::transition_frontier::sync::PeerBlockFetchError;
    let template = template_block();
    let root = block(&template, 100, None, 7);
    let first = block(&template, 101, Some(&root), 7);
    let second = block(&template, 102, Some(&first), 7);
    let (mut state, peer_id) = node(&second);
    let now = Timestamp::ZERO + Duration::from_secs(1000);
    state.transition_frontier.sync = TransitionFrontierSyncState::BlocksPending {
        time: now,
        chain: [&first, &second]
            .into_iter()
            .enumerate()
            .map(|(i, block)| BlockState::FetchPending {
                time: now,
                block_hash: block.hash().clone(),
                attempts: [(
                    peer_id,
                    BlockRpc::Pending {
                        time: now,
                        rpc_id: i as u64 + 1,
                    },
                )]
                .into(),
            })
            .collect(),
        root_snarked_ledger_updates: Default::default(),
        needed_protocol_states: Default::default(),
    };
    let error = |rpc_id| SyncAction::BlocksPeerQueryError {
        peer_id,
        rpc_id,
        error: PeerBlockFetchError::DataUnavailable,
    };
    reduce(&mut state, error(2), now);
    assert!(state
        .transition_frontier
        .sync
        .block_state(first.hash())
        .unwrap()
        .is_fetch_pending_from_peer(&peer_id, 1));
    assert_eq!(
        state
            .transition_frontier
            .sync
            .blocks_fetch_retry_iter(now)
            .collect::<Vec<_>>(),
        vec![second.hash().clone()]
    );
    assert!(!error(2).is_enabled(&state, now));
}
