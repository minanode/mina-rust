//! Completed ledger writes release their inputs while their response actions
//! still carry the data needed by sync, archive and frontier persistence.
//! Reuses the restore tests' node/store fixtures.

use std::sync::Arc;

use redux::{ActionMeta, Dispatcher};

use super::*;
use crate::{
    ledger::write::{
        BlockApplyResult, CommitResult, LedgerWriteAction, LedgerWriteKind, LedgerWriteRequest,
        LedgerWriteResponse, LedgerWriteState,
    },
    Action,
};

fn reduce(state: &mut State, action: LedgerWriteAction) -> Dispatcher<Action, State> {
    assert!(action.is_enabled(state, Timestamp::ZERO));
    let mut queued = Dispatcher::new();
    crate::reducer(
        state,
        &ActionMeta::ZERO.with_action(action.into()),
        &mut queued,
    );
    queued
}

fn start(state: &mut State, request: LedgerWriteRequest) {
    let kind = request.kind();
    reduce(
        state,
        LedgerWriteAction::Init {
            request,
            on_init: redux::callback!(
                release_test_init(_request: LedgerWriteRequest) -> Action {
                    LedgerWriteAction::Pending
                }
            ),
        },
    );
    assert_eq!(state.ledger.write.kind(), Some(kind));
    reduce(state, LedgerWriteAction::Pending);
    assert!(state.ledger.write.is_busy());
    assert_eq!(state.ledger.write.pending_requests().count(), 1);
}

fn assert_released(state: &State, kind: LedgerWriteKind) {
    assert!(matches!(
        state.ledger.write,
        LedgerWriteState::Success { .. }
    ));
    assert_eq!(state.ledger.write.kind(), Some(kind));
    assert!(!state.ledger.write.is_busy());
    assert_eq!(state.ledger.write.pending_requests().count(), 0);
    assert!(serde_json::to_vec(&state.ledger.write).unwrap().len() < 128);
    // LedgerState includes this exact write, not a second retained copy.
    assert!(serde_json::to_vec(&state.ledger).unwrap().len() < 512);
}

#[test]
fn completed_commit_releases_payload_and_forwards_result() {
    let mut state = state(false);
    let tip = state.transition_frontier.best_chain[0].clone();
    state.transition_frontier.sync = TransitionFrontierSyncState::CommitPending {
        time: Timestamp::ZERO,
        chain: vec![tip.clone()],
        root_snarked_ledger_updates: Default::default(),
        needed_protocol_states: Default::default(),
    };
    // Real captured OCaml blocks supply a large input; this tests ownership,
    // not consensus or a hash oracle. None of these history blocks is in State.
    let history = crate::persist::test_fixtures::mainnet_blocks().remove(0);
    let history_weak = Arc::downgrade(&history.block);
    let request = LedgerWriteRequest::Commit {
        root_history: crate::transition_frontier::root_history::RootHistory(vec![history; 32]),
        ledgers_to_keep: Default::default(),
        root_snarked_ledger_updates: Default::default(),
        needed_protocol_states: Default::default(),
        new_root: tip.clone(),
        new_best_tip: tip.clone(),
    };
    assert!(serde_json::to_vec(&request).unwrap().len() > 1_000_000);
    start(&mut state, request);
    assert!(history_weak.upgrade().is_some());
    let result = CommitResult {
        alive_masks: 12,
        needed_protocol_states: [tip.hash().clone()].into_iter().collect(),
        ..Default::default()
    };
    let jobs_weak = Arc::downgrade(&result.available_jobs);
    let expected = serde_json::to_value(&result).unwrap();
    let mut queued = reduce(
        &mut state,
        LedgerWriteAction::Success {
            response: LedgerWriteResponse::Commit {
                best_tip_hash: tip.hash().clone(),
                result,
            },
        },
    );
    assert_released(&state, LedgerWriteKind::Commit);
    assert_eq!(state.ledger.alive_masks, 12);
    assert!(
        history_weak.upgrade().is_none(),
        "completed request still owns history"
    );
    let Some(Action::TransitionFrontier(TransitionFrontierAction::Sync(
        followup @ TransitionFrontierSyncAction::CommitSuccess { .. },
    ))) = queued.pop()
    else {
        panic!("commit result was not forwarded");
    };
    assert!(followup.is_enabled(&state, Timestamp::ZERO));
    if let TransitionFrontierSyncAction::CommitSuccess { result } = &followup {
        assert_eq!(serde_json::to_value(result).unwrap(), expected);
    }
    assert!(
        jobs_weak.upgrade().is_some(),
        "follow-up must own commit jobs"
    );
    drop(followup);
    assert!(
        jobs_weak.upgrade().is_none(),
        "completed state still owns response"
    );
    assert!(matches!(
        queued.pop(),
        Some(Action::BlockProducer(
            crate::BlockProducerAction::StagedLedgerDiffCreateInit
        ))
    ));
    for expected in [
        Action::from(TransitionFrontierSyncAction::BlocksNextApplyInit),
        Action::from(TransitionFrontierSyncAction::CommitInit),
        Action::from(crate::transition_frontier::sync::ledger::staged::TransitionFrontierSyncLedgerStagedAction::ReconstructInit),
    ] {
        assert_eq!(serde_json::to_value(queued.pop().unwrap()).unwrap(),
            serde_json::to_value(expected).unwrap());
    }
    assert!(queued.pop().is_none());
    assert!(!LedgerWriteAction::Pending.is_enabled(&state, Timestamp::ZERO));

    // A subsequent write remains enabled after the payload has been freed.
    start(
        &mut state,
        LedgerWriteRequest::StagedLedgerReconstruct {
            snarked_ledger_hash: tip.snarked_ledger_hash().clone(),
            parts: None,
        },
    );
    assert!(!LedgerWriteAction::Success {
        response: LedgerWriteResponse::Commit {
            best_tip_hash: tip.hash().clone(),
            result: Default::default(),
        },
    }
    .is_enabled(&state, Timestamp::ZERO));
}

