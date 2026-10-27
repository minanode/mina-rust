//! The restore from the frontier store in the state machine (I5d1, R20 D5):
//! the load once the store is open, its outcomes and timeouts, the replay
//! the sync takes over with the sync held meanwhile, a replay that fails, and
//! the best tip proof rule. The reducers run for real; the effects only record
//! what the node asks the store thread for.

#[path = "ledger_write_release_tests.rs"]
mod ledger_write_release_tests;

use std::collections::BTreeMap;

use mina_core::{
    block::{AppliedBlock, ArcBlockWithHash},
    consensus::ConsensusConstants,
    constants,
};
use mina_p2p_messages::v2;
use mina_snark::{BlockVerifier, TransactionVerifier};
use redux::{EnablingCondition, Timestamp};

use super::*;
use crate::{
    persist::{
        event::{PersistOpened, PersistRestored, PersistStoreSummary},
        test_fixtures::{block_after, synthetic_chain, ChainLedgers},
    },
    transition_frontier::{
        genesis::TransitionFrontierGenesisState,
        persist_effectful::TransitionFrontierPersistEffectfulAction,
        sync::{
            SyncError, TransitionFrontierSyncAction, TransitionFrontierSyncBlockState,
            TransitionFrontierSyncState,
        },
        TransitionFrontierAction,
    },
    State,
};

struct Service {
    now: redux::Instant,
    started: redux::Instant,
    /// What the node asked the store thread for.
    asked: Vec<TransitionFrontierPersistEffectfulAction>,
    reads: Vec<crate::ledger::read::LedgerReadRequest>,
}

impl redux::Service for Service {}

impl redux::TimeService for Service {
    fn monotonic_time(&mut self) -> redux::Instant {
        self.now
    }
}

type Store = crate::Store<Service>;

fn effects(store: &mut Store, action: crate::ActionWithMeta) {
    if let crate::Action::TransitionFrontier(TransitionFrontierAction::PersistEffect(asked)) =
        action.action()
    {
        store.service.asked.push(asked.clone());
    }
    if let crate::Action::LedgerEffects(
        crate::ledger_effectful::LedgerEffectfulAction::ReadInit { request, .. },
    ) = action.action()
    {
        store.service.reads.push(request.clone());
    }
}

/// A node at its genesis block, synced there, its frontier store opening.
fn state(restore: bool) -> State {
    let consensus_constants = ConsensusConstants::create(
        constants::constraint_constants(),
        &constants::PROTOCOL_CONSTANTS,
    );
    let mut transition_frontier =
        crate::config::TransitionFrontierConfig::new(crate::config::DEVNET_CONFIG.clone());
    transition_frontier.persist = TransitionFrontierPersistConfig {
        enabled: true,
        checkpoint_interval_secs: DEFAULT_CHECKPOINT_INTERVAL_SECS,
        restore,
        load_timeout_secs: LOAD_TIMEOUT_SECS,
    };
    let config = crate::Config {
        ledger: crate::config::LedgerConfig {},
        snark: crate::config::SnarkConfig {
            block_verifier_index: BlockVerifier::make(),
            block_verifier_srs: mina_snark::get_srs(),
            work_verifier_index: TransactionVerifier::make(),
            work_verifier_srs: mina_snark::get_srs(),
        },
        p2p: crate::config::P2pConfig {
            libp2p_port: None,
            listen_port: None,
            identity_pub_key: crate::p2p::identity::SecretKey::deterministic(0).public_key(),
            initial_peers: vec![],
            external_addrs: vec![],
            enabled_channels: Default::default(),
            timeouts: Default::default(),
            limits: Default::default(),
            peer_discovery: false,
            meshsub: Default::default(),
            catchup_rpc: Default::default(),
        },
        transition_frontier,
        archive: None,
        block_producer: None,
        global: crate::config::GlobalConfig {
            build: Box::new(crate::config::BuildEnv::get()),
            snarker: None,
            consensus_constants: consensus_constants.clone(),
            client_port: None,
            testing_run: true,
            chain_id_override: None,
            skip_proof_verification: false,
            worker_control: false,
            snarker_strategy: Default::default(),
            block_first_lead: None,
        },
        tx_pool: ledger::transaction_pool::Config {
            trust_system: (),
            pool_max_size: 10,
            slot_tx_end: None,
        },
    };
    let mut state = State::new(config, &consensus_constants, Timestamp::ZERO);
    let tf = &mut state.transition_frontier;
    tf.genesis = TransitionFrontierGenesisState::produced_for_tests();
    let genesis = tf.genesis.block_with_real_or_dummy_proof().unwrap();
    assert!(genesis.is_genesis());
    tf.best_chain = vec![AppliedBlock {
        block: genesis,
        just_emitted_a_proof: false,
    }];
    tf.sync = TransitionFrontierSyncState::Synced {
        time: Timestamp::ZERO,
    };
    // The store opens once the chain id is known (`OpenInit`).
    assert!(matches!(tf.persist, TransitionFrontierPersistState::Idle));
    let chain_id = serde_json::from_str(
        r#""0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886""#,
    )
    .unwrap();
    state.p2p.initialize(&chain_id).unwrap();
    state
}

fn store_of(state: State) -> Store {
    let now = redux::Instant::now();
    redux::Store::new(
        crate::reducer,
        effects,
        Service {
            now,
            started: now,
            asked: Vec::new(),
            reads: Vec::new(),
        },
        redux::SystemTime::UNIX_EPOCH,
        state,
    )
}

/// A store whose node asked the store thread to open the store.
fn opening(restore: bool) -> Store {
    let mut store = store_of(state(restore));
    assert!(store.dispatch(TransitionFrontierPersistAction::OpenInit));
    assert!(matches!(
        store.service.asked.as_slice(),
        [TransitionFrontierPersistEffectfulAction::Open { .. }]
    ));
    store.service.asked.clear();
    store
}

/// Moves the clock to `secs` after the store was made. Enabling conditions
/// see the time of the last action, so a `CheckTimeouts` (as the node's main
/// loop dispatches) brings it there.
fn at(store: &mut Store, secs: u64) {
    store.service.now = store.service.started + std::time::Duration::from_secs(secs);
    assert!(store.dispatch(crate::action::CheckTimeoutsAction {}));
}

fn opened() -> TransitionFrontierPersistAction {
    TransitionFrontierPersistAction::OpenSuccess {
        opened: Box::new(PersistOpened {
            dir: "memory".into(),
            outcome: "opened generation 2".into(),
            deleted: 0,
            store: PersistStoreSummary {
                generation: 2,
                checkpoint_root: Some(v2::StateHash::zero()),
                ..Default::default()
            },
            elapsed_ms: 1.0,
        }),
    }
}

fn restore_state(store: &Store) -> PersistRestoreState {
    store
        .state()
        .transition_frontier
        .persist
        .restore()
        .cloned()
        .expect("the store is open")
}

fn holds_sync(store: &Store) -> bool {
    store.state().transition_frontier.persist_holds_sync()
}

fn ledger_hash(n: u64) -> v2::LedgerHash {
    v2::LedgerHash::from_fp(n.into())
}

/// A stored chain: a root and `after` blocks, whose snarked ledger moves on
/// at the block `moves_at` (counted from the root).
fn stored_chain(after: usize, moves_at: usize) -> Vec<ArcBlockWithHash> {
    let ledgers = |snarked| ChainLedgers {
        staking: ledger_hash(1),
        next: ledger_hash(2),
        snarked: ledger_hash(snarked),
        staged: v2::MinaBaseStagedLedgerHashStableV1::zero(
            ledger_hash(3),
            v2::PendingCoinbaseHash::from_fp(4u64.into()),
        ),
    };
    let mut chain = synthetic_chain(v2::StateHash::from_fp(9u64.into()), moves_at, &ledgers(10));
    while chain.len() < after + 1 {
        let parent = chain.last().unwrap().clone();
        chain.push(block_after(
            parent.hash().clone(),
            parent.height() + 1,
            parent.global_slot() + 1,
            &ledgers(11),
        ));
    }
    assert_eq!(chain.len(), after + 1);
    chain
}

fn restored(chain: &[ArcBlockWithHash]) -> TransitionFrontierPersistAction {
    TransitionFrontierPersistAction::LoadSuccess {
        restored: Box::new(PersistRestored {
            generation: 2,
            attempt: 1,
            root: chain[0].clone(),
            blocks: chain[1..].to_vec(),
            needed_protocol_states: BTreeMap::new(),
            ledgers: vec![ledger_hash(1), ledger_hash(10)],
            timings: Default::default(),
        }),
    }
}

fn best_tip_update(
    tip: &ArcBlockWithHash,
    root: &ArcBlockWithHash,
) -> TransitionFrontierSyncAction {
    TransitionFrontierSyncAction::BestTipUpdate {
        previous_root_snarked_ledger_hash: None,
        best_tip: tip.clone(),
        root_block: root.clone(),
        blocks_inbetween: vec![],
        on_success: None,
    }
}

