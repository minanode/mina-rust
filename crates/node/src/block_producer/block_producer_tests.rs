//! State machine tests of the block producer around a block proof: when the
//! genesis block is proved for a won slot, what a failed block proof does to
//! its won slot, and which action the block prover's result becomes.
//!
//! Each test builds a node's [`State`] with a made-up genesis block as its
//! best tip, runs the reducer on an action and checks the state and the
//! enabling conditions of what follows.

use mina_core::{
    block::{AppliedBlock, ArcBlockWithHash},
    consensus::ConsensusConstants,
    constants::{constraint_constants, PROTOCOL_CONSTANTS, PROTOCOL_VERSION},
};
use mina_p2p_messages::{bigint::BigInt, v2};
use redux::{ActionMeta, Dispatcher, EnablingCondition, Timestamp};

use super::{
    vrf_evaluator::VrfWonSlotWithHash, BlockProducerAction, BlockProducerConfig,
    BlockProducerCurrentState, BlockProducerWonSlot, BlockProducerWonSlotDiscardReason,
    BlockWithoutProof,
};
use crate::{
    account::AccountSecretKey,
    event_source::block_prove_result_action,
    ledger::LedgerConfig,
    p2p::{identity::SecretKey as P2pSecretKey, P2pConfig, P2pMeshsubConfig},
    snark::{get_srs, BlockVerifier, SnarkConfig, TransactionVerifier},
    transition_frontier::{
        genesis::{
            genesis_block_prover_input, GenesisConfigLoaded, TransitionFrontierGenesisAction,
            TransitionFrontierGenesisState,
        },
        TransitionFrontierAction, TransitionFrontierConfig,
    },
    Action, BuildEnv, Config, GlobalConfig, State, DEVNET_CONFIG,
};

/// A genesis for a genesis config with made-up ledger hashes, currencies and
/// seeds, as the genesis state's tests build it: the values don't matter
/// here. Its global slot is 0.
struct Genesis {
    negative_one: v2::MinaStateProtocolStateValueStableV2,
    state: v2::MinaStateProtocolStateValueStableV2,
    hash: v2::StateHash,
    stake_proof: v2::MinaBaseSparseLedgerBaseStableV2,
    block: ArcBlockWithHash,
}

fn ledger_hash() -> v2::LedgerHash {
    "jwrCSCRNpNx3ZBh6D4bUAnZtqPacKXXe6veGC9kQfHrxWqmEbJS"
        .parse()
        .unwrap()
}

fn genesis() -> Genesis {
    let amount = |nanomina: u64| v2::CurrencyAmountStableV1(nanomina.into());
    let seed: v2::EpochSeed = "2vamERQZWT1miAnrxLJsjPmZmoCSemrz9JugjN8weQJeqhU7ZD38"
        .parse()
        .unwrap();
    let loaded = GenesisConfigLoaded {
        runtime_config: Default::default(),
        constants: PROTOCOL_CONSTANTS.clone(),
        genesis_ledger_hash: ledger_hash(),
        genesis_total_currency: amount(1_000_000_000),
        genesis_producer_stake_proof: v2::MinaBaseSparseLedgerBaseStableV2 {
            indexes: std::iter::empty().collect(),
            depth: 35u64.into(),
            tree: v2::MinaBaseSparseLedgerBaseStableV2Tree::Hash(ledger_hash()),
        },
        staking_epoch_ledger_hash: ledger_hash(),
        staking_epoch_total_currency: amount(2_000_000_000),
        staking_epoch_seed: seed.clone(),
        next_epoch_ledger_hash: ledger_hash(),
        next_epoch_total_currency: amount(3_000_000_000),
        next_epoch_seed: seed,
    };
    let (negative_one, state, hash) = loaded
        .protocol_states(
            loaded.genesis_total_currency.clone(),
            loaded.staking_epoch_total_currency.clone(),
            loaded.next_epoch_total_currency.clone(),
        )
        .unwrap();
    let block =
        TransitionFrontierGenesisState::build_block_with_dummy_proof(&state, &hash).unwrap();
    assert_eq!(block.global_slot(), 0);
    Genesis {
        negative_one,
        state,
        hash,
        stake_proof: loaded.genesis_producer_stake_proof,
        block,
    }
}

impl Genesis {
    /// The genesis produced, not proved yet: what a block producer waits in
    /// until it proves the genesis block for its first won slot.
    fn produced(&self) -> TransitionFrontierGenesisState {
        TransitionFrontierGenesisState::Produced {
            time: Timestamp::ZERO,
            negative_one: self.negative_one.clone(),
            genesis: self.state.clone(),
            genesis_hash: self.hash.clone(),
            chain_id_genesis_hash: self.hash.clone(),
            genesis_producer_stake_proof: self.stake_proof.clone(),
            block_with_dummy_proof: self.block.clone(),
        }
    }

    fn prove_pending(&self) -> TransitionFrontierGenesisState {
        TransitionFrontierGenesisState::ProvePending {
            time: Timestamp::ZERO,
            negative_one: self.negative_one.clone(),
            genesis: self.state.clone(),
            genesis_hash: self.hash.clone(),
            genesis_producer_stake_proof: self.stake_proof.clone(),
            block_with_dummy_proof: self.block.clone(),
            deadline: self.won_slot(3).next_slot_time(),
            attempt: 1,
        }
    }

    /// When global slot `slot` starts.
    fn slot_time(&self, slot: u32) -> Timestamp {
        BlockProducerWonSlot::from_vrf_won_slot(&vrf_won_slot(slot), self.block.genesis_timestamp())
            .slot_time
    }

    fn won_slot(&self, slot: u32) -> BlockProducerWonSlot {
        BlockProducerWonSlot::from_vrf_won_slot(&vrf_won_slot(slot), self.block.genesis_timestamp())
    }
}

