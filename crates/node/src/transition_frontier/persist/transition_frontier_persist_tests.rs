//! The checkpoint cadence (R20 D4, D8): when a checkpoint is due.

use mina_p2p_messages::v2::StateHash;
use redux::Timestamp;

use super::*;
use crate::persist::event::{PersistSnapshotInfo, PersistStoreSummary};

const SEC: u64 = 1_000_000_000;

fn ready(has_checkpoint: bool) -> PersistReadyState {
    PersistReadyState {
        opened_at: Timestamp::new(0),
        dir: String::new(),
        open_outcome: "empty".into(),
        store: PersistStoreSummary {
            checkpoint_root: has_checkpoint.then(StateHash::zero),
            ..Default::default()
        },
        chain_put_sent: None,
        chain_puts_pending: 0,
        checkpoint: PersistCheckpointState::Idle,
        last_checkpoint: None,
        retry_after: None,
        stats: Default::default(),
        restore: PersistRestoreState::Idle,
        checkpoint_chain_len_limit: None,
    }
}

fn at(secs: u64) -> Timestamp {
    Timestamp::new(secs * SEC)
}

fn checkpointed(state: &mut PersistReadyState, secs: u64) {
    state.last_checkpoint = Some(PersistLastCheckpoint {
        time: at(secs),
        root: StateHash::zero(),
        root_height: 1,
        generation: 1,
    });
    state.store.checkpoint_root = Some(StateHash::zero());
}

#[test]
fn a_store_without_a_checkpoint_needs_one_at_once() {
    assert!(ready(false).is_checkpoint_due(at(1), 1800));
}

#[test]
fn the_first_chance_in_a_run_takes_a_checkpoint_even_if_the_store_has_one() {
    // Until I5d2 every run bootstraps, so the stored checkpoint is of an
    // older chain than the one being followed.
    assert!(ready(true).is_checkpoint_due(at(1), 1800));
}

#[test]
fn then_one_every_interval() {
    let mut state = ready(false);
    checkpointed(&mut state, 100);
    assert!(!state.is_checkpoint_due(at(100), 1800));
    assert!(!state.is_checkpoint_due(at(100 + 1799), 1800));
    assert!(state.is_checkpoint_due(at(100 + 1800), 1800));
    assert!(state.is_checkpoint_due(at(100 + 5000), 1800));
    // A clock behind the last checkpoint never makes one due.
    assert!(!state.is_checkpoint_due(at(50), 1800));
}

#[test]
fn a_chain_put_that_drops_the_checkpoint_makes_one_due() {
    let mut state = ready(false);
    checkpointed(&mut state, 100);
    // After a bootstrap the chain's root is not on the stored chain: the
    // store thread reports a summary without a checkpoint.
    state.store.checkpoint_root = None;
    assert!(state.is_checkpoint_due(at(101), 1800));
}

#[test]
fn none_while_one_is_being_taken_or_after_a_failure_until_the_retry() {
    let mut state = ready(false);
    state.checkpoint = PersistCheckpointState::Init {
        time: at(1),
        root: StateHash::zero(),
    };
    assert!(!state.is_checkpoint_due(at(2), 1800));
    state.checkpoint = PersistCheckpointState::Pending {
        time: at(1),
        root: StateHash::zero(),
        snapshot: Box::new(PersistSnapshotInfo::default()),
    };
    assert!(!state.is_checkpoint_due(at(2), 1800));
    state.checkpoint = PersistCheckpointState::Idle;
    state.retry_after = Some(at(1 + CHECKPOINT_RETRY_SECS));
    assert!(!state.is_checkpoint_due(at(2), 1800));
    assert!(state.is_checkpoint_due(at(1 + CHECKPOINT_RETRY_SECS), 1800));
}

#[test]
fn persistence_is_off_unless_configured() {
    let config = TransitionFrontierPersistConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.checkpoint_interval_secs, 30 * 60);
    assert!(matches!(
        TransitionFrontierPersistState::new(&config),
        TransitionFrontierPersistState::Disabled { .. }
    ));
    let on = TransitionFrontierPersistConfig {
        enabled: true,
        ..config
    };
    assert!(matches!(
        TransitionFrontierPersistState::new(&on),
        TransitionFrontierPersistState::Idle
    ));
}

#[test]
fn after_a_restore_a_long_stored_chain_needs_a_checkpoint_at_once() {
    // I5d3: a restore counts as a checkpoint, but if the stored chain is
    // longer than its limit (k + 1 and a few blocks), the next checkpoint is
    // due at once rather than an interval later.
    let mut state = ready(true);
    checkpointed(&mut state, 100);
    state.checkpoint_chain_len_limit = Some(296);
    state.store.chain_len = 296;
    assert!(!state.is_checkpoint_due(at(101), 1800));
    state.store.chain_len = 297;
    assert!(state.is_checkpoint_due(at(101), 1800));
    // Not while one is being taken, nor before a failed one's retry.
    state.retry_after = Some(at(200));
    assert!(!state.is_checkpoint_due(at(101), 1800));
    state.retry_after = None;
    state.checkpoint = PersistCheckpointState::Init {
        time: at(101),
        root: StateHash::zero(),
    };
    assert!(!state.is_checkpoint_due(at(102), 1800));
    // Without the limit (a run that didn't restore, or after the first
    // checkpoint), the chain's length doesn't matter: the interval does.
    state.checkpoint = PersistCheckpointState::Idle;
    state.checkpoint_chain_len_limit = None;
    assert!(!state.is_checkpoint_due(at(101), 1800));
    assert!(state.is_checkpoint_due(at(100 + 1800), 1800));
}