#[test]
fn the_store_is_loaded_once_open_and_its_chain_replayed_through_the_sync() {
    let mut store = opening(true);
    // Until the restore is decided, the sync takes no target.
    assert!(holds_sync(&store));
    let k = constants::PROTOCOL_CONSTANTS.k.as_u32();

    at(&mut store, 1);
    assert!(store.dispatch(opened()));
    // The load was asked for at once: 4k slots, our genesis block.
    let Some(TransitionFrontierPersistEffectfulAction::Load { request }) =
        store.service.asked.first().cloned()
    else {
        panic!("no load: {:?}", store.service.asked);
    };
    assert_eq!(request.stale_slots, 4 * k);
    assert_eq!(
        request.genesis.as_ref().map(|genesis| genesis.hash()),
        store
            .state()
            .genesis_block()
            .as_ref()
            .map(|genesis| genesis.hash())
    );
    assert!(request.current_slot.is_some());
    assert!(matches!(
        restore_state(&store),
        PersistRestoreState::LoadPending { .. }
    ));
    assert!(holds_sync(&store));
    // Not twice.
    assert!(!store.dispatch(TransitionFrontierPersistAction::LoadInit));

    // A stored chain of k + 12 blocks: the replay commits k + 1 of them,
    // the root moving on by 11 blocks, across a snarked ledger change.
    let chain = stored_chain(k as usize + 11, 5);
    let tip = chain.last().unwrap().clone();
    at(&mut store, 9);
    assert!(store.dispatch(restored(&chain)));
    let PersistRestoreState::Replaying { info, .. } = restore_state(&store) else {
        panic!("not replaying");
    };
    assert_eq!(info.blocks, k + 11);
    assert_eq!(&info.tip, tip.hash());
    let tf = &store.state().transition_frontier;
    // The best chain stays the genesis block's until the commit.
    assert_eq!(tf.best_chain.len(), 1);
    assert!(tf.root().unwrap().is_genesis());
    let TransitionFrontierSyncState::BlocksPending {
        chain: sync_chain,
        root_snarked_ledger_updates,
        ..
    } = &tf.sync
    else {
        panic!("not replaying through the sync: {:?}", tf.sync);
    };
    assert_eq!(sync_chain.len(), chain.len());
    assert!(matches!(
        &sync_chain[0],
        TransitionFrontierSyncBlockState::ApplySuccess { block, .. }
            if block.hash() == chain[0].hash() && !block.just_emitted_a_proof
    ));
    assert!(sync_chain[1..].iter().zip(&chain[1..]).all(|(state, block)| matches!(
        state,
        TransitionFrontierSyncBlockState::FetchSuccess { block: b, .. } if b.hash() == block.hash()
    )));
    // The new root (11 blocks on) has the next snarked ledger: the commit
    // builds it from the stored root's, with the staged ledger hash of the
    // block that first had it.
    assert_eq!(root_snarked_ledger_updates.len(), 1);
    let update = root_snarked_ledger_updates.get(&ledger_hash(11)).unwrap();
    assert_eq!(update.parent, ledger_hash(10));
    assert_eq!(&update.staged_ledger_hash, chain[5].staged_ledger_hashes());

    // While replaying, a network best tip doesn't change the sync's target.
    assert!(holds_sync(&store));
    let time = store.state().time();
    assert!(!best_tip_update(&tip, &chain[11]).is_enabled(store.state(), time));
    // Nor is the restore done before the commit.
    assert!(!store.dispatch(TransitionFrontierPersistAction::ReplayDone));
    // Nor a second replay.
    assert!(!store.dispatch(TransitionFrontierSyncAction::RestoreInit {
        root: chain[0].clone(),
        blocks: chain[1..].to_vec(),
        needed_protocol_states: BTreeMap::new(),
    }));
}

/// A store whose chain is no longer than k + 1: nothing moves the root.
#[test]
fn a_short_stored_chain_keeps_its_root() {
    let mut store = opening(true);
    assert!(store.dispatch(opened()));
    let chain = stored_chain(4, 2);
    assert!(store.dispatch(restored(&chain)));
    let TransitionFrontierSyncState::BlocksPending {
        chain: sync_chain,
        root_snarked_ledger_updates,
        ..
    } = &store.state().transition_frontier.sync
    else {
        panic!("not replaying");
    };
    assert_eq!(sync_chain.len(), 5);
    assert!(root_snarked_ledger_updates.is_empty());
}

