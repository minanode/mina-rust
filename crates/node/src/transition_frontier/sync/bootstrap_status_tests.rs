//! OCaml 4.0.4 Bootstrap_status and Syncable_ledger.Progress contracts.
use super::*;
use crate::{
    bootstrap_status::{BootstrapPhase, BootstrapStatus},
    transition_frontier::{
        genesis::TransitionFrontierGenesisState,
        sync::{
            ledger::{
                snarked::TransitionFrontierSyncLedgerSnarkedAction as A,
                TransitionFrontierSyncLedgerAction as L,
            },
            TransitionFrontierSyncAction as S,
        },
        TransitionFrontierAction,
    },
};

fn observe(state: &mut crate::State, action: crate::Action, ns: u64) {
    BootstrapStatus::observe(state, &action, Timestamp::new(ns));
}

#[test]
fn status_404_bootstrap_progress_retains_completed_ledgers() {
    let _ = mina_core::NetworkConfig::init("devnet");
    let root = block(&template_block(), 200, None, 7);
    let mut state = sync_test_node(vec![]);
    let now = 1_800_000_000_000_000_000;
    state.transition_frontier.genesis = TransitionFrontierGenesisState::ProveSuccess {
        time: Timestamp::new(now),
        genesis: root.clone(),
    };
    state.transition_frontier.sync = TransitionFrontierSyncState::Init {
        time: Timestamp::new(now),
        best_tip: root.clone(),
        root_block: root.clone(),
        blocks_inbetween: vec![],
    };
    observe(
        &mut state,
        S::Init {
            best_tip: root.clone(),
            root_block: root.clone(),
            blocks_inbetween: vec![],
        }
        .into(),
        now,
    );
    assert_eq!(state.transition_frontier.bootstrap.bootstrap_attempts, 1);
    assert_eq!(
        state.transition_frontier.bootstrap.bootstrap_started_at,
        Some(now)
    );
    let target = SyncLedgerTarget::root(&root);
    let pending = TransitionFrontierSyncLedgerPending {
        time: Timestamp::new(now),
        best_tip: root.clone(),
        root_block: root.clone(),
        blocks_inbetween: vec![],
        root_block_updates: vec![],
        ledger: TransitionFrontierSyncLedgerState::Snarked(
            TransitionFrontierSyncLedgerSnarkedState::pending(Timestamp::new(now), target.clone()),
        ),
    };
    state.transition_frontier.sync = TransitionFrontierSyncState::RootLedgerPending(pending);
    observe(&mut state, A::Pending.into(), now + 1);
    let snapshot = &state.transition_frontier.bootstrap;
    assert_eq!(snapshot.phase, BootstrapPhase::SyncingRootLedger);
    assert_eq!(snapshot.ledger_syncs[0].progress.targets, 1);
    assert!(snapshot.ledger_syncs[0].active);
    let sender = crate::p2p::identity::SecretKey::deterministic(1)
        .public_key()
        .peer_id();
    observe(
        &mut state,
        A::NumAccountsAccepted {
            num_accounts: 100,
            contents_hash: ledger_hash(7),
            sender,
        }
        .into(),
        now + 2,
    );
    observe(
        &mut state,
        A::ChildAccountsAccepted {
            address: crate::ledger::LedgerAddress::first(crate::ledger::LEDGER_DEPTH - 6),
            count: 64,
            sender,
        }
        .into(),
        now + 3,
    );
    observe(
        &mut state,
        A::ChildAccountsRejected {
            address: crate::ledger::LedgerAddress::first(crate::ledger::LEDGER_DEPTH - 6),
            sender,
        }
        .into(),
        now + 4,
    );
    let snapshot = &state.transition_frontier.bootstrap;
    assert_eq!(snapshot.ledger_syncs[0].progress.answers_accepted, 2);
    assert_eq!(snapshot.ledger_syncs[0].progress.answers_rejected, 1);
    assert_eq!(snapshot.ledger_syncs[0].progress.accounts_settled, 64);
    assert_eq!(
        snapshot.ledger_syncs[0].progress.last_accepted_answer_at,
        Some(now + 3)
    );
    *state.transition_frontier.sync.ledger_mut().unwrap() =
        TransitionFrontierSyncLedgerState::Snarked(
            TransitionFrontierSyncLedgerSnarkedState::Success {
                time: Timestamp::new(now + 5),
                target,
            },
        );
    observe(&mut state, A::Success.into(), now + 5);
    let snapshot = state.transition_frontier.bootstrap.clone();
    assert!(!snapshot.ledger_syncs[0].active);
    assert!(snapshot.ledger_syncs[0].progress.synced);
    assert_eq!(snapshot.ledger_syncs[0].progress.accounts_settled, 100);
    state.transition_frontier.best_chain = vec![AppliedBlock {
        block: root.clone(),
        just_emitted_a_proof: false,
    }];
    state.transition_frontier.sync = TransitionFrontierSyncState::Synced {
        time: Timestamp::new(now + 6),
    };
    observe(
        &mut state,
        TransitionFrontierAction::Synced {
            needed_protocol_states: Default::default(),
        }
        .into(),
        now + 6,
    );
    assert_eq!(
        state.transition_frontier.bootstrap.phase,
        BootstrapPhase::Participating
    );
    assert_eq!(
        serde_json::to_value(&state.transition_frontier.bootstrap.ledger_syncs).unwrap(),
        serde_json::to_value(&snapshot.ledger_syncs).unwrap()
    );
    assert_eq!(state.transition_frontier.bootstrap.bootstrap_attempts, 1);
}

#[test]
fn status_404_local_ledger_and_phase_mapping() {
    let _ = mina_core::NetworkConfig::init("devnet");
    let root = block(&template_block(), 300, None, 8);
    let now = 1_800_000_000_000_000_000;
    let mut state = sync_test_node(vec![]);
    state.transition_frontier.genesis = TransitionFrontierGenesisState::ProveSuccess {
        time: Timestamp::new(now),
        genesis: root.clone(),
    };
    let pending = reconstructing(&root, &root);
    state.transition_frontier.sync =
        TransitionFrontierSyncState::RootLedgerPending(pending.clone());
    observe(&mut state, L::Init.into(), now);
    assert_eq!(
        state.transition_frontier.bootstrap.phase,
        BootstrapPhase::BuildingStagedLedger
    );
    state.transition_frontier.sync = TransitionFrontierSyncState::StakingLedgerPending(pending);
    observe(&mut state, L::Init.into(), now + 1);
    assert_eq!(
        state.transition_frontier.bootstrap.phase,
        BootstrapPhase::SyncingEpochLedgers
    );
    let target = SyncLedgerTarget::staking_epoch(&root);
    *state.transition_frontier.sync.ledger_mut().unwrap() =
        TransitionFrontierSyncLedgerState::Snarked(
            TransitionFrontierSyncLedgerSnarkedState::Success {
                time: Timestamp::new(now + 2),
                target,
            },
        );
    observe(&mut state, L::LocalSnarkedSuccess.into(), now + 2);
    let p = &state.transition_frontier.bootstrap.ledger_syncs[0].progress;
    // OCaml's already-matching target is complete even with zero counted
    // accounts and without asking a peer for num_accounts.
    assert!(p.synced);
    assert_eq!(p.num_accounts, None);
    assert_eq!((p.accounts_settled, p.answers_accepted), (0, 0));
}