/// The genesis winner's win of global slot `slot`.
fn vrf_won_slot(slot: u32) -> VrfWonSlotWithHash {
    let producer = AccountSecretKey::genesis_producer().public_key();
    let seed = v2::EpochSeed::from(v2::MinaBaseEpochSeedStableV1(BigInt::zero()));
    VrfWonSlotWithHash::new(
        mina_vrf::VrfWonSlot {
            producer: producer.clone(),
            winner_account: producer,
            global_slot: slot,
            account_index: ledger::AccountIndex(0),
            vrf_output: Box::new(mina_vrf::genesis_vrf(seed).unwrap()),
            value_with_threshold: None,
        },
        ledger_hash(),
    )
}

/// A block producing node at `now`, the genesis winner, with `genesis` as its
/// genesis state and best tip and wins of `won_slots`. Its block producer is
/// idle.
fn node(genesis: &Genesis, now: Timestamp, won_slots: &[u32]) -> State {
    let consensus_constants =
        ConsensusConstants::create(constraint_constants(), &PROTOCOL_CONSTANTS);
    let config = Config {
        ledger: LedgerConfig {},
        snark: SnarkConfig {
            block_verifier_index: BlockVerifier::make(),
            block_verifier_srs: get_srs(),
            work_verifier_index: TransactionVerifier::make(),
            work_verifier_srs: get_srs(),
        },
        p2p: P2pConfig {
            libp2p_port: None,
            listen_port: None,
            identity_pub_key: P2pSecretKey::deterministic(0).public_key(),
            initial_peers: Vec::new(),
            external_addrs: Vec::new(),
            enabled_channels: Default::default(),
            timeouts: Default::default(),
            limits: Default::default(),
            peer_discovery: false,
            meshsub: P2pMeshsubConfig::default(),
            catchup_rpc: Default::default(),
        },
        transition_frontier: TransitionFrontierConfig::new(DEVNET_CONFIG.clone()),
        archive: None,
        block_producer: Some(BlockProducerConfig::new(
            AccountSecretKey::genesis_producer().public_key().into(),
        )),
        global: GlobalConfig {
            build: BuildEnv::get().into(),
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
            pool_max_size: 3000,
            slot_tx_end: None,
        },
    };
    let mut state = State::new(config, &consensus_constants, now);
    state.transition_frontier.genesis = genesis.produced();
    state.transition_frontier.best_chain = vec![AppliedBlock {
        block: genesis.block.clone(),
        just_emitted_a_proof: false,
    }];
    let producer = state.block_producer.as_mut().unwrap();
    producer.vrf_evaluator.genesis_timestamp = genesis.block.genesis_timestamp();
    for &slot in won_slots {
        producer
            .vrf_evaluator
            .won_slots
            .insert(slot, vrf_won_slot(slot));
    }
    state
}

/// `offset_s` seconds after `time`, or before if negative.
fn at(time: Timestamp, offset_s: i64) -> Timestamp {
    let offset_ns = offset_s.checked_mul(1_000_000_000).unwrap();
    Timestamp::new(u64::from(time).checked_add_signed(offset_ns).unwrap())
}

/// Runs the reducer on `action` at `now`, as the store does once the action
/// is enabled. What the reducer dispatches is not run.
fn reduce(state: &mut State, now: Timestamp, action: impl Into<Action>) {
    let action = action.into();
    assert!(action.is_enabled(state, now), "not enabled: {action:?}");
    crate::reducer(
        state,
        &ActionMeta::zero_custom(now).with_action(action),
        &mut Dispatcher::new(),
    );
}

/// A producer proves the genesis block from two minutes before its first won
/// slot until that slot ends, waiting for it or not. With 90 s slots this
/// stopped 30 s before the slot started: a producer whose first won slot was
/// the current one, or started within 30 s, never proved the genesis block,
/// so it never produced.
#[test]
fn the_genesis_is_proved_for_a_won_slot_until_it_ends() {
    assert_eq!(constraint_constants().block_window_duration_ms, 90_000);
    let genesis = genesis();
    let slot = 3;
    let start = genesis.slot_time(slot);

    for (offset_s, proves) in [
        (-130, false),
        (-120, true),
        (-60, true),
        (-10, true),
        (0, true),
        (10, true),
        (89, true),
        (90, false),
    ] {
        let now = at(start, offset_s);
        let mut state = node(&genesis, now, &[slot]);
        for current in [
            BlockProducerCurrentState::WonSlot {
                time: now,
                won_slot: genesis.won_slot(slot),
            },
            BlockProducerCurrentState::WonSlotWait {
                time: now,
                won_slot: genesis.won_slot(slot),
            },
        ] {
            state.block_producer.as_mut().unwrap().current = current;
            assert_eq!(
                state.producing_block_after_genesis(),
                proves,
                "{offset_s} s from the won slot"
            );
            assert_eq!(
                TransitionFrontierGenesisAction::ProveInit.is_enabled(&state, now),
                proves,
                "{offset_s} s from the won slot"
            );
        }
    }
}

/// The producer, at `now`, proving its block for won slot `slot`.
fn proving(genesis: &Genesis, now: Timestamp, slot: u32) -> BlockProducerCurrentState {
    let input =
        genesis_block_prover_input(&genesis.negative_one, &genesis.state, &genesis.stake_proof);
    BlockProducerCurrentState::BlockProvePending {
        time: now,
        won_slot: genesis.won_slot(slot),
        chain: vec![AppliedBlock {
            block: genesis.block.clone(),
            just_emitted_a_proof: false,
        }],
        emitted_ledger_proof: None,
        pending_coinbase_update: input.block.pending_coinbase_update,
        pending_coinbase_witness: input.pending_coinbase,
        stake_proof_sparse_ledger: genesis.stake_proof.clone(),
        block: BlockWithoutProof {
            protocol_state: genesis.state.clone(),
            delta_block_chain_proof: (genesis.hash.clone(), std::iter::empty().collect()),
            current_protocol_version: PROTOCOL_VERSION,
            proposed_protocol_version_opt: None,
            body: genesis.block.body().clone(),
        },
        block_hash: genesis.hash.clone(),
        verification_req_id: None,
    }
}