#[test]
fn a_restored_block_that_fails_drops_the_store_and_is_not_blacklisted() {
    let chain = stored_chain(3, 1);
    let mut state = state(true);
    let tf = &mut state.transition_frontier;
    let mut ready = PersistReadyState {
        opened_at: Timestamp::ZERO,
        dir: "memory".into(),
        open_outcome: "opened generation 2".into(),
        store: PersistStoreSummary {
            generation: 2,
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
    };
    let failing = chain[2].clone();
    let sync_with_error = TransitionFrontierSyncState::BlocksPending {
        time: Timestamp::ZERO,
        chain: vec![
            TransitionFrontierSyncBlockState::ApplySuccess {
                time: Timestamp::ZERO,
                block: AppliedBlock {
                    block: chain[0].clone(),
                    just_emitted_a_proof: false,
                },
            },
            TransitionFrontierSyncBlockState::ApplySuccess {
                time: Timestamp::ZERO,
                block: AppliedBlock {
                    block: chain[1].clone(),
                    just_emitted_a_proof: false,
                },
            },
            TransitionFrontierSyncBlockState::ApplyError {
                time: Timestamp::ZERO,
                block: failing.clone(),
                error: "staged ledger hash mismatch".into(),
            },
            TransitionFrontierSyncBlockState::FetchSuccess {
                time: Timestamp::ZERO,
                block: chain[3].clone(),
            },
        ],
        root_snarked_ledger_updates: Default::default(),
        needed_protocol_states: Default::default(),
    };
    tf.sync = sync_with_error.clone();
    ready.restore = PersistRestoreState::Replaying {
        time: Timestamp::ZERO,
        info: Box::new(PersistRestoreInfo {
            generation: 2,
            attempt: 1,
            root: chain[0].hash().clone(),
            root_height: chain[0].height(),
            tip: chain[3].hash().clone(),
            tip_height: chain[3].height(),
            blocks: 3,
            ledgers: 2,
            load_started: Timestamp::ZERO,
            timings: Default::default(),
        }),
    };
    tf.persist = TransitionFrontierPersistState::Ready(Box::new(ready));
    let control = state.clone();

    // The store's block: not blacklisted, the sync back at the genesis
    // block, the store dropped and the sync free to bootstrap.
    let mut store = store_of(state);
    assert!(store.dispatch(TransitionFrontierAction::SyncFailed {
        best_tip: chain[3].clone(),
        error: SyncError::RestoredBlockApplyFailed(failing.clone(), "mismatch".into()),
    }));
    assert!(
        store.dispatch(TransitionFrontierPersistAction::ReplayFailed {
            error: "mismatch".into()
        })
    );
    let tf = &store.state().transition_frontier;
    assert!(tf.blacklist.is_empty());
    assert!(tf.sync.is_synced());
    assert!(tf.root().unwrap().is_genesis());
    assert!(matches!(
        restore_state(&store),
        PersistRestoreState::Done {
            outcome: PersistRestoreOutcome::Failed { .. },
            ..
        }
    ));
    assert!(!holds_sync(&store));
    assert!(matches!(
        store.service.asked.as_slice(),
        [TransitionFrontierPersistEffectfulAction::Discard { .. }]
    ));

    // A peer's block that fails the same way is blacklisted.
    let mut store = store_of(control);
    assert!(store.dispatch(TransitionFrontierAction::SyncFailed {
        best_tip: chain[3].clone(),
        error: SyncError::BlockApplyFailed(failing.clone(), "mismatch".into()),
    }));
    assert!(store
        .state()
        .transition_frontier
        .blacklist
        .contains_key(failing.hash()));
}

/// A node whose replay of a stored chain of a root and `after` blocks has
/// just been committed: its best chain is the last k + 1 blocks, synced, the
/// restore not yet done.
fn replay_committed(after: usize) -> (Store, Vec<ArcBlockWithHash>) {
    let k = constants::PROTOCOL_CONSTANTS.k.as_u32() as usize;
    let chain = stored_chain(after, 2);
    let mut state = state(true);
    let tf = &mut state.transition_frontier;
    tf.best_chain = chain[chain.len().saturating_sub(k + 1)..]
        .iter()
        .map(|block| AppliedBlock {
            block: block.clone(),
            just_emitted_a_proof: false,
        })
        .collect();
    tf.sync = TransitionFrontierSyncState::Synced {
        time: Timestamp::ZERO,
    };
    let tip = chain.last().unwrap();
    tf.persist = TransitionFrontierPersistState::Ready(Box::new(PersistReadyState {
        opened_at: Timestamp::ZERO,
        dir: "memory".into(),
        open_outcome: "opened generation 2".into(),
        store: PersistStoreSummary {
            generation: 2,
            checkpoint_root: Some(chain[0].hash().clone()),
            chain_len: chain.len() as u32,
            tip: Some(tip.hash().clone()),
            ..Default::default()
        },
        chain_put_sent: None,
        chain_puts_pending: 0,
        checkpoint: PersistCheckpointState::Idle,
        last_checkpoint: None,
        retry_after: None,
        stats: Default::default(),
        restore: PersistRestoreState::Replaying {
            time: Timestamp::ZERO,
            info: Box::new(PersistRestoreInfo {
                generation: 2,
                attempt: 1,
                root: chain[0].hash().clone(),
                root_height: chain[0].height(),
                tip: tip.hash().clone(),
                tip_height: tip.height(),
                blocks: after as u32,
                ledgers: 2,
                load_started: Timestamp::ZERO,
                timings: Default::default(),
            }),
        },
        checkpoint_chain_len_limit: None,
    }));
    (store_of(state), chain)
}

fn ready_state(store: &Store) -> PersistReadyState {
    store
        .state()
        .transition_frontier
        .persist
        .ready()
        .cloned()
        .expect("the store is open")
}

/// I5d3: a node restarted more often than the checkpoint interval would
/// replay a longer chain each time (I5d1's runs: 291 to 313 blocks). After a
/// restore whose stored chain is more than k + 1 and a few blocks, the
/// checkpoint is taken as soon as the node is synced and its chain handed
/// to the store, and then on the usual schedule.
#[test]
fn a_restore_of_a_long_chain_takes_a_checkpoint_once_synced() {
    let k = constants::PROTOCOL_CONSTANTS.k.as_u32();
    let (mut store, chain) = replay_committed(k as usize + 23);
    assert!(store.dispatch(TransitionFrontierPersistAction::ReplayDone));
    let ready = ready_state(&store);
    assert_eq!(
        ready.checkpoint_chain_len_limit,
        Some(k + 1 + RESTORE_CHECKPOINT_SLACK_BLOCKS)
    );
    assert!(matches!(ready.checkpoint, PersistCheckpointState::Idle));
    // The stored checkpoint still counts as this run's for the interval.
    assert_eq!(
        ready.last_checkpoint.as_ref().map(|last| &last.root),
        Some(chain[0].hash())
    );

    // Not before the chain is handed to the store thread, so that the
    // checkpoint's blocks are stored.
    assert!(!store.dispatch(TransitionFrontierPersistAction::CheckpointInit));
    assert!(store.dispatch(TransitionFrontierPersistAction::ChainPutInit));
    let ready = ready_state(&store);
    let root = &store.state().transition_frontier.best_chain[0];
    assert!(
        matches!(&ready.checkpoint, PersistCheckpointState::Init { root: r, .. } if r == root.hash()),
        "no checkpoint: {:?}",
        ready.checkpoint
    );
    assert_eq!(root.hash(), chain[23].hash());

    // Once it is written, the next one is an interval away.
    let root_hash = root.hash().clone();
    assert!(
        store.dispatch(TransitionFrontierPersistAction::CheckpointPending {
            root: root_hash.clone(),
            snapshot: Default::default(),
        })
    );
    assert!(
        store.dispatch(TransitionFrontierPersistAction::CheckpointSuccess {
            done: Box::new(crate::persist::event::PersistCheckpointDone {
                root: root_hash.clone(),
                root_height: chain[23].height(),
                generation: 3,
                written: Default::default(),
                store: PersistStoreSummary {
                    generation: 3,
                    checkpoint_root: Some(root_hash),
                    chain_len: k + 1,
                    ..Default::default()
                },
                parts_bytes: 0,
                parts_encode_ms: 0.0,
                stream_parts: 0,
                stream_ms: 0.0,
                stream_max_ms: 0.0,
                elapsed_ms: 0.0,
                snapshot: Default::default(),
            }),
        })
    );
    let ready = ready_state(&store);
    assert_eq!(ready.checkpoint_chain_len_limit, None);
    assert_eq!(ready.last_checkpoint.as_ref().unwrap().generation, 3);
    // (The ledger write slot stays taken here, with no ledger thread to
    // answer the first checkpoint's snapshot, so ask the cadence itself.)
    let due = |store: &mut Store, secs| {
        at(store, secs);
        ready_state(store).is_checkpoint_due(store.state().time(), DEFAULT_CHECKPOINT_INTERVAL_SECS)
    };
    assert!(!due(&mut store, 60));
    assert!(due(&mut store, DEFAULT_CHECKPOINT_INTERVAL_SECS + 60));
}

/// A stored chain within a few blocks of k + 1 (a quick restart) keeps its
/// checkpoint until the chain grows past the limit or the interval passes.
#[test]
fn a_restore_of_a_short_chain_keeps_its_checkpoint() {
    let k = constants::PROTOCOL_CONSTANTS.k.as_u32();
    let (mut store, _) = replay_committed((k + RESTORE_CHECKPOINT_SLACK_BLOCKS) as usize);
    assert!(store.dispatch(TransitionFrontierPersistAction::ReplayDone));
    assert!(store.dispatch(TransitionFrontierPersistAction::ChainPutInit));
    let ready = ready_state(&store);
    assert!(matches!(ready.checkpoint, PersistCheckpointState::Idle));
    assert!(!store.dispatch(TransitionFrontierPersistAction::CheckpointInit));
    // The network moves on: once the stored chain passes the limit, the
    // checkpoint is due.
    let mut longer = ready.store.clone();
    longer.chain_len += 1;
    assert!(
        store.dispatch(TransitionFrontierPersistAction::ChainPutSuccess {
            done: Box::new(crate::persist::event::PersistChainPutDone {
                tip: ready.chain_put_sent.clone().unwrap(),
                generation: 2,
                checkpoint_invalidated: false,
                written: Default::default(),
                store: longer,
                elapsed_ms: 0.0,
            }),
        })
    );
    assert!(store.dispatch(TransitionFrontierPersistAction::CheckpointInit));
}

#[test]
fn each_load_outcome_but_a_restore_lets_the_node_bootstrap() {
    let outcomes = [
        TransitionFrontierPersistAction::LoadAbsent {
            reason: "the store is empty".into(),
        },
        TransitionFrontierPersistAction::LoadLedgersOnly {
            reason: "the stored tip is 2000 slots behind the clock".into(),
            ledgers: 3,
        },
        TransitionFrontierPersistAction::LoadDiscarded {
            reason: "3 restores didn't finish".into(),
        },
        TransitionFrontierPersistAction::LoadError {
            error: "I/O error".into(),
        },
    ];
    for outcome in outcomes {
        let mut store = opening(true);
        assert!(store.dispatch(opened()));
        assert!(holds_sync(&store));
        // A load that fell back to an older generation says what the store
        // now holds, before its outcome (I5e).
        let older = PersistStoreSummary {
            generation: 1,
            ..Default::default()
        };
        assert!(
            store.dispatch(TransitionFrontierPersistAction::StoreSummary {
                store: Box::new(older.clone()),
            })
        );
        let ready = store.state().transition_frontier.persist.ready().unwrap();
        assert_eq!(ready.store, older);
        assert!(store.dispatch(outcome.clone()), "{outcome:?}");
        let PersistRestoreState::Done { outcome: done, .. } = restore_state(&store) else {
            panic!("not done: {outcome:?}");
        };
        match (&outcome, &done) {
            (
                TransitionFrontierPersistAction::LoadAbsent { .. },
                PersistRestoreOutcome::Absent { .. },
            )
            | (
                TransitionFrontierPersistAction::LoadLedgersOnly { .. },
                PersistRestoreOutcome::LedgersOnly { ledgers: 3, .. },
            )
            | (
                TransitionFrontierPersistAction::LoadDiscarded { .. },
                PersistRestoreOutcome::Discarded { .. },
            )
            | (
                TransitionFrontierPersistAction::LoadError { .. },
                PersistRestoreOutcome::Failed { .. },
            ) => {}
            other => panic!("{other:?}"),
        }
        assert!(!holds_sync(&store), "{outcome:?}");
        // A discarded store is gone from the node's view too.
        if let PersistRestoreOutcome::Discarded { .. } = done {
            let ready = store.state().transition_frontier.persist.ready().unwrap();
            assert_eq!(ready.store.generation, 0);
        }
        // Nothing else arrives for this load.
        assert!(!store.dispatch(restored(&stored_chain(2, 1))));
    }
}

#[test]
fn a_load_or_an_open_that_takes_a_minute_is_given_up() {
    let mut store = opening(true);
    at(&mut store, 10);
    assert!(store.dispatch(opened()));
    at(&mut store, 10 + LOAD_TIMEOUT_SECS - 1);
    assert!(!store.dispatch(TransitionFrontierPersistAction::LoadTimeout));
    assert!(holds_sync(&store));
    // (A second later: the load was asked for a few actions, so a few
    // nanoseconds, after the open.)
    at(&mut store, 10 + LOAD_TIMEOUT_SECS + 1);
    assert!(store.dispatch(TransitionFrontierPersistAction::LoadTimeout));
    assert!(store.dispatch(TransitionFrontierPersistAction::LoadCancelPending));
    assert!(holds_sync(&store));
    assert!(!store.dispatch(restored(&stored_chain(2, 1))));
    assert!(
        !store.dispatch(TransitionFrontierPersistAction::LoadProgress {
            completed_stages: 10
        })
    );
    assert!(store.dispatch(TransitionFrontierPersistAction::LoadCancelled));
    assert!(!store.dispatch(TransitionFrontierPersistAction::LoadCancelled));
    assert!(matches!(
        restore_state(&store),
        PersistRestoreState::Done {
            outcome: PersistRestoreOutcome::TimedOut,
            ..
        }
    ));
    assert!(!holds_sync(&store));
    assert!(matches!(
        store.service.asked.last(),
        Some(TransitionFrontierPersistEffectfulAction::LoadCancel)
    ));
    // The load's answer, if it comes after all, is not taken.
    assert!(!store.dispatch(restored(&stored_chain(2, 1))));

    // The store thread doesn't open the store within a minute: persistence
    // is off for the run, and the sync is free.
    let mut store = opening(true);
    at(&mut store, LOAD_TIMEOUT_SECS - 1);
    assert!(!store.dispatch(TransitionFrontierPersistAction::OpenTimeout));
    assert!(holds_sync(&store));
    at(&mut store, LOAD_TIMEOUT_SECS + 1);
    assert!(store.dispatch(TransitionFrontierPersistAction::OpenTimeout));
    assert!(matches!(
        store.state().transition_frontier.persist,
        TransitionFrontierPersistState::Disabled { .. }
    ));
    assert!(!holds_sync(&store));
    assert!(!store.dispatch(opened()));
}

#[test]
fn progress_renews_the_idle_deadline_without_a_wall_clock_limit() {
    let mut store = opening(true);
    at(&mut store, 10);
    assert!(store.dispatch(opened()));
    // Ten minutes of real progress exceeds the old five-minute hard cap.
    for completed in 1..=12 {
        at(&mut store, 10 + completed * 50);
        assert!(
            store.dispatch(TransitionFrontierPersistAction::LoadProgress {
                completed_stages: completed
            })
        );
        assert!(!store.dispatch(TransitionFrontierPersistAction::LoadTimeout));
        // Duplicate or regressing counters cannot renew the deadline.
        assert!(
            !store.dispatch(TransitionFrontierPersistAction::LoadProgress {
                completed_stages: completed
            })
        );
        assert!(
            !store.dispatch(TransitionFrontierPersistAction::LoadProgress {
                completed_stages: completed - 1
            })
        );
    }
    at(&mut store, 610 + LOAD_TIMEOUT_SECS - 1);
    assert!(!store.dispatch(TransitionFrontierPersistAction::LoadTimeout));
    at(&mut store, 610 + LOAD_TIMEOUT_SECS + 1);
    assert!(store.dispatch(TransitionFrontierPersistAction::LoadTimeout));
    assert!(store.dispatch(TransitionFrontierPersistAction::LoadCancelPending));
    assert!(holds_sync(&store));
    assert!(store.dispatch(TransitionFrontierPersistAction::LoadCancelled));
    assert!(!holds_sync(&store));
}

#[test]
fn progress_can_finish_a_restore_after_the_old_hard_deadline() {
    let mut store = opening(true);
    at(&mut store, 10);
    assert!(store.dispatch(opened()));
    for completed in 1..=12 {
        at(&mut store, 10 + completed * 50);
        assert!(
            store.dispatch(TransitionFrontierPersistAction::LoadProgress {
                completed_stages: completed
            })
        );
    }
    assert!(store.dispatch(restored(&stored_chain(2, 1))));
    assert!(matches!(
        restore_state(&store),
        PersistRestoreState::Replaying { .. }
    ));
}

#[test]
fn direct_callers_cannot_disable_the_idle_deadline_with_an_oversized_wait() {
    let mut state = state(true);
    state.transition_frontier.config.persist.load_timeout_secs = u64::MAX;
    let mut store = store_of(state);
    assert!(store.dispatch(TransitionFrontierPersistAction::OpenInit));
    at(&mut store, 10);
    assert!(store.dispatch(opened()));
    at(&mut store, 10 + LOAD_MAX_IDLE_TIMEOUT_SECS - 1);
    assert!(!store.dispatch(TransitionFrontierPersistAction::LoadTimeout));
    at(&mut store, 10 + LOAD_MAX_IDLE_TIMEOUT_SECS + 1);
    assert!(store.dispatch(TransitionFrontierPersistAction::LoadTimeout));
    assert!(holds_sync(&store));
}

#[test]
fn without_restore_the_store_is_only_written() {
    let mut store = opening(false);
    assert!(!holds_sync(&store));
    assert!(store.dispatch(opened()));
    assert!(matches!(
        restore_state(&store),
        PersistRestoreState::Done {
            outcome: PersistRestoreOutcome::Off,
            ..
        }
    ));
    assert!(!store
        .service
        .asked
        .iter()
        .any(|asked| matches!(asked, TransitionFrontierPersistEffectfulAction::Load { .. })));
    assert!(!store.dispatch(TransitionFrontierPersistAction::LoadInit));
    assert!(!holds_sync(&store));
}

#[test]
fn the_sync_is_held_until_the_restore_is_decided() {
    let config = |restore| TransitionFrontierPersistConfig {
        enabled: true,
        checkpoint_interval_secs: DEFAULT_CHECKPOINT_INTERVAL_SECS,
        restore,
        load_timeout_secs: LOAD_TIMEOUT_SECS,
    };
    let disabled = TransitionFrontierPersistState::Disabled {
        reason: "off".into(),
    };
    assert!(!disabled.holds_sync(true));
    let idle = TransitionFrontierPersistState::new(&config(true));
    assert!(idle.holds_sync(true));
    assert!(!idle.holds_sync(false));
    let pending = TransitionFrontierPersistState::OpenPending {
        time: Timestamp::ZERO,
    };
    assert!(pending.holds_sync(true));
    let ready = |restore| {
        let mut state = PersistReadyState {
            opened_at: Timestamp::ZERO,
            dir: String::new(),
            open_outcome: String::new(),
            store: Default::default(),
            chain_put_sent: None,
            chain_puts_pending: 0,
            checkpoint: Default::default(),
            last_checkpoint: None,
            retry_after: None,
            stats: Default::default(),
            restore: PersistRestoreState::Idle,
            checkpoint_chain_len_limit: None,
        };
        state.restore = restore;
        TransitionFrontierPersistState::Ready(Box::new(state))
    };
    for (restore, held) in [
        (PersistRestoreState::Idle, true),
        (
            PersistRestoreState::LoadPending {
                time: Timestamp::ZERO,
                completed_stages: 0,
                last_progress_time: None,
            },
            true,
        ),
        (
            PersistRestoreState::Done {
                time: Timestamp::ZERO,
                outcome: PersistRestoreOutcome::TimedOut,
            },
            false,
        ),
    ] {
        assert_eq!(ready(restore.clone()).holds_sync(true), held, "{restore:?}");
        assert!(!ready(restore).holds_sync(false));
    }
}

/// OCaml's `Best_tip_prover.prove`: a best tip is served with a proof only
/// if the chain is k + 1 blocks or rooted at genesis.
#[test]
fn the_best_tip_is_proven_only_for_a_full_chain_or_from_genesis() {
    let state = state(true);
    let mut tf = state.transition_frontier;
    // The genesis block alone.
    assert!(tf.can_prove_best_tip());
    let k = constants::PROTOCOL_CONSTANTS.k.as_u32() as usize;
    let chain = stored_chain(k + 1, 3);
    let applied = |blocks: &[ArcBlockWithHash]| {
        blocks
            .iter()
            .map(|block| AppliedBlock {
                block: block.clone(),
                just_emitted_a_proof: false,
            })
            .collect::<Vec<_>>()
    };
    for (len, provable) in [(k + 1, true), (k, false), (k + 2, false), (2, false)] {
        tf.best_chain = applied(&chain[..len]);
        assert_eq!(tf.can_prove_best_tip(), provable, "{len} blocks");
    }
    tf.best_chain = vec![];
    assert!(!tf.can_prove_best_tip());
}

/// Run the actual request callback and inspect the response it dispatches,
/// before transport effects. No peer or network service is needed.
fn rpc_response(
    state: &State,
    request: crate::p2p::channels::rpc::P2pRpcRequest,
) -> Option<Box<crate::p2p::channels::rpc::P2pRpcResponse>> {
    use crate::p2p::{
        callbacks::P2pCallbacksAction,
        channels::{rpc::P2pChannelsRpcAction, P2pChannelsAction},
        P2pAction,
    };
    let mut state = state.clone();
    let peer_id = crate::p2p::identity::SecretKey::deterministic(1)
        .public_key()
        .peer_id();
    let action = P2pCallbacksAction::P2pChannelsRpcRequestReceived {
        peer_id,
        id: 7,
        request: Box::new(request),
    };
    let meta = redux::ActionMeta::zero_custom(state.time());
    let mut dispatcher = redux::Dispatcher::new();
    State::p2p_callback_reducer(
        crate::Substate::new(&mut state, &mut dispatcher),
        meta.with_action(&action),
    );
    let Some(crate::Action::P2p(P2pAction::Channels(P2pChannelsAction::Rpc(
        P2pChannelsRpcAction::ResponseSend {
            peer_id: response_peer,
            id,
            response,
        },
    )))) = dispatcher.pop()
    else {
        panic!("request did not dispatch a response");
    };
    assert_eq!(response_peer, peer_id);
    assert_eq!(id, 7);
    assert!(dispatcher.pop().is_none());
    response
}

fn best_tip_response(state: &State) -> Option<Box<crate::p2p::channels::rpc::P2pRpcResponse>> {
    rpc_response(
        state,
        crate::p2p::channels::rpc::P2pRpcRequest::BestTipWithProof,
    )
}

#[test]
fn restore_best_tip_is_unavailable_until_the_restored_frontier_commits() {
    use crate::p2p::channels::rpc::P2pRpcResponse;

    // OCaml rpcs.ml:1135-1151 returns None when get_transition_frontier ()
    // is None. In Rust genesis is already injected throughout startup restore.
    let idle = state(true);
    assert!(idle.transition_frontier.can_prove_best_tip());
    assert!(best_tip_response(&idle).is_none(), "before store open");
    let mut store = opening(true);
    assert!(best_tip_response(store.state()).is_none(), "opening store");
    assert!(store.dispatch(opened()));
    let mut ready_idle = store.state().clone();
    ready_idle
        .transition_frontier
        .persist
        .ready_mut()
        .unwrap()
        .restore = PersistRestoreState::Idle;
    assert!(best_tip_response(&ready_idle).is_none(), "before load");
    assert!(best_tip_response(store.state()).is_none(), "loading");

    let k = constants::PROTOCOL_CONSTANTS.k.as_u32() as usize;
    let chain = stored_chain(k, 3);
    let expected_tip = chain.last().unwrap();
    assert!(store.dispatch(restored(&chain)));
    assert!(best_tip_response(store.state()).is_none(), "replaying");

    // Model the ledger's successful commit notification. The frontier reducer
    // installs the applied chain; ReplayDone then releases the restore hold.
    let mut committed = store.state().clone();
    committed.transition_frontier.sync = TransitionFrontierSyncState::CommitSuccess {
        time: committed.time(),
        chain: chain
            .iter()
            .map(|block| AppliedBlock {
                block: block.clone(),
                just_emitted_a_proof: false,
            })
            .collect(),
        root_snarked_ledger_updates: Default::default(),
        needed_protocol_states: Default::default(),
    };
    let mut store = store_of(committed);
    assert!(
        best_tip_response(store.state()).is_none(),
        "commit notification"
    );
    assert!(store.dispatch(TransitionFrontierAction::Synced {
        needed_protocol_states: Default::default(),
    }));
    assert!(
        best_tip_response(store.state()).is_none(),
        "before ReplayDone"
    );
    assert!(store.dispatch(TransitionFrontierPersistAction::ReplayDone));
    let Some(response) = best_tip_response(store.state()) else {
        panic!("committed frontier is unavailable");
    };
    let P2pRpcResponse::BestTipWithProof(response) = *response else {
        panic!("wrong response kind");
    };
    // Expected block comes from the stored input, not the response under test.
    assert_eq!(response.best_tip, expected_tip.block);
    assert_eq!(response.proof.1, chain[0].block);
    assert_eq!(response.proof.0.len(), k);

    // With restore disabled, a normal genesis frontier remains serveable.
    assert!(best_tip_response(&state(false)).is_some());
}

#[test]
fn restore_catchup_responses_do_not_offer_the_genesis_frontier() {
    use crate::p2p::channels::rpc::{
        CatchupRequest, CatchupResponse, P2pRpcRequest, P2pRpcResponse,
    };
    use mina_p2p_messages::{list::List, rpc};

    let mut store = opening(true);
    assert!(store.dispatch(opened()));
    let loading = store.state().clone();
    let chain = stored_chain(4, 2);
    assert!(store.dispatch(restored(&chain)));
    for state in [&loading, store.state()] {
        let genesis = state.transition_frontier.best_tip().unwrap();
        let requests = [
            CatchupRequest::Ancestry(Box::new(rpc::GetAncestryV2Query {
                data: genesis.consensus_state().clone(),
                hash: genesis.hash().inner().0.clone(),
            })),
            CatchupRequest::TransitionChain(List::one(genesis.hash().inner().0.clone())),
            CatchupRequest::TransitionChainProof(genesis.hash().inner().0.clone()),
            CatchupRequest::TransitionKnowledge,
        ];
        for request in requests {
            let Some(response) = rpc_response(state, P2pRpcRequest::Catchup(request)) else {
                panic!("missing typed catch-up decline");
            };
            let P2pRpcResponse::Catchup(response) = *response else {
                panic!("wrong response kind");
            };
            assert!(match *response {
                CatchupResponse::Ancestry(response) => response.is_none(),
                CatchupResponse::TransitionChain(response) => response.is_none(),
                CatchupResponse::TransitionChainProof(response) => response.is_none(),
                CatchupResponse::TransitionKnowledge(response) => response.is_empty(),
            });
        }
        assert!(rpc_response(state, P2pRpcRequest::Block(genesis.hash().clone())).is_none());
    }
}

#[test]
fn restore_ledger_requests_decline_without_reading_loaded_or_partial_ledgers() {
    use crate::p2p::{
        channels::{
            rpc::{
                P2pChannelsRpcAction, P2pChannelsRpcState, P2pRpcLocalState,
                P2pRpcRemotePendingRequestState, P2pRpcRemoteState, P2pRpcRequest, P2pRpcResponse,
            },
            streaming_rpc::{
                staged_ledger_parts::StagedLedgerPartsSendProgress, P2pChannelsStreamingRpcAction,
                P2pChannelsStreamingRpcState, P2pStreamingRpcLocalState,
                P2pStreamingRpcRemoteState, P2pStreamingRpcRequest,
            },
            P2pChannelsAction,
        },
        P2pAction, P2pPeerState, P2pPeerStatus, P2pPeerStatusReady, PeerId,
    };

    let mut store = opening(true);
    assert!(store.dispatch(opened()));
    let loading = store.state().clone();
    assert!(store.dispatch(restored(&stored_chain(4, 2))));
    for mut state in [loading, store.state().clone()] {
        let peer_id = PeerId::from_bytes([5; 32]);
        let genesis = state.transition_frontier.best_tip().unwrap();
        let hash = genesis.snarked_ledger_hash().clone();
        let block_hash = genesis.hash().clone();
        let mut ready = P2pPeerStatusReady::new(false, Timestamp::ZERO, &Default::default());
        ready.channels.rpc = P2pChannelsRpcState::Ready {
            time: Timestamp::ZERO,
            local: P2pRpcLocalState::WaitingForRequest {
                time: Timestamp::ZERO,
            },
            remote: P2pRpcRemoteState {
                pending_requests: [
                    P2pRpcRequest::LedgerQuery(
                        hash.clone(),
                        v2::MinaLedgerSyncLedgerQueryStableV1::NumAccounts,
                    ),
                    P2pRpcRequest::StagedLedgerAuxAndPendingCoinbasesAtBlock(block_hash.clone()),
                ]
                .into_iter()
                .enumerate()
                .map(|(id, request)| P2pRpcRemotePendingRequestState {
                    time: Timestamp::ZERO,
                    id: id as u64,
                    request,
                    is_pending: false,
                })
                .collect(),
                last_responded: Timestamp::ZERO,
            },
        };
        let streaming = P2pStreamingRpcRequest::StagedLedgerParts(block_hash);
        ready.channels.streaming_rpc = P2pChannelsStreamingRpcState::Ready {
            time: Timestamp::ZERO,
            local: P2pStreamingRpcLocalState::WaitingForRequest {
                time: Timestamp::ZERO,
            },
            remote: P2pStreamingRpcRemoteState::Requested {
                time: Timestamp::ZERO,
                id: 2,
                progress: StagedLedgerPartsSendProgress::LedgerGetIdle {
                    time: Timestamp::ZERO,
                }
                .into(),
                request: Box::new(streaming),
            },
            remote_last_responded: Timestamp::ZERO,
        };
        state.p2p.ready_mut().unwrap().peers.insert(
            peer_id,
            P2pPeerState {
                is_libp2p: false,
                dial_opts: None,
                identify: None,
                status: P2pPeerStatus::Ready(Box::new(ready)),
            },
        );
        let mut dispatcher = redux::Dispatcher::new();
        let action = redux::ActionMeta::zero_custom(state.time())
            .with_action(crate::ledger::read::LedgerReadAction::FindTodos.into());
        crate::reducer(&mut state, &action, &mut dispatcher);
        let mut declined = Vec::new();
        let mut streamed_declines = 0;
        while let Some(action) = dispatcher.pop() {
            match &action {
                crate::Action::P2p(P2pAction::Channels(P2pChannelsAction::Rpc(
                    P2pChannelsRpcAction::ResponseSend {
                        peer_id: peer,
                        id,
                        response,
                    },
                ))) => {
                    assert_eq!(*peer, peer_id);
                    if *id == 0 {
                        assert!(
                            matches!(response.as_deref(), Some(P2pRpcResponse::LedgerQueryError(h, error))
                            if h == &hash && error.to_string() == "No Frontier")
                        );
                    } else {
                        assert_eq!(*id, 1);
                        assert!(response.is_none());
                    }
                    declined.push(*id);
                }
                crate::Action::P2p(P2pAction::Channels(P2pChannelsAction::StreamingRpc(
                    P2pChannelsStreamingRpcAction::ResponseSendInit {
                        peer_id: peer,
                        id,
                        response,
                    },
                ))) => {
                    assert_eq!(*peer, peer_id);
                    assert_eq!(*id, 2);
                    assert!(response.is_none());
                    declined.push(*id);
                }
                crate::Action::P2pEffectful(crate::p2p::P2pEffectfulAction::Channels(
                    crate::p2p::channels::P2pChannelsEffectfulAction::MessageSend {
                        msg: crate::p2p::channels::ChannelMsg::StreamingRpc(
                            crate::p2p::channels::streaming_rpc::StreamingRpcChannelMsg::Response(2, None),
                        ),
                        ..
                    },
                )) => streamed_declines += 1,
                crate::Action::Ledger(crate::ledger::LedgerAction::Read(
                    crate::ledger::read::LedgerReadAction::Init { .. },
                )) => {
                    panic!("restore scheduled a peer ledger read");
                }
                _ => {}
            }
            // Run the channel transitions too: merely queueing a response
            // whose enabling condition rejects it would leave the peer waiting.
            if matches!(action, crate::Action::P2p(_)) {
                assert!(action.is_enabled(&state, state.time()), "{action:?}");
                let action = redux::ActionMeta::zero_custom(state.time()).with_action(action);
                crate::reducer(&mut state, &action, &mut dispatcher);
            }
        }
        assert_eq!(declined, [0, 1, 2]);
        assert_eq!(streamed_declines, 1);
    }
}
/// A node whose genesis came from the genesis cache (I5d2): its genesis
/// ledgers were not loaded.
fn deferring(state: &mut State) {
    let tf = &mut state.transition_frontier;
    tf.config.genesis_ledgers_deferred = Some(crate::config::DEVNET_CONFIG.clone());
    tf.genesis_ledgers = crate::transition_frontier::GenesisLedgersState::Deferred;
}

fn genesis_ledgers(store: &Store) -> crate::transition_frontier::GenesisLedgersState {
    store.state().transition_frontier.genesis_ledgers.clone()
}

/// I5d2: with its genesis ledgers deferred, a node that doesn't restore
/// loads them before it bootstraps, so that its bootstrap is the one a node
/// started from the full config makes (the genesis ledgers are copy
/// origins); the sync waits for them and stays held if they can't be loaded.
#[test]
fn deferred_genesis_ledgers_are_loaded_before_a_bootstrap() {
    use crate::transition_frontier::{GenesisLedgersState, TransitionFrontierAction};
    let outcomes = [
        TransitionFrontierPersistAction::LoadAbsent {
            reason: "the store is empty".into(),
        },
        TransitionFrontierPersistAction::LoadLedgersOnly {
            reason: "the stored tip is 2000 slots behind the clock".into(),
            ledgers: 3,
        },
        TransitionFrontierPersistAction::LoadDiscarded {
            reason: "3 restores didn't finish".into(),
        },
    ];
    for (i, outcome) in outcomes.into_iter().enumerate() {
        let mut state = state(true);
        deferring(&mut state);
        let mut store = store_of(state);
        assert!(store.dispatch(TransitionFrontierPersistAction::OpenInit));
        assert!(store.dispatch(opened()));
        // Not while the restore is to be decided.
        assert!(!store.dispatch(TransitionFrontierAction::GenesisLedgersLoadInit));
        assert!(store.dispatch(outcome.clone()));
        // The restore is done, but the sync waits for the genesis ledgers.
        assert!(holds_sync(&store), "{outcome:?}");
        assert!(store.dispatch(TransitionFrontierAction::GenesisLedgersLoadInit));
        assert!(matches!(
            genesis_ledgers(&store),
            GenesisLedgersState::Pending { .. }
        ));
        assert!(holds_sync(&store));
        assert!(!store.dispatch(TransitionFrontierAction::GenesisLedgersLoadInit));
        if i == 0 {
            assert!(
                store.dispatch(TransitionFrontierAction::GenesisLedgersLoadError {
                    error: "no such file".into()
                })
            );
            assert!(matches!(
                genesis_ledgers(&store),
                GenesisLedgersState::Error { .. }
            ));
        } else {
            assert!(store.dispatch(TransitionFrontierAction::GenesisLedgersLoadSuccess));
            assert!(matches!(
                genesis_ledgers(&store),
                GenesisLedgersState::Loaded
            ));
        }
        assert_eq!(holds_sync(&store), i == 0, "{outcome:?}");
        assert!(!store.dispatch(TransitionFrontierAction::GenesisLedgersLoadInit));
    }

    // Without a usable store (another process has it, say), at once.
    let mut state = state(true);
    deferring(&mut state);
    state.transition_frontier.persist = TransitionFrontierPersistState::Disabled {
        reason: "locked".into(),
    };
    let mut store = store_of(state);
    assert!(holds_sync(&store));
    assert!(store.dispatch(TransitionFrontierAction::GenesisLedgersLoadInit));
}

/// I5d2: a restore replaces the genesis ledgers, so they are never loaded,
/// and don't hold the sync once the restore is done.
#[test]
fn deferred_genesis_ledgers_are_not_loaded_after_a_restore() {
    use crate::transition_frontier::{GenesisLedgersState, TransitionFrontierAction};
    let k = constants::PROTOCOL_CONSTANTS.k.as_u32();
    let (mut store, _) = {
        let (store, chain) = replay_committed(k as usize + 3);
        let mut state = store.state().clone();
        deferring(&mut state);
        (store_of(state), chain)
    };
    // Replaying: held, by the restore.
    assert!(holds_sync(&store));
    assert!(!store.dispatch(TransitionFrontierAction::GenesisLedgersLoadInit));
    assert!(store.dispatch(TransitionFrontierPersistAction::ReplayDone));
    assert!(!holds_sync(&store));
    assert!(!store.dispatch(TransitionFrontierAction::GenesisLedgersLoadInit));
    assert!(matches!(
        genesis_ledgers(&store),
        GenesisLedgersState::Deferred
    ));
}

fn ledger_read(
    request: crate::ledger::read::LedgerReadRequest,
) -> crate::ledger::read::LedgerReadAction {
    crate::ledger::read::LedgerReadAction::Init {
        request,
        callback: crate::ledger::read::LedgerReadInitCallback::None,
    }
}

/// GraphQL/RPC ledger-read requests used to stay in `state.rpc.requests`
/// forever after their response: only the P2P connection RPCs dispatched
/// `RpcAction::Finish`. Every answered request must leave the map,
/// whatever answers it.
#[test]
fn answered_ledger_read_rpcs_leave_the_request_map() {
    use crate::rpc::{
        AccountQuery, DelegatorsQuery, HardForkBlockSpec, LedgerExportKind, RpcAction,
        RpcScanStateSummaryGetQuery,
    };
    use ledger::TokenId;
    use mina_core::requests::RpcId;

    let mut store = store_of(state(true));
    let root = store
        .state()
        .transition_frontier
        .best_tip()
        .unwrap()
        .merkle_root_hash()
        .clone();
    let genesis_producer = crate::account::AccountSecretKey::genesis_producer();
    let producer = genesis_producer.public_key();
    let account_id =
        ledger::AccountId::new(genesis_producer.public_key_compressed(), TokenId::default());

    // The requests that wait for the ledger service, which this harness
    // has none of: driven through their Pending and Success by hand.
    let accounts = RpcId::new_unchecked(0, 0);
    assert!(store.dispatch(RpcAction::LedgerAccountsGetInit {
        rpc_id: accounts,
        account_query: AccountQuery::SinglePublicKey(producer.clone()),
    }));
    assert!(store.dispatch(RpcAction::LedgerAccountsGetPending { rpc_id: accounts }));
    assert!(store.dispatch(RpcAction::LedgerAccountsGetSuccess {
        rpc_id: accounts,
        account_query: AccountQuery::SinglePublicKey(producer.clone()),
        accounts: vec![],
    }));

    let status = RpcId::new_unchecked(0, 1);
    assert!(store.dispatch(RpcAction::LedgerStatusGetInit {
        rpc_id: status,
        ledger_hash: root,
    }));
    assert!(store.dispatch(RpcAction::LedgerStatusGetPending { rpc_id: status }));
    assert!(store.dispatch(RpcAction::LedgerStatusGetSuccess {
        rpc_id: status,
        response: None,
    }));

    let delegators = RpcId::new_unchecked(0, 2);
    assert!(store.dispatch(RpcAction::LedgerAccountDelegatorsGetInit {
        rpc_id: delegators,
        query: DelegatorsQuery::LastEpoch { limit: None },
        account_id,
    }));
    assert!(store.dispatch(RpcAction::LedgerAccountDelegatorsGetPending { rpc_id: delegators }));
    assert!(
        store.dispatch(RpcAction::LedgerAccountDelegatorsGetSuccess {
            rpc_id: delegators,
            response: None,
        })
    );

    let export = RpcId::new_unchecked(0, 3);
    assert!(store.dispatch(RpcAction::LedgerExportInit {
        rpc_id: export,
        kind: LedgerExportKind::SnarkedLedger,
    }));
    assert!(store.dispatch(RpcAction::LedgerExportPending { rpc_id: export }));
    assert!(store.dispatch(RpcAction::LedgerExportSuccess {
        rpc_id: export,
        response: Err("no ledger".to_owned()),
    }));

    // No fork block to export: the reducer answers this one itself, with
    // the pending and success it enqueues.
    let fork = RpcId::new_unchecked(0, 4);
    assert!(store.dispatch(RpcAction::HardForkConfigInit {
        rpc_id: fork,
        spec: HardForkBlockSpec::StateHash(v2::StateHash::zero()),
    }));

    // ScanStateSummary through its own pair; without a breadcrumb in this
    // state the reducer may also answer it within the init dispatch.
    let summary = RpcId::new_unchecked(0, 5);
    assert!(store.dispatch(RpcAction::ScanStateSummaryGetInit {
        rpc_id: summary,
        query: RpcScanStateSummaryGetQuery::ForBestTip,
    }));
    if let Some(rpc) = store.state().rpc.requests.get(&summary) {
        if rpc.status.is_init() {
            assert!(store.dispatch(RpcAction::ScanStateSummaryGetPending {
                rpc_id: summary,
                block: None,
            }));
        }
        assert!(store.dispatch(RpcAction::ScanStateSummaryGetSuccess {
            rpc_id: summary,
            scan_state: Ok(Vec::new()),
        }));
    }

    for (name, rpc_id) in [
        ("LedgerAccountsGet", accounts),
        ("LedgerStatusGet", status),
        ("LedgerAccountDelegatorsGet", delegators),
        ("LedgerExport", export),
        ("HardForkConfig", fork),
        ("ScanStateSummaryGet", summary),
    ] {
        assert!(
            !store.state().rpc.requests.contains_key(&rpc_id),
            "{name}: the answered request stayed in the map"
        );
    }
}

/// A transaction submission's answer, whichever of the three it is, takes
/// its request out of `state.rpc.requests` too: an accepted, a rejected and
/// a refused submission leave nothing behind.
#[test]
fn answered_transaction_submissions_leave_the_request_map() {
    use crate::rpc::{RpcAction, RpcRequest, RpcRequestState, RpcRequestStatus};
    use mina_core::requests::RpcId;

    let (accepted, rejected, refused) = (
        RpcId::new_unchecked(0, 0),
        RpcId::new_unchecked(0, 1),
        RpcId::new_unchecked(0, 2),
    );
    let mut state = state(true);
    // Waiting for the pool's verdict, which this harness has no pool for.
    for rpc_id in [accepted, rejected] {
        state.rpc.requests.insert(
            rpc_id,
            RpcRequestState {
                req: RpcRequest::TransactionInject(vec![]),
                status: RpcRequestStatus::Pending {
                    time: Timestamp::ZERO,
                },
                data: Default::default(),
            },
        );
    }
    let mut store = store_of(state);
    assert!(store.dispatch(RpcAction::TransactionInjectSuccess {
        rpc_id: accepted,
        response: vec![],
    }));
    assert!(store.dispatch(RpcAction::TransactionInjectRejected {
        rpc_id: rejected,
        response: vec![],
    }));
    // Nothing to submit: refused within the init.
    assert!(store.dispatch(RpcAction::TransactionInjectInit {
        rpc_id: refused,
        commands: vec![],
    }));
    for (name, rpc_id) in [
        ("accepted", accepted),
        ("rejected", rejected),
        ("refused", refused),
    ] {
        assert!(
            !store.state().rpc.requests.contains_key(&rpc_id),
            "{name}: the answered submission stayed in the map"
        );
    }
}

fn assert_ledger_work_held(store: &mut Store) {
    use crate::ledger::read::LedgerReadRequest;
    let tf = &store.state().transition_frontier;
    let tip = tf.best_tip().unwrap().clone();
    let hash = tip.merkle_root_hash().clone();
    assert!(tf.persist_holds_sync());
    assert!(tf.is_bootstrapping(Timestamp::ZERO));
    assert!(!store
        .dispatch(crate::block_producer::BlockProducerAction::BestTipUpdate { best_tip: tip }));
    for request in [
        LedgerReadRequest::GetAccounts(hash.clone(), vec![], None),
        LedgerReadRequest::GetNumAccounts(hash.clone()),
        LedgerReadRequest::GetChildAccountsAtAddr(
            hash.clone(),
            crate::ledger::LedgerAddress::root(),
        ),
        LedgerReadRequest::GetChildHashesAtAddr(hash, crate::ledger::LedgerAddress::root(), 1),
    ] {
        assert!(!store.dispatch(ledger_read(request)));
    }
    assert!(store.service.reads.is_empty());
}

#[test]
fn an_abandoned_restore_loads_genesis_before_releasing_a_producer() {
    let mut state = state(true);
    deferring(&mut state);
    let mut store = store_of(state);
    assert!(store.dispatch(TransitionFrontierPersistAction::OpenInit));
    assert!(store.dispatch(opened()));
    at(&mut store, LOAD_TIMEOUT_SECS + 1);
    assert!(store.dispatch(TransitionFrontierPersistAction::LoadTimeout));
    assert!(store.dispatch(TransitionFrontierPersistAction::LoadCancelPending));
    assert_ledger_work_held(&mut store);
    assert!(!store.dispatch(TransitionFrontierAction::GenesisLedgersLoadInit));

    assert!(store.dispatch(TransitionFrontierPersistAction::LoadCancelled));
    assert_ledger_work_held(&mut store);
    assert!(store.dispatch(TransitionFrontierAction::GenesisLedgersLoadInit));
    assert_ledger_work_held(&mut store);
    assert!(store.dispatch(TransitionFrontierAction::GenesisLedgersLoadSuccess));
    assert!(!holds_sync(&store));
    let best_tip = store
        .state()
        .transition_frontier
        .best_tip()
        .unwrap()
        .clone();
    assert!(store.dispatch(crate::block_producer::BlockProducerAction::BestTipUpdate { best_tip }));
}

/// GraphQL account/accounts and RPC All/MultipleIds requests stay in Init;
/// FindTodos retries them against the new best tip after restore, or against
/// genesis only once the full load succeeds. They cannot complete as empty.
#[test]
fn cached_genesis_account_queries_wait_for_usable_ledgers() {
    use crate::{
        ledger::read::{LedgerReadAction, LedgerReadRequest},
        rpc::{AccountQuery, RpcAction},
    };
    use mina_core::requests::RpcId;
    let public_key = crate::account::AccountSecretKey::genesis_producer().public_key();
    let queries = [
        AccountQuery::All,
        AccountQuery::SinglePublicKey(public_key.clone()),
        AccountQuery::PubKeyWithTokenId(public_key, Default::default()),
        AccountQuery::MultipleIds(vec![]),
    ];
    for restored in [false, true] {
        let mut state = if restored {
            let (store, _) =
                replay_committed(constants::PROTOCOL_CONSTANTS.k.as_u32() as usize + 3);
            store.state().clone()
        } else {
            state(true)
        };
        deferring(&mut state);
        let mut store = store_of(state);
        assert_ledger_work_held(&mut store);
        for (i, query) in queries.iter().enumerate() {
            let rpc_id = RpcId::new_unchecked(0, i);
            assert!(store.dispatch(RpcAction::LedgerAccountsGetInit {
                rpc_id,
                account_query: query.clone(),
            }));
            assert!(store.state().rpc.requests[&rpc_id].status.is_init());
            assert!(!store.dispatch(RpcAction::LedgerAccountsGetSuccess {
                rpc_id,
                accounts: vec![],
                account_query: query.clone(),
            }));
        }
        assert!(store.dispatch(LedgerReadAction::FindTodos));
        assert!(store.service.reads.is_empty());
        if restored {
            assert!(store.dispatch(TransitionFrontierPersistAction::ReplayDone));
        } else {
            assert!(store.dispatch(TransitionFrontierPersistAction::OpenInit));
            assert!(store.dispatch(opened()));
            assert!(store.dispatch(TransitionFrontierPersistAction::LoadAbsent {
                reason: "empty".into()
            }));
            assert!(store.dispatch(TransitionFrontierAction::GenesisLedgersLoadInit));
            assert_ledger_work_held(&mut store);
            assert!(store.dispatch(TransitionFrontierAction::GenesisLedgersLoadSuccess));
        }
        assert!(store.state().transition_frontier.genesis_ledgers_ready());
        assert!(store.dispatch(LedgerReadAction::FindTodos));
        let expected_hash = store
            .state()
            .transition_frontier
            .best_tip()
            .unwrap()
            .merkle_root_hash();
        assert_eq!(store.service.reads.len(), queries.len());
        for (request, query) in store.service.reads.iter().zip(&queries) {
            assert!(
                matches!(request, LedgerReadRequest::AccountsForRpc(_, hash, actual)
                if hash == expected_hash && actual == query)
            );
        }
    }
}

#[test]
fn failed_deferred_genesis_load_keeps_all_ledger_work_held() {
    let (store, _) = replay_committed(constants::PROTOCOL_CONSTANTS.k.as_u32() as usize + 3);
    let mut state = store.state().clone();
    deferring(&mut state);
    let mut store = store_of(state);
    // A late replay failure, after some stored ledgers may have been inserted.
    assert!(
        store.dispatch(TransitionFrontierPersistAction::ReplayFailed {
            error: "replay failed".into()
        })
    );
    assert!(store.dispatch(TransitionFrontierAction::GenesisLedgersLoadInit));
    assert!(
        store.dispatch(TransitionFrontierAction::GenesisLedgersLoadError {
            error: "config unreadable".into()
        })
    );
    assert_ledger_work_held(&mut store);
    assert!(!store.dispatch(TransitionFrontierAction::GenesisLedgersLoadInit));
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn failed_deferred_genesis_load_stops_the_node() {
    const MODE: &str = "MINA_I5D2_FATAL_GENESIS_TEST";
    if std::env::var_os(MODE).is_some() {
        crate::transition_frontier::transition_frontier_effects::genesis_ledgers_load_failed(
            "config unreadable",
        );
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "transition_frontier::persist::transition_frontier_restore_tests::failed_deferred_genesis_load_stops_the_node", "--nocapture"])
        .env(MODE, "1")
        .output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Fatal: deferred genesis ledger load failed"),
        "{stderr}"
    );
    assert!(stderr.contains("config unreadable"), "{stderr}");
}