#[test]
fn completed_apply_forwards_archive_and_proof_flag_and_errors() {
    for failed in [false, true] {
        let mut state = state(false);
        let pred = state.transition_frontier.best_chain[0].clone();
        let block = crate::persist::test_fixtures::mainnet_blocks().remove(0);
        let expected_hash = block.hash().clone();
        start(
            &mut state,
            LedgerWriteRequest::BlockApply {
                block: block.clone(),
                pred_block: pred,
                skip_verification: false,
                restored: true,
            },
        );
        let result = BlockApplyResult {
            block,
            just_emitted_a_proof: true,
            archive_data: Some(crate::ledger::write::BlockApplyResultArchive {
                accounts_accessed: vec![],
                accounts_created: vec![],
                tokens_used: vec![],
                sender_receipt_chains_from_parent_ledger: vec![],
            }),
        };
        let expected = serde_json::to_value(&result).unwrap();
        let mut queued = reduce(
            &mut state,
            LedgerWriteAction::Success {
                response: LedgerWriteResponse::BlockApply {
                    block_hash: expected_hash.clone(),
                    result: if failed {
                        Err("apply failed".into())
                    } else {
                        Ok(result)
                    },
                },
            },
        );
        assert_released(&state, LedgerWriteKind::BlockApply);
        let Some(Action::TransitionFrontier(TransitionFrontierAction::Sync(first))) = queued.pop()
        else {
            panic!("apply result missing");
        };
        if failed {
            assert!(
                matches!(first, TransitionFrontierSyncAction::BlocksNextApplyError { hash, error }
                if hash == expected_hash && error == "apply failed")
            );
        } else {
            let TransitionFrontierSyncAction::BlocksSendToArchive { hash, data } = first else {
                panic!("archive data missing");
            };
            assert_eq!(hash, expected_hash);
            assert_eq!(serde_json::to_value(data).unwrap(), expected);
            assert!(
                matches!(queued.pop(), Some(Action::TransitionFrontier(TransitionFrontierAction::Sync(
                TransitionFrontierSyncAction::BlocksNextApplySuccess { hash, just_emitted_a_proof: true }
            ))) if hash == expected_hash)
            );
        }
    }
}