/// A failed block proof discards its won slot (`ProofGenerationFailed`), and the
/// producer does not take that slot again while it lasts: it would build and
/// prove the same block again, twice each time, until the slot ended. The
/// next won slot is still found.
#[test]
fn a_failed_block_proof_discards_its_won_slot_for_good() {
    let genesis = genesis();
    let (slot, next) = (3, 6);
    let now = at(genesis.slot_time(slot), 10);
    let mut state = node(&genesis, now, &[slot, next]);
    assert_eq!(state.cur_global_slot(), Some(slot));
    state.block_producer.as_mut().unwrap().current = proving(&genesis, now, slot);
    state.block_producer.as_mut().unwrap().proof_retried = true;
    reduce(
        &mut state,
        now,
        BlockProducerAction::BlockProveError {
            block_hash: genesis.hash.clone(),
        },
    );
    assert!(!state
        .block_producer
        .as_ref()
        .unwrap()
        .vrf_evaluator
        .won_slots
        .contains_key(&slot));
    let current = &state.block_producer.as_ref().unwrap().current;
    let BlockProducerCurrentState::WonSlotDiscarded {
        won_slot, reason, ..
    } = current
    else {
        panic!("{current:?}");
    };
    assert_eq!(won_slot.global_slot(), slot);
    assert_eq!(
        *reason,
        BlockProducerWonSlotDiscardReason::ProofGenerationFailed
    );

    let best_tip = genesis.block.clone();
    let next_won_slot = |state: &State| {
        state
            .block_producer
            .with(None, |bp| bp.next_won_slot(slot, &best_tip))
            .map(|won_slot| won_slot.global_slot())
    };
    assert_eq!(next_won_slot(&state), Some(next));
    assert!(BlockProducerAction::WonSlotSearch.is_enabled(&state, now));
    let won_slot = |slot| BlockProducerAction::WonSlot {
        won_slot: genesis.won_slot(slot),
    };
    // The failed slot is gone from the VRF cache, so WonSlotSearch cannot
    // find it, and the discarded state starts the search after it, so even a
    // WonSlot action for it is refused while the slot lasts.
    assert!(!won_slot(slot).is_enabled(&state, now));
    assert!(won_slot(next).is_enabled(&state, now));

    // With no other won slot, there is nothing to search for.
    let producer = state.block_producer.as_mut().unwrap();
    producer.vrf_evaluator.won_slots.remove(&next);
    assert_eq!(next_won_slot(&state), None);
    assert!(!BlockProducerAction::WonSlotSearch.is_enabled(&state, now));

    // A slot discarded for its staking ledger is not skipped: the best tip
    // that discarded it decides. One whose block lost to the best tip, or
    // that ended before its block was started, is.
    let producer = state.block_producer.as_mut().unwrap();
    producer
        .vrf_evaluator
        .won_slots
        .insert(slot, vrf_won_slot(slot));
    let discarded = |reason| BlockProducerCurrentState::WonSlotDiscarded {
        time: now,
        won_slot: genesis.won_slot(slot),
        reason,
    };
    producer.current = discarded(BlockProducerWonSlotDiscardReason::BestTipStakingLedgerDifferent);
    assert_eq!(next_won_slot(&state), Some(slot));
    for reason in [
        BlockProducerWonSlotDiscardReason::BestTipSuperior,
        BlockProducerWonSlotDiscardReason::WonSlotExpired,
    ] {
        state.block_producer.as_mut().unwrap().current = discarded(reason);
        assert_eq!(next_won_slot(&state), None, "{reason:?}");
    }
}

/// The genesis block made a block of global slot `slot` and height
/// `height`, with truncated VRF output `vrf` and state hash `hash`.
fn block_at(
    genesis: &Genesis,
    slot: u32,
    height: u32,
    vrf: &v2::ConsensusVrfOutputTruncatedStableV1,
    hash: &v2::StateHash,
) -> ArcBlockWithHash {
    let mut block = genesis.block.clone();
    set_consensus(
        &mut std::sync::Arc::make_mut(&mut block.block)
            .header
            .protocol_state
            .body
            .consensus_state,
        slot,
        height,
        vrf,
    );
    block.hash = hash.clone();
    block
}

fn set_consensus(
    state: &mut v2::ConsensusProofOfStakeDataConsensusStateValueStableV2,
    slot: u32,
    height: u32,
    vrf: &v2::ConsensusVrfOutputTruncatedStableV1,
) {
    state.curr_global_slot_since_hard_fork.slot_number =
        v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(slot.into());
    state.blockchain_length = height.into();
    state.last_vrf_output = vrf.clone();
}

/// A truncated VRF output whose Blake2b digest, which selection compares, is
/// greater than `than`'s, or smaller.
fn vrf_output(
    than: &v2::ConsensusVrfOutputTruncatedStableV1,
    greater: bool,
) -> v2::ConsensusVrfOutputTruncatedStableV1 {
    (0u8..)
        .map(|byte| v2::ConsensusVrfOutputTruncatedStableV1(vec![byte; 32].into()))
        .find(|vrf| (vrf.blake2b() > than.blake2b()) == greater)
        .unwrap()
}