#[test]
fn ordinary_and_compact_startup_do_not_wait_for_deferred_ledgers() {
    use crate::ledger::read::LedgerReadRequest;
    for restore in [false, true] {
        let state = state(restore);
        assert!(state
            .transition_frontier
            .config
            .genesis_ledgers_deferred
            .is_none());
        assert!(state.transition_frontier.genesis_ledgers_ready());
        let hash = state
            .transition_frontier
            .best_tip()
            .unwrap()
            .merkle_root_hash()
            .clone();
        assert!(ledger_read(LedgerReadRequest::GetNumAccounts(hash))
            .is_enabled(&state, Timestamp::ZERO));
    }
}

#[test]
fn cached_genesis_status_and_delegator_queries_resume() {
    use crate::{
        ledger::read::{LedgerReadAction, LedgerReadRequest},
        rpc::{DelegatorsQuery, RpcAction, MAX_LAST_EPOCH_DELEGATORS},
    };
    use mina_core::requests::RpcId;
    let (store, _) = replay_committed(constants::PROTOCOL_CONSTANTS.k.as_u32() as usize + 3);
    let mut state = store.state().clone();
    deferring(&mut state);
    let mut store = store_of(state);
    let hash = store
        .state()
        .transition_frontier
        .best_tip()
        .unwrap()
        .merkle_root_hash()
        .clone();
    let account_id = ledger::AccountId {
        public_key: crate::account::AccountSecretKey::genesis_producer()
            .public_key()
            .try_into()
            .unwrap(),
        token_id: Default::default(),
    };
    let status_id = RpcId::new_unchecked(0, 0);
    let queries = [
        DelegatorsQuery::Ledger(hash.clone()),
        DelegatorsQuery::LastEpoch { limit: None },
        DelegatorsQuery::LastEpoch {
            limit: Some(MAX_LAST_EPOCH_DELEGATORS),
        },
    ]
    .into_iter()
    .enumerate()
    .map(|(index, query)| (RpcId::new_unchecked(0, index + 1), query))
    .collect::<Vec<_>>();
    assert!(store.dispatch(RpcAction::LedgerStatusGetInit {
        rpc_id: status_id,
        ledger_hash: hash.clone(),
    }));
    for (rpc_id, query) in &queries {
        assert!(store.dispatch(RpcAction::LedgerAccountDelegatorsGetInit {
            rpc_id: *rpc_id,
            query: query.clone(),
            account_id: account_id.clone(),
        }));
    }
    assert!(store.dispatch(LedgerReadAction::FindTodos));
    assert!(store.service.reads.is_empty());
    for id in std::iter::once(status_id).chain(queries.iter().map(|(id, _)| *id)) {
        assert!(store.state().rpc.requests[&id].status.is_init());
    }
    assert!(store.dispatch(TransitionFrontierPersistAction::ReplayDone));
    assert!(store.dispatch(LedgerReadAction::FindTodos));
    assert_eq!(store.service.reads.len(), 1 + queries.len());
    assert!(store.service.reads.iter().any(|read| matches!(read,
        LedgerReadRequest::GetLedgerStatus(id, actual) if *id == status_id && actual == &hash)));
    for (rpc_id, query) in &queries {
        assert!(store.service.reads.iter().any(|read| matches!(read,
            LedgerReadRequest::GetAccountDelegators(id, actual, account)
                if id == rpc_id && actual == query && account == &account_id)));
    }
}