#[test]
fn completed_reconstruction_forwards_success_and_error() {
    use crate::transition_frontier::sync::{
        ledger::{staged::TransitionFrontierSyncLedgerStagedState, SyncLedgerTarget},
        TransitionFrontierSyncLedgerPending,
    };
    for failed in [false, true] {
        let mut state = state(false);
        let root = state.transition_frontier.best_chain[0].block.clone();
        let target = SyncLedgerTarget::root(&root).with_staged().unwrap();
        let hash = target.staged.hashes.non_snark.ledger_hash.clone();
        state.transition_frontier.sync =
            TransitionFrontierSyncState::RootLedgerPending(TransitionFrontierSyncLedgerPending {
                time: Timestamp::ZERO,
                best_tip: root.clone(),
                root_block: root.clone(),
                blocks_inbetween: vec![],
                root_block_updates: vec![],
                ledger: TransitionFrontierSyncLedgerStagedState::ReconstructPending {
                    time: Timestamp::ZERO,
                    target,
                    parts: None,
                }
                .into(),
            });
        start(
            &mut state,
            LedgerWriteRequest::StagedLedgerReconstruct {
                snarked_ledger_hash: root.snarked_ledger_hash().clone(),
                parts: None,
            },
        );
        let mut queued = reduce(
            &mut state,
            LedgerWriteAction::Success {
                response: LedgerWriteResponse::StagedLedgerReconstruct {
                    staged_ledger_hash: hash.clone(),
                    result: if failed {
                        Err("reconstruct failed".into())
                    } else {
                        Ok(())
                    },
                },
            },
        );
        assert_released(&state, LedgerWriteKind::StagedLedgerReconstruct);
        let first = queued.pop().unwrap();
        assert!(first.is_enabled(&state, Timestamp::ZERO));
        let Action::TransitionFrontier(TransitionFrontierAction::Sync(
            TransitionFrontierSyncAction::Ledger(crate::transition_frontier::sync::ledger::TransitionFrontierSyncLedgerAction::Staged(followup))
        )) = first else { panic!("reconstruction result missing"); };
        if failed {
            assert!(
                matches!(followup, crate::transition_frontier::sync::ledger::staged::TransitionFrontierSyncLedgerStagedAction::ReconstructError { error } if error == "reconstruct failed")
            );
        } else {
            assert!(
                matches!(followup, crate::transition_frontier::sync::ledger::staged::TransitionFrontierSyncLedgerStagedAction::ReconstructSuccess { ledger_hash } if ledger_hash == hash)
            );
        }
    }
}

#[test]
fn restored_frontier_can_checkpoint_after_releasing_completed_write() {
    // Start at the same restored frontier as the cadence/restart tests.
    let k = constants::PROTOCOL_CONSTANTS.k.as_u32();
    let (mut store, _) = replay_committed(k as usize + 23);
    assert!(store.dispatch(TransitionFrontierPersistAction::ReplayDone));
    // A completed Commit must leave the slot available to the checkpoint.
    let mut restored = store.state().clone();
    restored.ledger.write = LedgerWriteState::Success {
        time: Timestamp::ZERO,
        kind: LedgerWriteKind::Commit,
    };
    let mut store = store_of(restored);
    assert!(store.dispatch(TransitionFrontierPersistAction::ChainPutInit));
    let LedgerWriteState::Init { request, .. } = &store.state().ledger.write else {
        panic!("checkpoint not started after commit");
    };
    assert_eq!(request.kind(), LedgerWriteKind::PersistCheckpoint);
    let root = store.state().transition_frontier.best_chain[0]
        .hash()
        .clone();
    assert!(store.dispatch(LedgerWriteAction::Pending));
    let snapshot = crate::persist::event::PersistSnapshotInfo {
        staged_accounts: 7,
        staged_hashes: 11,
        ..Default::default()
    };
    let expected = serde_json::to_value(&snapshot).unwrap();
    assert!(store.dispatch(LedgerWriteAction::Success {
        response: LedgerWriteResponse::PersistCheckpoint {
            root: root.clone(),
            result: Ok(snapshot)
        }
    }));
    assert_released(store.state(), LedgerWriteKind::PersistCheckpoint);
    let ready = ready_state(&store);
    let PersistCheckpointState::Pending {
        root: actual_root,
        snapshot,
        ..
    } = ready.checkpoint
    else {
        panic!("checkpoint response not consumed");
    };
    assert_eq!(actual_root, root);
    assert_eq!(serde_json::to_value(snapshot).unwrap(), expected);

    // A response for a previous write must not complete the next write.
    let mut failed = store.state().clone();
    let request = LedgerWriteRequest::StagedLedgerReconstruct {
        snarked_ledger_hash: failed.transition_frontier.best_chain[0]
            .snarked_ledger_hash()
            .clone(),
        parts: None,
    };
    // The response kind guard rejects a late checkpoint response for a new write.
    start(&mut failed, request);
    assert!(!LedgerWriteAction::Success {
        response: LedgerWriteResponse::PersistCheckpoint {
            root,
            result: Err("late snapshot".into())
        }
    }
    .is_enabled(&failed, Timestamp::ZERO));
}

