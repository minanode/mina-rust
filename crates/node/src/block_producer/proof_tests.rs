//! Fault injection at the prover and verifier service boundaries. The node's
//! real action guards, reducers and verifier callbacks run in a Redux store;
//! no network, prover, or node process is started.
use std::{collections::VecDeque, sync::Arc};

use mina_core::{block::AppliedBlock, consensus::ConsensusConstants, constants};
use mina_p2p_messages::v2;
use mina_snark::{
    block_verify::{
        SnarkBlockVerifyAction, SnarkBlockVerifyError, SnarkBlockVerifyId, VerifiableBlockWithHash,
    },
    block_verify_effectful::SnarkBlockVerifyService,
    BlockVerifier, SnarkAction, TransactionVerifier, VerifierSRS,
};
use redux::Timestamp;

use super::{
    timing_tests, BlockProducerAction, BlockProducerCurrentState, BlockProducerWonSlotDiscardReason,
};
use crate::{BlockProducerEffectfulAction, State};

type TestStore = crate::Store<TestService>;

struct TestService {
    now: redux::Instant,
    prove_attempts: usize,
    verify_requests: VecDeque<(SnarkBlockVerifyId, VerifiableBlockWithHash)>,
    produced: usize,
    injection_actions: usize,
    discarded: Vec<BlockProducerWonSlotDiscardReason>,
    other_actions: usize,
}

impl redux::TimeService for TestService {
    fn monotonic_time(&mut self) -> redux::Instant {
        self.now
    }
}
impl redux::Service for TestService {}
impl SnarkBlockVerifyService for TestService {
    fn verify_init(
        &mut self,
        req_id: SnarkBlockVerifyId,
        _: BlockVerifier,
        _: Arc<VerifierSRS>,
        block: VerifiableBlockWithHash,
    ) {
        self.verify_requests.push_back((req_id, block));
    }
}

fn effects(store: &mut TestStore, action: crate::ActionWithMeta) {
    use crate::{Action, EventSourceAction};
    let (action, meta) = action.split();
    match action {
        Action::Snark(SnarkAction::BlockVerifyEffect(action)) => action.effects(&meta, store),
        Action::BlockProducerEffectful(BlockProducerEffectfulAction::BlockProveInit) => {
            store.service.prove_attempts += 1;
            store.dispatch(BlockProducerAction::BlockProvePending);
        }
        Action::BlockProducerEffectful(BlockProducerEffectfulAction::BlockProveSuccess) => {
            store.dispatch(BlockProducerAction::BlockProduced);
        }
        Action::BlockProducerEffectful(BlockProducerEffectfulAction::BlockProduced { .. }) => {
            store.service.produced += 1;
        }
        Action::BlockProducerEffectful(BlockProducerEffectfulAction::WonSlotDiscard { reason }) => {
            store.service.discarded.push(reason);
            store.dispatch(BlockProducerAction::WonSlotSearch);
        }
        Action::BlockProducer(
            BlockProducerAction::BlockInject | BlockProducerAction::BlockInjected,
        ) => {
            store.service.injection_actions += 1;
        }
        Action::CheckTimeouts(_) => store.service.other_actions += 1,
        Action::EventSource(EventSourceAction::NewEvent {
            event:
                crate::event_source::Event::BlockProducerEvent(super::BlockProducerEvent::BlockProve(
                    hash,
                    result,
                )),
        }) => {
            let action =
                crate::event_source::block_prove_result_action(store.state(), hash, result);
            store.dispatch(action);
        }
        // A network effect would be an unintended broadcast in these tests.
        Action::P2pEffectful(_) => panic!("unexpected network effect"),
        _ => {}
    }
}

fn store_with_state(state: State) -> TestStore {
    redux::Store::new(
        crate::reducer,
        effects,
        TestService {
            now: redux::Instant::now(),
            prove_attempts: 1,
            verify_requests: VecDeque::new(),
            produced: 0,
            injection_actions: 0,
            discarded: Vec::new(),
            other_actions: 0,
        },
        redux::SystemTime::UNIX_EPOCH,
        state,
    )
}