#[test]
fn cached_genesis_ledger_exports_resume_after_restore() {
    use crate::{
        ledger::read::{LedgerReadAction, LedgerReadRequest},
        rpc::{LedgerExportKind, RpcAction},
    };
    use mina_core::requests::RpcId;

    let (store, _) = replay_committed(constants::PROTOCOL_CONSTANTS.k.as_u32() as usize + 3);
    let mut state = store.state().clone();
    deferring(&mut state);
    let expected_tip = state.transition_frontier.best_tip().unwrap().hash().clone();
    let mut store = store_of(state);
    for (index, kind) in LedgerExportKind::ALL.into_iter().enumerate() {
        let rpc_id = RpcId::new_unchecked(0, index);
        assert!(store.dispatch(RpcAction::LedgerExportInit { rpc_id, kind }));
        assert!(store.state().rpc.requests[&rpc_id].status.is_init());
    }
    assert!(store.dispatch(LedgerReadAction::FindTodos));
    assert!(store.service.reads.is_empty(), "restore must hold exports");
    assert!(store.dispatch(TransitionFrontierPersistAction::ReplayDone));
    assert!(store.dispatch(LedgerReadAction::FindTodos));
    assert_eq!(store.service.reads.len(), LedgerExportKind::ALL.len());
    for (index, kind) in LedgerExportKind::ALL.into_iter().enumerate() {
        let rpc_id = RpcId::new_unchecked(0, index);
        assert!(store.service.reads.iter().any(|read| matches!(read,
            LedgerReadRequest::LedgerExport(id, request)
                if *id == rpc_id && request.kind == kind && request.best_tip.state_hash == expected_tip)));
    }
}

