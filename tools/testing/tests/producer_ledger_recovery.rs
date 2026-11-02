//! Requires an isolated network namespace with loopback enabled.
//! Uses real ledger workers, VRF evaluation and peer transfers, but dummy
//! SNARK proofs. It does not qualify proof generation or public networking.

use std::time::{Duration, Instant};

use mina_node::{
    block_producer::{BlockProducerAction, BlockProducerCurrentState},
    ledger::write::LedgerWriteRequest,
    ledger_effectful::LedgerEffectfulAction,
    transition_frontier::genesis::{GenesisConfig, NonStakers},
    Action, ActionKind, BlockProducerConfig,
};
use mina_node_testing::{
    cluster::{
        runner::{ClusterRunner, RunCfg, RunCfgAdvanceTime},
        Cluster, ClusterConfig, ClusterNodeId,
    },
    node::{RustNodeBlockProducerTestingConfig, RustNodeTestingConfig},
};
use mina_p2p_messages::v2;

async fn wait_synced(runner: &mut ClusterRunner<'_>, nodes: &[ClusterNodeId]) {
    for &node_id in nodes {
        let ready = |state: &mina_node::State| {
            state.transition_frontier.sync.is_synced()
                && state.transition_frontier.best_tip().is_some()
        };
        if ready(runner.node(node_id).unwrap().state()) {
            continue;
        }
        runner
            .run(
                RunCfg::default()
                    .timeout(Duration::from_secs(180))
                    .action_handler(move |id, state, _, _| id == node_id && ready(state)),
            )
            .await
            .expect("local node must initialize its genesis frontier");
    }
}