/// Two state hashes, the smaller first.
fn state_hashes() -> (v2::StateHash, v2::StateHash) {
    let a: v2::StateHash = "3NKY1kxHMRfjBbjfAA5fsasUCWFF9B7YqYFfNH4JFku6ZCUUXyLG"
        .parse()
        .unwrap();
    let b: v2::StateHash = "3NLFoBQ6y3nku79LQqPgKBmuo5Ngnpr7rfZygzdRrcPtz2gewRFC"
        .parse()
        .unwrap();
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

/// A win of the best tip's slot with the same VRF output, as a node sharing
/// our key has when it made the best tip, is tried: our block, on the best
/// tip's parent, has the same height and VRF output, and the greater state
/// hash wins (proof_of_stake.ml:2842-2859). OCaml makes it
/// (block_producer.ml:715-747). The search used to skip it. A greater VRF
/// output still rules the slot out, and once our block is injected the slot
/// is not tried again, though the best tip, ours, has the same VRF output.
#[test]
fn an_equal_vrf_win_of_the_best_tips_slot_is_tried() {
    let genesis = genesis();
    let (slot, next) = (3, 6);
    let now = at(genesis.slot_time(slot), 10);
    let won_slot = genesis.won_slot(slot);
    let ours = v2::ConsensusVrfOutputTruncatedStableV1::from(&*won_slot.vrf_output);
    let (low, high) = state_hashes();
    let tip = block_at(&genesis, slot, 2, &ours, &low);
    let mut state = node(&genesis, now, &[slot, next]);
    state.transition_frontier.best_chain = vec![AppliedBlock {
        block: tip.clone(),
        just_emitted_a_proof: false,
    }];
    let next_won_slot = |state: &State, tip: &ArcBlockWithHash| {
        state
            .block_producer
            .with(None, |bp| bp.next_won_slot(slot, tip))
            .map(|won_slot| won_slot.global_slot())
    };

    assert_eq!(next_won_slot(&state, &tip), Some(slot));
    assert!(BlockProducerAction::WonSlotSearch.is_enabled(&state, now));
    let won = BlockProducerAction::WonSlot {
        won_slot: won_slot.clone(),
    };
    assert!(won.is_enabled(&state, now));
    let mut candidate = tip.consensus_state().clone();
    set_consensus(&mut candidate, slot, 2, &ours);
    assert!(mina_core::consensus::consensus_take(
        tip.consensus_state(),
        &candidate,
        tip.hash(),
        &high,
    ));

    let better = block_at(&genesis, slot, 2, &vrf_output(&ours, true), &low);
    assert_eq!(next_won_slot(&state, &better), Some(next));

    state.block_producer.as_mut().unwrap().current = BlockProducerCurrentState::Injected {
        time: now,
        won_slot,
        chain: vec![],
        block: tip.clone(),
    };
    assert_eq!(next_won_slot(&state, &tip), Some(next));
    assert!(!won.is_enabled(&state, now));
}

/// Whether our block beats the best tip is decided once its consensus state
/// exists, by selection: a best tip of our slot with a greater VRF output
/// does not end a waiting win, and a taller block of ours beats it. Both
/// used to be discarded on the slot and VRF output alone. As tall, our block
/// loses on the VRF output and is discarded.
#[test]
fn a_win_is_kept_until_its_block_shows_whether_it_beats_the_best_tip() {
    let genesis = genesis();
    let slot = 3;
    let now = at(genesis.slot_time(slot), 10);
    let won_slot = genesis.won_slot(slot);
    let ours = v2::ConsensusVrfOutputTruncatedStableV1::from(&*won_slot.vrf_output);
    let (low, high) = state_hashes();
    let tip = block_at(&genesis, slot, 2, &vrf_output(&ours, true), &low);

    let waiting = BlockProducerCurrentState::WonSlotWait {
        time: now,
        won_slot,
    };
    assert_eq!(waiting.won_slot_should_discard(&tip, now), None);

    let mut built = proving(&genesis, now, slot);
    let with_height = |built: &mut BlockProducerCurrentState, height| {
        let BlockProducerCurrentState::BlockProvePending {
            block, block_hash, ..
        } = built
        else {
            unreachable!()
        };
        set_consensus(
            &mut block.protocol_state.body.consensus_state,
            slot,
            height,
            &ours,
        );
        *block_hash = high.clone();
    };
    with_height(&mut built, 3);
    assert_eq!(built.won_slot_should_discard(&tip, now), None);
    with_height(&mut built, 2);
    assert_eq!(
        built.won_slot_should_discard(&tip, now),
        Some(BlockProducerWonSlotDiscardReason::BestTipSuperior)
    );
}

/// A win whose slot ends before its block is started is given up, so the
/// search goes on to the next one. It used to stay in `WonSlotWait`, which
/// neither produces nor searches, until a best tip discarded it.
/// OCaml skips a won slot that has passed and checks again
/// (block_producer.ml:1174-1191).
#[test]
fn an_expired_waiting_win_makes_way_for_the_next() {
    let genesis = genesis();
    let (slot, next) = (3, 5);
    let end = genesis.won_slot(slot).next_slot_time();
    let mut state = node(&genesis, end, &[slot, next]);
    state.block_producer.as_mut().unwrap().current = BlockProducerCurrentState::WonSlotWait {
        time: genesis.slot_time(slot),
        won_slot: genesis.won_slot(slot),
    };
    let expire = BlockProducerAction::WonSlotDiscard {
        reason: BlockProducerWonSlotDiscardReason::WonSlotExpired,
    };
    assert!(!expire.is_enabled(&state, at(end, -1)));
    assert!(!BlockProducerAction::WonSlotProduceInit.is_enabled(&state, end));
    assert!(!BlockProducerAction::WonSlotSearch.is_enabled(&state, end));

    reduce(&mut state, end, expire);
    let current = &state.block_producer.as_ref().unwrap().current;
    assert!(
        matches!(
            current,
            BlockProducerCurrentState::WonSlotDiscarded {
                reason: BlockProducerWonSlotDiscardReason::WonSlotExpired,
                ..
            }
        ),
        "{current:?}"
    );
    assert!(BlockProducerAction::WonSlotSearch.is_enabled(&state, end));
    let found = state.block_producer.with(None, |bp| {
        bp.next_won_slot(state.cur_global_slot().unwrap(), &genesis.block)
    });
    assert_eq!(found.map(|won_slot| won_slot.global_slot()), Some(next));
}

/// The block prover's result goes to the genesis block while it is being
/// proved, and to the produced block otherwise; a failure becomes an error
/// action (it was a `todo!`), unless proof verification is off, where it
/// becomes a dummy proof.
#[test]
fn a_block_proof_result_goes_to_its_block() {
    let genesis = genesis();
    let now = at(genesis.slot_time(3), 10);
    let mut state = node(&genesis, now, &[]);
    state.transition_frontier.genesis = genesis.prove_pending();
    let other: v2::StateHash = "3NKLvHFnXhZw3mzUDz8ZdaicqVGMHEuoUYzB4YxjzKrkXLg2nYNr"
        .parse()
        .unwrap();
    assert_ne!(other, genesis.hash);
    let proof = ledger::dummy::dummy_blockchain_proof().clone();
    let failed = || Err("block proof failed 2 times: kimchi error".to_owned());

    let action = block_prove_result_action(&state, genesis.hash.clone(), failed());
    let Action::TransitionFrontier(TransitionFrontierAction::Genesis(
        TransitionFrontierGenesisAction::ProveError { error },
    )) = action
    else {
        panic!("{action:?}");
    };
    assert_eq!(
        error,
        format!(
            "block {}: block proof failed 2 times: kimchi error",
            genesis.hash
        )
    );

    let action = block_prove_result_action(&state, other.clone(), failed());
    let Action::BlockProducer(BlockProducerAction::BlockProveError { block_hash }) = action else {
        panic!("{action:?}");
    };
    assert_eq!(block_hash, other);

    let action = block_prove_result_action(&state, genesis.hash.clone(), Ok(proof.clone()));
    assert!(
        matches!(
            action,
            Action::TransitionFrontier(TransitionFrontierAction::Genesis(
                TransitionFrontierGenesisAction::ProveSuccess { .. }
            ))
        ),
        "{action:?}"
    );
    let action = block_prove_result_action(&state, other.clone(), Ok(proof.clone()));
    assert!(
        matches!(
            action,
            Action::BlockProducer(BlockProducerAction::BlockProofVerify { .. })
        ),
        "{action:?}"
    );

    state.config.skip_proof_verification = true;
    let action = block_prove_result_action(&state, other.clone(), failed());
    let Action::BlockProducer(BlockProducerAction::BlockProofVerify { proof: dummy, .. }) = action
    else {
        panic!("{action:?}");
    };
    assert_eq!(dummy, proof);
}

/// A real Redux store: all guards/reducers and their dispatched actions run.
/// Only the prover service is replaced, to inject failures at its boundary.
struct GenesisTestService {
    now: redux::Instant,
    requests: Vec<(v2::StateHash, Timestamp)>,
}
impl redux::TimeService for GenesisTestService {
    fn monotonic_time(&mut self) -> redux::Instant {
        self.now
    }
}
impl redux::Service for GenesisTestService {}
fn genesis_effects(store: &mut crate::Store<GenesisTestService>, action: crate::ActionWithMeta) {
    use crate::transition_frontier::genesis_effectful::TransitionFrontierGenesisEffectfulAction;
    match action.action() {
        Action::TransitionFrontier(TransitionFrontierAction::GenesisEffect(
            TransitionFrontierGenesisEffectfulAction::ProveInit {
                block_hash,
                deadline,
                ..
            },
        )) => store.service.requests.push((block_hash.clone(), *deadline)),
        Action::EventSource(crate::EventSourceAction::NewEvent {
            event:
                crate::event_source::Event::BlockProducerEvent(super::BlockProducerEvent::BlockProve(
                    hash,
                    result,
                )),
        }) => {
            let action = block_prove_result_action(store.state(), hash.clone(), result.clone());
            store.dispatch(action);
        }
        _ => {}
    }
}
fn genesis_store() -> (Genesis, crate::Store<GenesisTestService>) {
    let genesis = genesis();
    let now = genesis.slot_time(3);
    let mut state = node(&genesis, now, &[3]);
    state.block_producer.as_mut().unwrap().current = BlockProducerCurrentState::WonSlot {
        time: now,
        won_slot: genesis.won_slot(3),
    };
    let mut store = redux::Store::new(
        crate::reducer,
        genesis_effects,
        GenesisTestService {
            now: redux::Instant::now(),
            requests: vec![],
        },
        redux::SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(u64::from(now)),
        state,
    );
    assert!(store.dispatch(TransitionFrontierGenesisAction::ProveInit));
    (genesis, store)
}
fn genesis_result(
    store: &mut crate::Store<GenesisTestService>,
    hash: &v2::StateHash,
    result: Result<std::sync::Arc<v2::MinaBaseProofStableV2>, String>,
) {
    assert!(store.dispatch(crate::EventSourceAction::NewEvent {
        event: crate::event_source::Event::BlockProducerEvent(
            super::BlockProducerEvent::BlockProve(hash.clone(), result)
        ),
    }));
}

#[test]
fn genesis_iframe_death_and_proof_failure_retry_once_then_give_up() {
    // The bridge has the same Err boundary for host death, prover failure,
    // invalid proofs and expired work. Exercise each on both attempts.
    for error in [
        "prover iframe removed",
        "genesis proof generation failed",
        "the isolated prover's proof does not verify",
    ] {
        let (genesis, mut store) = genesis_store();
        assert_eq!(store.service.requests.len(), 1);
        let initial = store.state().genesis_block().unwrap();
        assert!(store.state().cur_global_slot().is_some());
        assert!(!store.dispatch(BlockProducerAction::WonSlotProduceInit));
        genesis_result(&mut store, &genesis.hash, Err(error.to_owned()));
        assert_eq!(store.service.requests.len(), 2);
        assert_eq!(store.service.requests[0], store.service.requests[1]);
        assert!(matches!(
            store.state().transition_frontier.genesis,
            TransitionFrontierGenesisState::ProvePending { attempt: 2, .. }
        ));
        genesis_result(&mut store, &genesis.hash, Err(error.to_owned()));
        assert!(matches!(
            store.state().transition_frontier.genesis,
            TransitionFrontierGenesisState::ProveError { attempts: 2, .. }
        ));
        assert!(std::sync::Arc::ptr_eq(
            &initial.block,
            &store.state().genesis_block().unwrap().block
        ));
        assert!(store.state().cur_global_slot().is_some());
        assert!(!store.dispatch(TransitionFrontierGenesisAction::ProveInit));
        assert!(!store.dispatch(BlockProducerAction::WonSlotProduceInit));
        // Duplicate/late completions cannot restart or replace the failed genesis.
        genesis_result(
            &mut store,
            &genesis.hash,
            Ok(ledger::dummy::dummy_blockchain_proof().clone()),
        );
        assert_eq!(store.service.requests.len(), 2);
        assert!(matches!(
            store.state().transition_frontier.genesis,
            TransitionFrontierGenesisState::ProveError { .. }
        ));
    }
}

#[test]
fn genesis_first_or_retry_failure_after_slot_end_does_not_retry() {
    for retry in [false, true] {
        let (genesis, mut store) = genesis_store();
        if retry {
            genesis_result(&mut store, &genesis.hash, Err("first failure".into()));
        }
        store.service.now += std::time::Duration::from_secs(90);
        // Advance the store clock before receiving the event.
        store.dispatch(crate::CheckTimeoutsAction {});
        genesis_result(
            &mut store,
            &genesis.hash,
            Err("iframe died at deadline".into()),
        );
        assert!(matches!(
            store.state().transition_frontier.genesis,
            TransitionFrontierGenesisState::ProveError { .. }
        ));
        assert_eq!(store.service.requests.len(), if retry { 2 } else { 1 });
        assert!(store.state().genesis_block().is_some());
    }
}

#[test]
fn genesis_retry_success_finishes_and_stale_failure_is_ignored() {
    let (genesis, mut store) = genesis_store();
    genesis_result(
        &mut store,
        &genesis.hash,
        Err("prover iframe removed".into()),
    );
    // The service boundary already checked the proof; this is a state-machine
    // test, not a cryptographic oracle. Browser reruns verify real genesis proofs.
    genesis_result(
        &mut store,
        &genesis.hash,
        Ok(ledger::dummy::dummy_blockchain_proof().clone()),
    );
    assert!(store
        .state()
        .transition_frontier
        .genesis
        .proven_block()
        .is_some());
    genesis_result(&mut store, &genesis.hash, Err("stale host failure".into()));
    assert!(store
        .state()
        .transition_frontier
        .genesis
        .proven_block()
        .is_some());
    assert_eq!(store.service.requests.len(), 2);
}

/// OCaml finishes the proof uninterruptibly and adopts it even after the slot.
/// Peers recover the retained block by RPC; gossip freshness is independent.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L853-L1026>
#[test]
fn a_successful_late_proof_can_be_adopted_for_rpc_catchup() {
    let genesis = genesis();
    let slot = 3;
    let now = at(genesis.won_slot(slot).next_slot_time(), 40);
    let mut state = node(&genesis, now, &[slot, slot + 2]);
    let BlockProducerCurrentState::BlockProvePending {
        won_slot,
        chain,
        mut block,
        ..
    } = proving(&genesis, now, slot)
    else {
        unreachable!()
    };
    block
        .protocol_state
        .body
        .consensus_state
        .curr_global_slot_since_hard_fork
        .slot_number = v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(slot.into());
    block.protocol_state.body.consensus_state.blockchain_length =
        (genesis.block.height() + 1).into();
    block.protocol_state.body.blockchain_state.timestamp.0 =
        (u64::from(genesis.slot_time(slot)) / 1_000_000).into();
    block.protocol_state.previous_state_hash = genesis.hash.clone();
    let block_hash = block.protocol_state.try_hash().unwrap();
    state.block_producer.as_mut().unwrap().current = BlockProducerCurrentState::BlockProveSuccess {
        time: now,
        won_slot,
        chain,
        block,
        block_hash,
        proof: genesis.block.header().protocol_state_proof.clone(),
    };
    assert!(!BlockProducerAction::WonSlotDiscard {
        reason: BlockProducerWonSlotDiscardReason::WonSlotExpired,
    }
    .is_enabled(&state, now));
    reduce(&mut state, now, BlockProducerAction::BlockProduced);
    assert_eq!(state.block_production_delay_buckets, [0, 0, 0, 1, 0, 0, 0]);
    assert_eq!(state.block_production_delay_sum_ms, 130_000);
    assert!(BlockProducerAction::BlockInject.is_enabled(&state, now));
    reduce(&mut state, now, BlockProducerAction::BlockInjected);
    assert!(matches!(
        state.block_producer.as_ref().unwrap().current,
        BlockProducerCurrentState::Injected { .. }
    ));
}

/// OCaml skips a won slot that has passed and checks the next queued win.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L1173-L1192>
#[test]
fn an_expired_unstarted_slot_is_discarded_and_the_next_win_is_found() {
    let genesis = genesis();
    let (slot, next) = (3, 15);
    for offset in [89, 90, 720] {
        let now = at(genesis.slot_time(slot), offset);
        for waiting in [false, true] {
            let mut state = node(&genesis, now, &[slot, next]);
            let won_slot = genesis.won_slot(slot);
            state.block_producer.as_mut().unwrap().current = if waiting {
                BlockProducerCurrentState::WonSlotWait {
                    time: now,
                    won_slot,
                }
            } else {
                BlockProducerCurrentState::WonSlot {
                    time: now,
                    won_slot,
                }
            };
            let discard = BlockProducerAction::WonSlotDiscard {
                reason: BlockProducerWonSlotDiscardReason::WonSlotExpired,
            };
            assert_eq!(discard.is_enabled(&state, now), offset >= 90);
            if offset < 90 {
                continue;
            }
            reduce(&mut state, now, discard);
            assert!(BlockProducerAction::WonSlotSearch.is_enabled(&state, now));
            let found = state
                .block_producer
                .with(None, |bp| {
                    bp.next_won_slot(state.cur_global_slot().unwrap(), &genesis.block)
                })
                .unwrap();
            assert_eq!(found.global_slot(), next);
            reduce(
                &mut state,
                now,
                BlockProducerAction::WonSlot { won_slot: found },
            );
            assert_eq!(
                state
                    .block_producer
                    .as_ref()
                    .unwrap()
                    .current
                    .won_slot()
                    .unwrap()
                    .global_slot(),
                next
            );

            // Expiration must never cancel proof work that already started.
            state.block_producer.as_mut().unwrap().current = proving(&genesis, now, slot);
            assert!(!BlockProducerAction::WonSlotDiscard {
                reason: BlockProducerWonSlotDiscardReason::WonSlotExpired,
            }
            .is_enabled(&state, now));
        }
    }
}

/// OCaml set_next_producer_timing captures the source consensus state once.
/// A new tip while the producer waits for the scheduled slot does not change
/// that source, even when status is read again.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L1371-L1427>
#[test]
fn producer_timing_keeps_the_consensus_point_until_rescheduled() {
    let genesis = genesis();
    let now = at(genesis.slot_time(1), 10);
    let mut state = node(&genesis, now, &[3, 6]);
    assert_eq!(state.block_producer.next_block_production(), None);
    // Run the real search and scheduling reducers, not a prepared RPC status.
    reduce(&mut state, now, BlockProducerAction::WonSlotSearch);
    reduce(
        &mut state,
        now,
        BlockProducerAction::WonSlot {
            won_slot: genesis.won_slot(3),
        },
    );
    reduce(&mut state, now, BlockProducerAction::WonSlotWait);
    let timing = state.block_producer.next_block_production().unwrap();
    assert_eq!(timing.generated_from_consensus_at.global_slot, 0);
    assert_eq!(timing.next_won_slot.as_ref().unwrap().global_slot, 3);

    let mut new_tip = (*genesis.block.block).clone();
    let consensus = &mut new_tip.header.protocol_state.body.consensus_state;
    consensus.curr_global_slot_since_hard_fork.slot_number =
        v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(1u32.into());
    consensus.global_slot_since_genesis =
        v2::MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(1u32.into());
    let new_tip = mina_core::block::BlockWithHash::try_new(std::sync::Arc::new(new_tip)).unwrap();
    state.transition_frontier.best_chain[0].block = new_tip.clone();
    reduce(
        &mut state,
        now,
        BlockProducerAction::BestTipUpdate { best_tip: new_tip },
    );
    assert_eq!(
        state.transition_frontier.best_tip().unwrap().global_slot(),
        1
    );
    assert_eq!(state.block_producer.next_block_production(), Some(timing));

    // A higher tip makes the scheduled slot obsolete. Recompute with no
    // winners, then schedule a new winner from that newer consensus point.
    let mut higher_tip = (*genesis.block.block).clone();
    let consensus = &mut higher_tip.header.protocol_state.body.consensus_state;
    consensus.curr_global_slot_since_hard_fork.slot_number =
        v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(4u32.into());
    consensus.global_slot_since_genesis =
        v2::MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(4u32.into());
    let higher_tip =
        mina_core::block::BlockWithHash::try_new(std::sync::Arc::new(higher_tip)).unwrap();
    state.transition_frontier.best_chain[0].block = higher_tip.clone();
    reduce(
        &mut state,
        now,
        BlockProducerAction::BestTipUpdate {
            best_tip: higher_tip,
        },
    );
    state
        .block_producer
        .as_mut()
        .unwrap()
        .vrf_evaluator
        .won_slots
        .clear();
    reduce(
        &mut state,
        now,
        BlockProducerAction::WonSlotDiscard {
            reason: BlockProducerWonSlotDiscardReason::BestTipGlobalSlotHigher,
        },
    );
    let timing = state.block_producer.next_block_production().unwrap();
    assert_eq!(timing.generated_from_consensus_at.global_slot, 4);
    assert_eq!(timing.next_won_slot, None);
    state
        .block_producer
        .as_mut()
        .unwrap()
        .vrf_evaluator
        .won_slots
        .insert(6, vrf_won_slot(6));
    reduce(&mut state, now, BlockProducerAction::WonSlotSearch);
    reduce(
        &mut state,
        now,
        BlockProducerAction::WonSlot {
            won_slot: genesis.won_slot(6),
        },
    );
    let timing = state.block_producer.next_block_production().unwrap();
    assert_eq!(timing.generated_from_consensus_at.global_slot, 4);
    assert_eq!(timing.next_won_slot.unwrap().global_slot, 6);
}

/// `node`, also running a SNARK worker for the genesis winner's key, idle,
/// with `lead` as its block-first lead (P5.3).
fn node_with_worker(
    genesis: &Genesis,
    now: Timestamp,
    won_slots: &[u32],
    lead: Option<std::time::Duration>,
) -> State {
    let mut state = node(genesis, now, won_slots);
    state.config.snarker = Some(crate::SnarkerConfig {
        public_key: AccountSecretKey::genesis_producer().public_key(),
        fee: v2::CurrencyFeeStableV1(v2::UnsignedExtendedUInt64Int64ForVersionTagsStableV1(
            100_000_000.into(),
        )),
        strategy: crate::SnarkerStrategy::Sequential,
        auto_commit: true,
        block_first_lead: lead,
        skip_zkapp_jobs: false,
    });
    state.external_snark_worker.0.state =
        crate::external_snark_worker::ExternalSnarkWorkerState::Idle;
    state
}

/// Runs the reducer on `action` at `now` and returns what it dispatched.
fn reduce_collect(state: &mut State, now: Timestamp, action: impl Into<Action>) -> Vec<Action> {
    let action = action.into();
    assert!(action.is_enabled(state, now), "not enabled: {action:?}");
    let mut dispatcher = Dispatcher::new();
    crate::reducer(
        state,
        &ActionMeta::zero_custom(now).with_action(action),
        &mut dispatcher,
    );
    std::iter::from_fn(|| dispatcher.pop()).collect()
}

/// P5.3: the web node's SNARK worker shares its one prover instance with the
/// block producer, which proves one thing at a time and cannot stop a
/// proof. So the worker takes no job from `lead` (the longest a job takes)
/// plus the production estimate before a won slot until the slot is done:
/// a block proof never waits behind SNARK work. A worker with a prover of its
/// own (no lead, the native node) takes jobs at any time.
#[test]
fn a_worker_sharing_the_prover_takes_no_job_near_our_won_slot() {
    use std::time::Duration;
    // Natively the block takes 6 s to produce (20 s in a browser).
    const ESTIMATE_S: i64 = 6;
    let genesis = genesis();
    let slot = 30;
    let start = genesis.slot_time(slot);
    let lead = Duration::from_secs(200);
    let lead_s = lead.as_secs() as i64;
    for (offset_s, may) in [
        (-lead_s - ESTIMATE_S - 1, true),
        (-lead_s - ESTIMATE_S, false),
        (-60, false),
        (0, false),
        (89, false),
        // The slot is over: not due any more.
        (90, true),
    ] {
        let now = at(start, offset_s);
        let mut state = node_with_worker(&genesis, now, &[slot], Some(lead));
        // The genesis is proved: only the won slot counts here.
        state.transition_frontier.genesis = TransitionFrontierGenesisState::ProveSuccess {
            time: now,
            genesis: genesis.block.clone(),
        };
        for current in [
            BlockProducerCurrentState::WonSlot {
                time: now,
                won_slot: genesis.won_slot(slot),
            },
            BlockProducerCurrentState::WonSlotWait {
                time: now,
                won_slot: genesis.won_slot(slot),
            },
        ] {
            state.block_producer.as_mut().unwrap().current = current;
            assert_eq!(
                state.snark_worker_may_take_job(),
                may,
                "{offset_s} s from the won slot"
            );
            // A worker with its own prover is never held back.
            let mut own = state.clone();
            own.config.snarker.as_mut().unwrap().block_first_lead = None;
            assert!(own.snark_worker_may_take_job());
        }
        // While a block is being produced, whatever the time.
        state.block_producer.as_mut().unwrap().current = proving(&genesis, now, slot);
        assert!(!state.snark_worker_may_take_job(), "{offset_s} s, proving");
        // Idle, with no won slot ahead.
        state.block_producer.as_mut().unwrap().current =
            BlockProducerCurrentState::Idle { time: now };
        assert!(state.snark_worker_may_take_job(), "{offset_s} s, idle");
    }
}

#[test]
fn block_first_lead_does_not_pause_without_a_producer() {
    let genesis = genesis();
    let slot = 30;
    let now = at(genesis.slot_time(slot), -30);
    let mut state = node_with_worker(
        &genesis,
        now,
        &[slot],
        Some(std::time::Duration::from_secs(420)),
    );
    state.block_producer = crate::block_producer::BlockProducerState::new(now, None);
    assert!(state.snark_worker_may_take_job());
}

/// The genesis block's proof is a block proof too: the worker takes no job
/// while it is due (two minutes before the first won slot) or being made.
#[test]
fn a_worker_sharing_the_prover_gives_way_to_the_genesis_proof() {
    let genesis = genesis();
    let slot = 3;
    let lead = Some(std::time::Duration::from_secs(1));
    for (offset_s, produced_may, pending_may) in [(-200, true, false), (-100, false, false)] {
        let now = at(genesis.slot_time(slot), offset_s);
        let mut state = node_with_worker(&genesis, now, &[slot], lead);
        state.block_producer.as_mut().unwrap().current = BlockProducerCurrentState::WonSlot {
            time: now,
            won_slot: genesis.won_slot(slot),
        };
        assert_eq!(
            state.snark_worker_may_take_job(),
            produced_may,
            "{offset_s}"
        );
        state.transition_frontier.genesis = genesis.prove_pending();
        assert_eq!(state.snark_worker_may_take_job(), pending_may, "{offset_s}");
    }
}

/// Held back, the worker records it, so that it looks for a job again as
/// soon as it may (the periodic check dispatches `AutoCreateCommitment`
/// then); and it commits to nothing meanwhile.
#[test]
fn a_held_back_worker_commits_to_nothing_and_says_so_once() {
    use crate::snark_pool::SnarkPoolAction;
    let genesis = genesis();
    let slot = 30;
    let now = at(genesis.slot_time(slot), -30);
    let mut state = node_with_worker(
        &genesis,
        now,
        &[slot],
        Some(std::time::Duration::from_secs(120)),
    );
    state.transition_frontier.genesis = TransitionFrontierGenesisState::ProveSuccess {
        time: now,
        genesis: genesis.block.clone(),
    };
    state.block_producer.as_mut().unwrap().current = BlockProducerCurrentState::WonSlot {
        time: now,
        won_slot: genesis.won_slot(slot),
    };
    assert!(!state.snark_worker_may_take_job());

    let dispatched = reduce_collect(&mut state, now, SnarkPoolAction::AutoCreateCommitment);
    assert!(
        matches!(
            dispatched.as_slice(),
            [Action::SnarkPool(SnarkPoolAction::WorkerPausedForBlock {
                paused: true
            })]
        ),
        "{dispatched:?}"
    );
    reduce(
        &mut state,
        now,
        SnarkPoolAction::WorkerPausedForBlock { paused: true },
    );
    assert!(state.snark_pool.worker_paused_for_block);
    // Said once: the same state is not set again, and nothing is committed.
    assert!(!SnarkPoolAction::WorkerPausedForBlock { paused: true }.is_enabled(&state, now));
    let dispatched = reduce_collect(&mut state, now, SnarkPoolAction::AutoCreateCommitment);
    assert!(dispatched.is_empty(), "{dispatched:?}");

    // Our block is in: the worker may again, and clears the record when it
    // looks for a job.
    let later = at(genesis.slot_time(slot), 95);
    state.block_producer.as_mut().unwrap().current =
        BlockProducerCurrentState::Idle { time: later };
    assert!(state.snark_worker_may_take_job());
    let dispatched = reduce_collect(&mut state, later, SnarkPoolAction::AutoCreateCommitment);
    assert!(
        dispatched.iter().any(|action| matches!(
            action,
            Action::SnarkPool(SnarkPoolAction::WorkerPausedForBlock { paused: false })
        )),
        "{dispatched:?}"
    );
}