/// Called with a real store worker stopped inside a read. Time is simulated;
/// the worker remains blocked for every assertion and is released by its caller.
pub(crate) fn stalled_load_cancellation_deadline(
    control: &crate::persist::load_control::LoadControl,
) {
    let mut store = opening(true);
    at(&mut store, 10);
    assert!(store.dispatch(opened()));
    at(&mut store, 10 + LOAD_TIMEOUT_SECS + 1);
    assert!(store.dispatch(TransitionFrontierPersistAction::LoadTimeout));
    assert!(control.cancel());
    assert!(store.dispatch(TransitionFrontierPersistAction::LoadCancelPending));
    at(&mut store, 10 + LOAD_TIMEOUT_SECS + 30);
    assert!(!store.dispatch(TransitionFrontierPersistAction::LoadTimeout));
    assert!(holds_sync(&store));
    at(&mut store, 10 + LOAD_TIMEOUT_SECS + 32);
    assert!(store.dispatch(TransitionFrontierPersistAction::LoadTimeout));
    assert!(
        !holds_sync(&store),
        "a stalled read must release bootstrap after cancellation's cap"
    );
    assert!(matches!(
        store.state().transition_frontier.persist,
        TransitionFrontierPersistState::Disabled { .. }
    ));
    // No new writes queue behind the stuck store thread, and no late load
    // result can re-enable persistence or insert a restored chain.
    assert!(!store.dispatch(TransitionFrontierPersistAction::LoadCancelled));
    assert!(!store.dispatch(restored(&stored_chain(2, 1))));
    assert!(
        !store.dispatch(TransitionFrontierPersistAction::StoreSummary {
            store: Box::new(PersistStoreSummary::default()),
        })
    );
}

#[test]
fn ledger_0201_handoff_winner_is_not_abandoned_by_the_cancellation_cap() {
    let control = crate::persist::load_control::LoadControl::default();
    let mut store = opening(true);
    at(&mut store, 10);
    assert!(store.dispatch(opened()));
    assert!(control.begin_handoff());
    for elapsed in [61, 92, 600] {
        at(&mut store, 10 + elapsed);
        assert!(store.dispatch(TransitionFrontierPersistAction::LoadTimeout));
        assert!(!control.cancel());
        // The production effect does not dispatch LoadCancelPending when
        // handoff won. Its installed ledgers must not be abandoned.
        assert!(holds_sync(&store));
    }
    assert!(store.dispatch(restored(&stored_chain(2, 1))));
    assert!(matches!(
        restore_state(&store),
        PersistRestoreState::Replaying { .. }
    ));
}