#[tokio::test]
#[ignore = "isolated network, dedicated ports and local producer fixture"]
async fn producer_recovers_from_worker_hash_mismatch() {
    assert_eq!(
        std::env::var("MINA_TEST_ISOLATED_NETWORK").as_deref(),
        Ok("1")
    );
    let work_dir = temp_dir::TempDir::new().unwrap();
    mina_core::set_work_dir(work_dir.path().to_owned());
    mina_core::network::NetworkConfig::init("devnet").unwrap();
    rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build_global()
        .unwrap();
    mina_node_native::tracing::initialize(mina_node_native::tracing::Level::INFO);
    let initial_time = redux::Timestamp::global_now();
    let mut constants = v2::PROTOCOL_CONSTANTS.clone();
    // Fill the private frontier before injection so peers can serve a full
    // best-tip proof even though this genesis inherits a post-fork height.
    constants.k = 1.into();
    constants.genesis_state_timestamp =
        v2::BlockTimeTimeStableV1((u64::from(initial_time) / 1_000_000).into());
    let genesis = GenesisConfig::Counts {
        whales: 2,
        fish: 0,
        non_stakers: NonStakers::None,
        constants,
    };
    let config: ClusterConfig = serde_json::from_value(serde_json::json!({
        "port_range": [4300, 4309],
        "all_rust_to_rust_use_webrtc": false,
        "proof_kind": "Dummy"
    }))
    .unwrap();
    let mut cluster = Cluster::new(config);
    let mut runner = ClusterRunner::new(&mut cluster, |_| {});
    let base = RustNodeTestingConfig {
        genesis: genesis.into(),
        initial_time,
        peer_discovery: false,
        max_peers: 3,
        libp2p_target_peers: Some(2),
        ..RustNodeTestingConfig::devnet_default()
    };
    let seed = runner.add_rust_node(base.clone().with_libp2p_port(9420));
    wait_synced(&mut runner, &[seed]).await;
    let keys = runner.block_producer_sec_keys(seed);
    assert_eq!(keys.len(), 2);
    let producers: Vec<_> = keys
        .into_iter()
        .enumerate()
        .map(|(i, (sec_key, stake))| {
            assert!(stake > 0);
            runner.add_rust_node(RustNodeTestingConfig {
                initial_peers: vec![seed.into()],
                libp2p_port: Some(9421 + i as u16),
                block_producer: Some(RustNodeBlockProducerTestingConfig {
                    config: BlockProducerConfig::new(sec_key.public_key().into()),
                    sec_key,
                }),
                ..base.clone()
            })
        })
        .collect();
    let producer = producers[0];
    wait_synced(&mut runner, &producers).await;

    runner
        .run(
            RunCfg::default()
                .timeout(Duration::from_secs(300))
                .advance_time(RunCfgAdvanceTime::Rand(1..=200))
                .action_handler(move |id, state, _, _| {
                    id == producer
                        && state.transition_frontier.best_tip().is_some_and(|tip| {
                            tip.height() > 1 && state.block_producer.is_me(tip.producer())
                        })
                }),
        )
        .await
        .expect("producer must produce before fault injection");
    let before = runner
        .node(producer)
        .unwrap()
        .state()
        .transition_frontier
        .best_tip()
        .unwrap();
    eprintln!(
        "producer-recovery baseline height={} slot={}",
        before.height(),
        before.global_slot()
    );

    let started = Instant::now();
    let mut injected = false;
    let mut paused = false;
    let mut staking_sync_started = false;
    let mut root_sync_completed = false;
    runner
        .run(
            RunCfg::default()
                .timeout(Duration::from_secs(300))
                .advance_time(RunCfgAdvanceTime::Rand(1..=200))
                .action_handler(move |id, state, service, action| {
                    if id != producer {
                        return false;
                    }
                    if !injected {
                        if let Action::LedgerEffects(LedgerEffectfulAction::WriteInit {
                            request: LedgerWriteRequest::BlockApply { block, .. },
                            ..
                        }) = action.action()
                        {
                            if !state.block_producer.is_me(block.producer()) {
                                service.inject_next_block_apply_mismatch();
                                injected = true;
                                eprintln!(
                                    "producer-recovery injecting peer block {}",
                                    block.hash()
                                );
                            }
                        }
                    }
                    if matches!(
                        action.action(),
                        Action::BlockProducer(BlockProducerAction::LedgerRecoveryAbort)
                    ) {
                        assert!(injected);
                        paused = true;
                        eprintln!("producer-recovery paused");
                    }
                    if paused && state.transition_frontier.ledger_recovery_pending() {
                        let bp = state.block_producer.as_ref().unwrap();
                        assert!(matches!(bp.current, BlockProducerCurrentState::Idle { .. }));
                        assert!(bp.next_producer_timing.is_none());
                        assert!(!state.block_producer.is_producing());
                        assert!(state.transition_frontier.best_tip().is_none());
                        staking_sync_started |= action.action().kind()
                            == ActionKind::TransitionFrontierSyncLedgerStakingPending;
                        root_sync_completed |= action.action().kind()
                            == ActionKind::TransitionFrontierSyncLedgerRootSuccess;
                    }
                    if let Some(recovery) = &state.transition_frontier.ledger_recovery {
                        assert_eq!(recovery.attempts, 1, "must not repeatedly quarantine");
                        if recovery.recovered_at.is_some() {
                            assert!(paused);
                            assert!(staking_sync_started && root_sync_completed);
                            assert!(state.transition_frontier.sync.is_synced());
                            assert!(state.transition_frontier.best_tip().is_some());
                            return true;
                        }
                    }
                    false
                }),
        )
        .await
        .expect("producer must quarantine then recover through peers");
    let state = runner.node(producer).unwrap().state();
    let recovery = state.transition_frontier.ledger_recovery.as_ref().unwrap();
    let recovered_at = recovery.recovered_at.unwrap();
    let recovered_height = state.transition_frontier.best_tip().unwrap().height();
    eprintln!(
        "producer-recovery complete phase_wall_seconds={:.3} simulated_seconds={:.3} height={}",
        started.elapsed().as_secs_f64(),
        recovered_at
            .checked_sub(recovery.time)
            .unwrap()
            .as_secs_f64(),
        recovered_height
    );

    let mut won = false;
    runner
        .run(
            RunCfg::default()
                .timeout(Duration::from_secs(300))
                .advance_time(RunCfgAdvanceTime::Rand(1..=200))
                .action_handler(move |id, state, _, action| {
                    if id != producer {
                        return false;
                    }
                    let recovery = state.transition_frontier.ledger_recovery.as_ref().unwrap();
                    assert_eq!(recovery.attempts, 1);
                    assert_eq!(recovery.recovered_at, Some(recovered_at));
                    if matches!(
                        action.action(),
                        Action::BlockProducer(BlockProducerAction::WonSlot { .. })
                    ) {
                        won = true;
                    }
                    let produced = state.transition_frontier.best_tip().is_some_and(|tip| {
                        tip.height() > recovered_height
                            && state.block_producer.is_me(tip.producer())
                    });
                    if produced {
                        assert!(won, "must win a new slot after recovery");
                    }
                    produced
                }),
        )
        .await
        .expect("recovered producer must win and produce a later slot");
    let tip = runner
        .node(producer)
        .unwrap()
        .state()
        .transition_frontier
        .best_tip()
        .unwrap();
    eprintln!(
        "producer-recovery produced again height={} slot={} hash={}",
        tip.height(),
        tip.global_slot(),
        tip.hash()
    );
}