#[test]
fn completed_diff_forwards_output_without_retaining_it() {
    use crate::{
        account::AccountSecretKey,
        block_producer::{
            vrf_evaluator::VrfWonSlotWithHash, BlockProducerConfig, BlockProducerCurrentState,
            BlockProducerState, BlockProducerWonSlot,
        },
        block_producer_effectful::StagedLedgerDiffCreateOutput,
    };
    let mut state = state(false);
    let pred = state.transition_frontier.best_chain[0].clone();
    let producer = AccountSecretKey::genesis_producer().public_key();
    let won_slot = BlockProducerWonSlot::from_vrf_won_slot(
        &VrfWonSlotWithHash::new(
            mina_vrf::VrfWonSlot {
                producer: producer.clone(),
                winner_account: producer.clone(),
                global_slot: 1,
                account_index: ledger::AccountIndex(0),
                vrf_output: Box::new(mina_vrf::genesis_vrf(v2::EpochSeed::zero()).unwrap()),
                value_with_threshold: None,
            },
            pred.staking_epoch_ledger_hash().clone(),
        ),
        pred.genesis_timestamp(),
    );
    let slot = won_slot.global_slot_since_genesis(pred.global_slot_diff());
    state.block_producer = BlockProducerState::new(
        Timestamp::ZERO,
        Some(BlockProducerConfig::new(producer.clone().into())),
    );
    state.block_producer.as_mut().unwrap().current =
        BlockProducerCurrentState::StagedLedgerDiffCreatePending {
            time: Timestamp::ZERO,
            won_slot,
            chain: vec![pred.clone()],
            transactions_by_fee: vec![],
        };
    start(
        &mut state,
        LedgerWriteRequest::StagedLedgerDiffCreate {
            pred_block: pred.clone(),
            global_slot_since_genesis: slot.clone(),
            is_new_epoch: false,
            producer: producer.clone().into(),
            delegator: producer.clone().into(),
            coinbase_receiver: producer.into(),
            completed_snarks: Default::default(),
            supercharge_coinbase: false,
            transactions_by_fee: vec![],
        },
    );
    let mut pending_coinbase = ledger::scan_state::pending_coinbase::PendingCoinbase::create(
        constants::constraint_constants().pending_coinbase_depth,
    );
    pending_coinbase.merkle_root();
    let output = Arc::new(StagedLedgerDiffCreateOutput {
        diff: pred.body().staged_ledger_diff.clone(),
        diff_hash: pred
            .header()
            .protocol_state
            .body
            .blockchain_state
            .body_reference
            .clone(),
        staged_ledger_hash: pred.staged_ledger_hashes().clone(),
        emitted_ledger_proof: None,
        pending_coinbase_update: v2::MinaBasePendingCoinbaseUpdateStableV1::zero(),
        pending_coinbase_witness: v2::MinaBasePendingCoinbaseWitnessStableV2 {
            pending_coinbases: (&pending_coinbase).into(),
            is_new_stack: true,
        },
        stake_proof_sparse_ledger: v2::MinaBaseSparseLedgerBaseStableV2 {
            indexes: mina_p2p_messages::list::List::new(),
            depth: 35u64.into(),
            tree: v2::MinaBaseSparseLedgerBaseStableV2Tree::Hash(
                pred.snarked_ledger_hash().clone(),
            ),
        },
    });
    let weak = Arc::downgrade(&output);
    let expected = serde_json::to_value(&output).unwrap();
    let mut queued = reduce(
        &mut state,
        LedgerWriteAction::Success {
            response: LedgerWriteResponse::StagedLedgerDiffCreate {
                pred_block_hash: pred.hash().clone(),
                global_slot_since_genesis: slot,
                result: Ok(output),
            },
        },
    );
    assert_released(&state, LedgerWriteKind::StagedLedgerDiffCreate);
    let followup = queued.pop().unwrap();
    assert!(followup.is_enabled(&state, Timestamp::ZERO));
    let Action::BlockProducer(crate::BlockProducerAction::StagedLedgerDiffCreateSuccess { output }) =
        followup
    else {
        panic!("diff output not forwarded");
    };
    assert_eq!(serde_json::to_value(&output).unwrap(), expected);
    drop(output);
    assert!(weak.upgrade().is_none(), "completed state retains the diff");
}