fn store() -> TestStore {
    let constants = ConsensusConstants::create(
        constants::constraint_constants(),
        &constants::PROTOCOL_CONSTANTS,
    );
    let bp = timing_tests::pending_proof();
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
        transition_frontier: crate::config::TransitionFrontierConfig::new(
            crate::config::DEVNET_CONFIG.clone(),
        ),
        archive: None,
        block_producer: Some(bp.config.clone()),
        global: crate::config::GlobalConfig {
            build: Box::new(crate::config::BuildEnv::get()),
            snarker: None,
            consensus_constants: constants.clone(),
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
    let mut state = State::new(config, &constants, Timestamp::ZERO);
    *state.block_producer.as_mut().unwrap() = bp;
    state
        .block_producer
        .as_mut()
        .unwrap()
        .vrf_evaluator
        .won_slots
        .insert(1, timing_tests::vrf_won_slot(1));
    // A synthetic parent supplies only slot scheduling context, never a crypto oracle.
    let BlockProducerCurrentState::BlockProvePending {
        block, block_hash, ..
    } = &state.block_producer.as_ref().unwrap().current
    else {
        unreachable!()
    };
    let mut parent = block.clone();
    parent.protocol_state.body.constants.genesis_state_timestamp =
        v2::BlockTimeTimeStableV1(0u64.into());
    parent
        .protocol_state
        .body
        .consensus_state
        .curr_global_slot_since_hard_fork
        .slot_number = v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(0u32.into());
    let parent = parent.with_hash_and_proof(block_hash.clone(), proof());
    state.transition_frontier.genesis =
        crate::transition_frontier::genesis::TransitionFrontierGenesisState::ProveSuccess {
            time: Timestamp::ZERO,
            genesis: parent.clone(),
        };
    state.transition_frontier.best_chain.push(AppliedBlock {
        block: parent,
        just_emitted_a_proof: false,
    });
    store_with_state(state)
}

fn proof() -> Arc<v2::MinaBaseProofStableV2> {
    ledger::dummy::dummy_blockchain_proof().clone()
}
fn hash(store: &TestStore) -> v2::StateHash {
    match &store.state().block_producer.as_ref().unwrap().current {
        BlockProducerCurrentState::BlockProvePending { block_hash, .. } => block_hash.clone(),
        _ => panic!("expected a pending proof"),
    }
}

#[test]
fn genesis_prover_error_uses_genesis_error_action() {
    use crate::transition_frontier::{
        genesis::{TransitionFrontierGenesisAction, TransitionFrontierGenesisState},
        TransitionFrontierAction,
    };

    let initial = store();
    let hash = hash(&initial);
    let mut state = initial.state().clone();
    let BlockProducerCurrentState::BlockProvePending {
        block,
        stake_proof_sparse_ledger,
        won_slot,
        ..
    } = &state.block_producer.as_ref().unwrap().current
    else {
        unreachable!()
    };
    let pending = TransitionFrontierGenesisState::ProvePending {
        time: Timestamp::ZERO,
        negative_one: block.protocol_state.clone(),
        genesis: block.protocol_state.clone(),
        genesis_hash: hash.clone(),
        genesis_producer_stake_proof: stake_proof_sparse_ledger.clone(),
        block_with_dummy_proof: block.clone().with_hash_and_proof(hash.clone(), proof()),
        deadline: won_slot.next_slot_time(),
        attempt: 1,
    };
    state.transition_frontier.genesis = pending;
    let mut store = store_with_state(state);
    let action = crate::event_source::block_prove_result_action(
        store.state(),
        hash,
        Err("injected genesis prover failure".to_owned()),
    );
    assert!(matches!(
        action,
        crate::Action::TransitionFrontier(TransitionFrontierAction::Genesis(
            TransitionFrontierGenesisAction::ProveError { .. }
        ))
    ));
    store.dispatch(action);
    assert!(matches!(
        store.state().transition_frontier.genesis,
        TransitionFrontierGenesisState::ProvePending { attempt: 2, .. }
    ));
    assert_eq!(store.service.produced, 0);
}
fn prove_result(store: &mut TestStore, result: Result<Arc<v2::MinaBaseProofStableV2>, String>) {
    let hash = hash(store);
    assert!(store.dispatch(crate::EventSourceAction::NewEvent {
        event: crate::event_source::Event::BlockProducerEvent(
            super::BlockProducerEvent::BlockProve(hash, result)
        ),
    }));
}
fn verify_result(store: &mut TestStore, valid: bool) -> SnarkBlockVerifyId {
    let (req_id, _) = store
        .service
        .verify_requests
        .pop_front()
        .expect("worker request");
    let result = if valid {
        SnarkBlockVerifyAction::Success { req_id }
    } else {
        SnarkBlockVerifyAction::Error {
            req_id,
            error: SnarkBlockVerifyError::VerificationFailed,
        }
    };
    assert!(store.dispatch(result));
    assert!(store.state().snark.block_verify.jobs.is_empty());
    req_id
}
fn assert_discarded_without_broadcast(
    store: &TestStore,
    reason: BlockProducerWonSlotDiscardReason,
) {
    let bp = store.state().block_producer.as_ref().unwrap();
    assert!(matches!(
        bp.current,
        BlockProducerCurrentState::WonSlotDiscarded { .. }
    ));
    assert!(bp.current.won_slot_should_search());
    assert!(!bp.current.is_producing());
    assert!(!bp.vrf_evaluator.won_slots.contains_key(&1));
    assert!(bp.injected_blocks.is_empty());
    assert_eq!(store.service.prove_attempts, 2);
    assert_eq!(store.service.produced, 0);
    assert_eq!(store.service.injection_actions, 0);
    assert_eq!(store.service.discarded, [reason]);
}

#[test]
fn prover_error_then_success_waits_for_worker_verification() {
    let mut store = store();
    prove_result(
        &mut store,
        Err("injected first-attempt prover failure".into()),
    );
    assert_eq!(store.service.prove_attempts, 2);
    prove_result(&mut store, Ok(proof()));
    assert_eq!(store.service.verify_requests.len(), 1);
    assert_eq!(store.service.produced, 0);
    // The service call returned without a verification answer: other actions run.
    assert!(store.dispatch(crate::CheckTimeoutsAction {}));
    assert_eq!(store.service.other_actions, 1);
    assert_eq!(store.service.produced, 0);
    verify_result(&mut store, true);
    assert_eq!(store.service.produced, 1);
    assert!(matches!(
        store.state().block_producer.as_ref().unwrap().current,
        BlockProducerCurrentState::Produced { .. }
    ));
}

#[test]
fn prover_errors_twice_discard_and_allow_a_later_slot() {
    let mut store = store();
    prove_result(
        &mut store,
        Err("injected first-attempt prover failure".into()),
    );
    prove_result(&mut store, Err("injected retry prover failure".into()));
    assert_discarded_without_broadcast(
        &store,
        BlockProducerWonSlotDiscardReason::ProofGenerationFailed,
    );
    assert!(store.service.verify_requests.is_empty());
    assert!(store.dispatch(BlockProducerAction::WonSlot {
        won_slot: timing_tests::won_slot(2)
    }));
    let bp = store.state().block_producer.as_ref().unwrap();
    assert!(matches!(
        bp.current,
        BlockProducerCurrentState::WonSlot { .. }
    ));
    assert!(!bp.proof_retried);
}

#[test]
fn verifier_rejects_twice_without_broadcast_or_a_third_attempt() {
    let mut store = store();
    prove_result(&mut store, Ok(proof()));
    let first = verify_result(&mut store, false);
    assert_eq!(store.service.prove_attempts, 2);
    prove_result(&mut store, Ok(proof()));
    // A duplicate result from the first request cannot validate the retry.
    assert!(!store.dispatch(SnarkBlockVerifyAction::Success { req_id: first }));
    assert_eq!(store.service.produced, 0);
    verify_result(&mut store, false);
    assert_discarded_without_broadcast(
        &store,
        BlockProducerWonSlotDiscardReason::ProofVerificationFailed,
    );
}

#[test]
fn verifier_rejects_once_then_accepts_retry() {
    let mut store = store();
    prove_result(&mut store, Ok(proof()));
    verify_result(&mut store, false);
    prove_result(&mut store, Ok(proof()));
    assert_eq!(store.service.produced, 0);
    verify_result(&mut store, true);
    assert_eq!(store.service.prove_attempts, 2);
    assert_eq!(store.service.produced, 1);
}

#[test]
fn verifier_rejection_then_prover_error_discards_without_broadcast() {
    let mut store = store();
    prove_result(&mut store, Ok(proof()));
    verify_result(&mut store, false);
    prove_result(&mut store, Err("injected retry prover failure".into()));
    assert_discarded_without_broadcast(
        &store,
        BlockProducerWonSlotDiscardReason::ProofGenerationFailed,
    );
}

#[test]
fn stale_prover_and_verifier_results_cannot_advance_a_different_block() {
    let mut store = store();
    let pending_hash = hash(&store);
    assert_ne!(pending_hash, v2::StateHash::zero());
    for result in [Ok(proof()), Err("stale failure".into())] {
        let action = crate::event_source::block_prove_result_action(
            store.state(),
            v2::StateHash::zero(),
            result,
        );
        assert!(!store.dispatch(action));
    }
    assert!(store.service.verify_requests.is_empty());
    assert!(!store.dispatch(BlockProducerAction::BlockProveSuccess {
        block_hash: pending_hash
    }));
    prove_result(&mut store, Ok(proof()));
    let (req_id, _) = store.service.verify_requests.pop_front().unwrap();
    let mut state = store.state().clone();
    let BlockProducerCurrentState::BlockProvePending { block_hash, .. } =
        &mut state.block_producer.as_mut().unwrap().current
    else {
        unreachable!()
    };
    *block_hash = v2::StateHash::zero();
    let mut store = store_with_state(state);
    assert!(store.dispatch(SnarkBlockVerifyAction::Success { req_id }));
    assert_eq!(store.service.produced, 0);
    assert_eq!(store.service.prove_attempts, 1);
    assert_eq!(hash(&store), v2::StateHash::zero());
    assert!(store.state().snark.block_verify.jobs.is_empty());
}
