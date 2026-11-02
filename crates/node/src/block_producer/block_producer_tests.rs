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
    vrf_evaluator::{EpochFingerprint, VrfWonSlotWithHash},
    BlockProducerAction, BlockProducerConfig, BlockProducerCurrentState, BlockProducerWonSlot,
    BlockProducerWonSlotDiscardReason, BlockWithoutProof,
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

/// The genesis winner's win of global slot `slot`, evaluated with the epoch
/// data a block of the made-up genesis selects for the slot's epoch.
fn vrf_won_slot(slot: u32) -> VrfWonSlotWithHash {
    let producer = AccountSecretKey::genesis_producer().public_key();
    let seed = v2::EpochSeed::from(v2::MinaBaseEpochSeedStableV1(BigInt::zero()));
    let epoch = slot / 7140;
    let epoch_data = EpochFingerprint::select(genesis().block.consensus_state(), epoch).unwrap_or(
        EpochFingerprint {
            epoch,
            ledger: ledger_hash(),
            seed: seed.clone(),
            total_currency: 0,
        },
    );
    VrfWonSlotWithHash::new(
        mina_vrf::VrfWonSlot {
            producer: producer.clone(),
            winner_account: producer,
            global_slot: slot,
            account_index: ledger::AccountIndex(0),
            vrf_output: Box::new(mina_vrf::genesis_vrf(seed).unwrap()),
            value_with_threshold: None,
        },
        epoch_data,
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
            seed_only: false,
            sharing: Default::default(),
            meshsub: P2pMeshsubConfig::default(),
            catchup_rpc: Default::default(),
        },
        transition_frontier: TransitionFrontierConfig::new(DEVNET_CONFIG.clone()),
        archive: None,
        block_producer: Some(BlockProducerConfig::new(
            AccountSecretKey::genesis_producer().public_key().into(),
        )),
        global: GlobalConfig {
            snark_work_fee: None,
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

/// With no block yet, a win in the epoch after the genesis epoch is made on
/// the genesis block, with its next epoch data, so the genesis block is
/// proved for it; a win two epochs on is not (BP-MN-3A).
#[test]
fn the_genesis_is_proved_for_a_win_of_the_next_epoch_only() {
    let genesis = genesis();
    for (slot, proves) in [
        (7140 + 3, true),
        (2 * 7140 - 1, true),
        (2 * 7140 + 3, false),
    ] {
        let now = at(genesis.slot_time(slot), -10);
        let mut state = node(&genesis, now, &[slot]);
        state.block_producer.as_mut().unwrap().current = BlockProducerCurrentState::WonSlot {
            time: now,
            won_slot: genesis.won_slot(slot),
        };
        assert_eq!(state.producing_block_after_genesis(), proves, "slot {slot}");
        assert_eq!(
            TransitionFrontierGenesisAction::ProveInit.is_enabled(&state, now),
            proves
        );
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
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/mina_lib/mina_lib.ml#L1372-L1428>
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

/// R43e.2: the uptime and error reporters see a best tip only once the node
/// takes part in the chain. OCaml has no transition frontier while it
/// bootstraps; this node's holds the genesis block until its first sync
/// commits, so catching up from genesis is still a bootstrap.
#[test]
fn reporters_see_a_best_tip_only_once_the_node_participates() {
    use crate::{
        rpc_effectful::{reporting_snapshot, reporting_sync_phase},
        transition_frontier::sync::TransitionFrontierSyncState,
    };
    let genesis = genesis();
    let now = genesis.slot_time(10);
    let mut state = node(&genesis, now, &[]);
    let no_report = |state: &State| {
        let snapshot = reporting_snapshot(state, now);
        assert!(snapshot.best_tip.is_none());
        assert!(snapshot.uptime_job.is_none());
        assert!(snapshot.uptime_spec.is_none());
    };
    assert_eq!(reporting_sync_phase(&state), (false, None));
    no_report(&state);

    state.transition_frontier.sync = TransitionFrontierSyncState::Init {
        time: now,
        best_tip: genesis.block.clone(),
        root_block: genesis.block.clone(),
        blocks_inbetween: Vec::new(),
    };
    assert_eq!(reporting_sync_phase(&state), (false, Some("Bootstrap")));
    no_report(&state);

    let catchup = || TransitionFrontierSyncState::BlocksSuccess {
        time: now,
        chain: Vec::new(),
        root_snarked_ledger_updates: Default::default(),
        needed_protocol_states: Default::default(),
    };
    state.transition_frontier.sync = catchup();
    assert_eq!(reporting_sync_phase(&state), (false, Some("Bootstrap")));
    no_report(&state);

    state.transition_frontier.sync = TransitionFrontierSyncState::Synced { time: now };
    // GenesisInject sets Synced before either network sync or restore.
    assert_eq!(reporting_sync_phase(&state), (false, Some("Bootstrap")));
    no_report(&state);
    // Recognition must not depend on the genesis state's cached hash/proof.
    state.transition_frontier.genesis = TransitionFrontierGenesisState::Idle;
    no_report(&state);

    // Catching up from a chain synced before.
    let (hash, _) = state_hashes();
    let vrf = genesis.block.consensus_state().last_vrf_output.clone();
    state.transition_frontier.best_chain.push(AppliedBlock {
        block: block_at(&genesis, 5, 2, &vrf, &hash),
        just_emitted_a_proof: false,
    });
    state.transition_frontier.sync = catchup();
    assert_eq!(reporting_sync_phase(&state), (true, Some("Catchup")));
    assert!(reporting_snapshot(&state, now).best_tip.is_some());

    state.transition_frontier.sync = TransitionFrontierSyncState::Synced { time: now };
    assert_eq!(reporting_sync_phase(&state), (true, Some("Synced")));
    assert!(reporting_snapshot(&state, now).best_tip.is_some());

    // Restore must suppress even a non-genesis tip until its ledgers are ready.
    use crate::transition_frontier::persist::TransitionFrontierPersistState;
    state.transition_frontier.persist = TransitionFrontierPersistState::Idle;
    assert_eq!(reporting_sync_phase(&state), (false, Some("Bootstrap")));
    no_report(&state);
    state.transition_frontier.persist = TransitionFrontierPersistState::OpenPending { time: now };
    no_report(&state);
    use crate::transition_frontier::persist::{
        PersistCheckpointState, PersistReadyState, PersistRestoreInfo, PersistRestoreOutcome,
        PersistRestoreState,
    };
    let info = Box::new(PersistRestoreInfo {
        generation: 1,
        attempt: 1,
        root: genesis.hash.clone(),
        root_height: genesis.block.height(),
        tip: state.transition_frontier.best_tip().unwrap().hash().clone(),
        tip_height: state.transition_frontier.best_tip().unwrap().height(),
        blocks: 1,
        ledgers: 1,
        load_started: now,
        timings: Default::default(),
    });
    for restore in [
        PersistRestoreState::LoadPending {
            time: now,
            completed_stages: 0,
            last_progress_time: None,
        },
        PersistRestoreState::Cancelling { time: now },
        PersistRestoreState::Replaying {
            time: now,
            info: info.clone(),
        },
    ] {
        state.transition_frontier.persist =
            TransitionFrontierPersistState::Ready(Box::new(PersistReadyState {
                opened_at: now,
                dir: String::new(),
                open_outcome: "test".into(),
                store: Default::default(),
                chain_put_sent: None,
                chain_puts_pending: 0,
                checkpoint: PersistCheckpointState::Idle,
                last_checkpoint: None,
                retry_after: None,
                stats: Default::default(),
                restore,
                checkpoint_chain_len_limit: None,
            }));
        no_report(&state);
    }
    let TransitionFrontierPersistState::Ready(ready) = &mut state.transition_frontier.persist
    else {
        unreachable!()
    };
    ready.restore = PersistRestoreState::Done {
        time: now,
        outcome: PersistRestoreOutcome::Restored {
            info,
            replay_ms: 1.0,
        },
    };
    assert!(reporting_snapshot(&state, now).best_tip.is_some());

    state.transition_frontier.ledger_recovery = Some(crate::transition_frontier::LedgerRecovery {
        block_hash: genesis.hash.clone(),
        error: "test".into(),
        attempts: 1,
        time: now,
        recovered_at: None,
    });
    no_report(&state);
    state.transition_frontier.ledger_recovery = None;

    // Re-bootstrap retains the old chain while replacing its ledgers.
    state.transition_frontier.sync = TransitionFrontierSyncState::Init {
        time: now,
        best_tip: genesis.block.clone(),
        root_block: genesis.block.clone(),
        blocks_inbetween: Vec::new(),
    };
    no_report(&state);
}

/// OCaml broadcasts local production before frontier insertion. A later
/// fork and a new producer slot must not erase the uptime notification.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/block_producer/block_producer.ml#L968-L980>
#[test]
fn uptime_retains_local_production_across_a_fork() {
    use crate::rpc_effectful::reporting_produced_block;
    let genesis = genesis();
    let now = genesis.slot_time(7);
    let mut state = node(&genesis, now, &[]);
    uptime_watch(
        &mut state,
        now,
        crate::rpc::RpcReportingProducedBlockWatch::Start { subscription: 1 },
    );
    let BlockProducerCurrentState::BlockProvePending {
        won_slot,
        chain,
        block,
        ..
    } = proving(&genesis, now, 7)
    else {
        unreachable!()
    };
    let block_hash = state_hashes().0;
    state.block_producer.as_mut().unwrap().current = BlockProducerCurrentState::BlockProveSuccess {
        time: now,
        won_slot,
        chain,
        block,
        block_hash: block_hash.clone(),
        proof: genesis.block.header().protocol_state_proof.clone(),
    };
    assert!(reporting_produced_block(&state).is_none());
    reduce(&mut state, now, BlockProducerAction::BlockProduced);
    let found = reporting_produced_block(&state).unwrap();
    assert_eq!(found.block.hash(), &block_hash);
    assert_eq!(found.slot_time, now);
    assert_eq!(found.subscription, 1);
    // Notification is available before injection, as in OCaml.
    assert!(!state
        .transition_frontier
        .best_chain
        .iter()
        .any(|b| b.hash() == &block_hash));
    reduce(&mut state, now, BlockProducerAction::BlockInjected);
    state.transition_frontier.best_chain.push(AppliedBlock {
        block: found.block.clone(),
        just_emitted_a_proof: false,
    });
    state.block_producer.as_mut().unwrap().current = BlockProducerCurrentState::Idle { time: now };
    // Adopt a competing chain that contains none of our completed block.
    state.transition_frontier.best_chain.truncate(1);
    assert_eq!(
        reporting_produced_block(&state).unwrap().block.hash(),
        &block_hash
    );
}

#[test]
fn uptime_does_not_infer_local_production_from_a_matching_key() {
    use crate::rpc_effectful::reporting_produced_block;
    let genesis = genesis();
    let mut state = node(&genesis, genesis.slot_time(10), &[]);
    let (hash, _) = state_hashes();
    let vrf = genesis.block.consensus_state().last_vrf_output.clone();
    let received = block_at(&genesis, 7, 2, &vrf, &hash);
    assert_eq!(
        received.producer(),
        &state.block_producer.config().unwrap().pub_key
    );
    state.transition_frontier.best_chain.push(AppliedBlock {
        block: received,
        just_emitted_a_proof: false,
    });
    assert!(reporting_produced_block(&state).is_none());
}

fn uptime_watch(
    state: &mut State,
    now: Timestamp,
    watch: crate::rpc::RpcReportingProducedBlockWatch,
) {
    reduce(
        state,
        now,
        crate::rpc::RpcAction::ReportingSnapshotGet {
            rpc_id: crate::rpc::RpcId::new_unchecked(1, 1),
            produced_block_watch: Some(watch),
        },
    );
}

fn uptime_complete(state: &mut State, genesis: &Genesis, slot: u32, hash: &v2::StateHash) {
    let now = genesis.slot_time(slot);
    let BlockProducerCurrentState::BlockProvePending {
        won_slot,
        chain,
        block,
        ..
    } = proving(genesis, now, slot)
    else {
        unreachable!()
    };
    state.block_producer.as_mut().unwrap().current = BlockProducerCurrentState::BlockProveSuccess {
        time: now,
        won_slot,
        chain,
        block,
        block_hash: hash.clone(),
        proof: genesis.block.header().protocol_state_proof.clone(),
    };
    reduce(state, now, BlockProducerAction::BlockProduced);
    assert_eq!(state.block_producer.produced_block().unwrap().hash(), hash);
}

/// Both actual BlockProduced reductions run before the reporter next polls.
/// The first completion fills OCaml's waiter even when the consumer stalls.
/// OCaml: <https://github.com/janestreet/async_kernel/blob/v0.14.0/src/bvar.ml#L35-L49>
#[test]
fn uptime_latches_first_completion_before_next_poll_even_across_a_fork() {
    use crate::{
        rpc::RpcReportingProducedBlockWatch as Watch, rpc_effectful::reporting_produced_block,
    };
    for fork in [false, true] {
        let genesis = genesis();
        let now = genesis.slot_time(7);
        let mut state = node(&genesis, now, &[]);
        let (first, second) = state_hashes();
        uptime_watch(&mut state, now, Watch::Start { subscription: 1 });
        uptime_complete(&mut state, &genesis, 7, &first);
        let block = state.block_producer.produced_block().unwrap().clone();
        state.transition_frontier.best_chain.push(AppliedBlock {
            block,
            just_emitted_a_proof: false,
        });
        if fork {
            state.transition_frontier.best_chain.truncate(1);
        }
        uptime_complete(&mut state, &genesis, 8, &second);
        let found = reporting_produced_block(&state).unwrap();
        assert_eq!(found.block.hash(), &first, "fork={fork}");
        assert_eq!(found.slot_time, now);
        assert_eq!(found.subscription, 1);
        // An ordinary telemetry sample does not consume the waiter's value.
        reduce(
            &mut state,
            now,
            crate::rpc::RpcAction::ReportingSnapshotGet {
                rpc_id: crate::rpc::RpcId::new_unchecked(1, 2),
                produced_block_watch: None,
            },
        );
        assert_eq!(
            reporting_produced_block(&state).unwrap().block.hash(),
            &first
        );
        uptime_watch(&mut state, now, Watch::Cancel { subscription: 1 });
        assert!(state
            .block_producer
            .as_ref()
            .unwrap()
            .uptime_produced_block
            .is_none());
    }
}

#[test]
fn uptime_subscription_excludes_history_and_old_cancellation() {
    use crate::{
        rpc::RpcReportingProducedBlockWatch as Watch, rpc_effectful::reporting_produced_block,
    };
    let genesis = genesis();
    let now = genesis.slot_time(7);
    let mut state = node(&genesis, now, &[]);
    let (first, second) = state_hashes();
    uptime_complete(&mut state, &genesis, 7, &first);
    assert!(state
        .block_producer
        .as_ref()
        .unwrap()
        .uptime_produced_block
        .is_none());
    uptime_watch(&mut state, now, Watch::Start { subscription: 1 });
    assert!(reporting_produced_block(&state).is_none());
    uptime_complete(&mut state, &genesis, 8, &first);
    uptime_watch(&mut state, now, Watch::Start { subscription: 2 });
    assert!(reporting_produced_block(&state).is_none());
    uptime_watch(&mut state, now, Watch::Cancel { subscription: 1 });
    uptime_complete(&mut state, &genesis, 9, &second);
    let found = reporting_produced_block(&state).unwrap();
    assert_eq!(found.block.hash(), &second);
    assert_eq!(found.subscription, 2);
    uptime_watch(&mut state, now, Watch::Cancel { subscription: 2 });
    uptime_complete(&mut state, &genesis, 10, &first);
    assert!(reporting_produced_block(&state).is_none());
}

/// BP-MN-11: exercise the actual guards and reducers, withholding ledger/VRF
/// service responses to cover asynchronous boundaries. No cryptography or
/// network services run; the assertions concern control flow, not VRF values.
mod vrf_waiting {
    use super::*;
    use crate::block_producer::vrf_evaluator::{
        BlockProducerVrfEvaluatorAction as VrfAction, BlockProducerVrfEvaluatorState,
        BlockProducerVrfEvaluatorStatus as Status, EpochContext, InterruptReason,
    };

    fn evaluator(state: &State) -> &BlockProducerVrfEvaluatorState {
        state.block_producer.vrf_evaluator().unwrap()
    }

    fn invariant(state: &State) {
        if matches!(
            evaluator(state).status,
            Status::ReadyToEvaluate { .. }
                | Status::EpochDelegatorTablePending { .. }
                | Status::EpochDelegatorTableSuccess { .. }
                | Status::InitialSlotSelection { .. }
        ) {
            assert!(matches!(
                evaluator(state).epoch_context(),
                EpochContext::Current(_) | EpochContext::Next(_)
            ));
        }
    }

    fn step(state: &mut State, action: VrfAction) -> Vec<VrfAction> {
        invariant(state);
        let actions = reduce_collect(state, state.time(), action);
        invariant(state);
        actions
            .into_iter()
            .filter_map(|a| match a {
                Action::BlockProducer(BlockProducerAction::VrfEvaluator(a)) => Some(a),
                _ => None,
            })
            // As in the store, queued actions still have to pass their guard.
            // In these fixtures CleanupOldSlots has no retained wins to prune.
            .filter(|a| a.is_enabled(state, state.time()))
            .collect()
    }

    fn only_step(state: &mut State, action: VrfAction) -> VrfAction {
        let mut actions = step(state, action);
        assert_eq!(actions.len(), 1, "{actions:?}");
        actions.pop().unwrap()
    }

    fn initialized(genesis: &Genesis) -> State {
        let mut state = node(genesis, genesis.slot_time(10), &[]);
        let action = only_step(
            &mut state,
            VrfAction::InitializeEvaluator {
                best_tip: genesis.block.clone(),
            },
        );
        assert!(step(&mut state, action).is_empty());
        assert!(matches!(
            evaluator(&state).epoch_context(),
            EpochContext::Waiting
        ));
        state
    }

    fn check(
        genesis: &Genesis,
        current_epoch: Option<u32>,
        best_tip_epoch: u32,
        root_block_epoch: u32,
        finalized: bool,
    ) -> VrfAction {
        let slots = genesis.block.constants().slots_per_epoch.as_u32();
        VrfAction::CheckEpochEvaluability {
            current_epoch,
            is_next_epoch_seed_finalized: finalized,
            best_tip_epoch,
            root_block_epoch,
            best_tip_slot: 10,
            best_tip_global_slot: best_tip_epoch * slots + 10,
            next_epoch_first_slot: Some((best_tip_epoch + 1) * slots),
            staking_epoch_data: Box::new(
                genesis.block.consensus_state().staking_epoch_data.clone(),
            ),
            next_epoch_data: Box::new(genesis.block.consensus_state().next_epoch_data.clone()),
        }
    }

    fn complete_table() -> VrfAction {
        VrfAction::FinalizeDelegatorTableConstruction {
            delegator_table: Some(Default::default()),
        }
    }

    /// Marks the tip's epoch (staking data) and, from `through` on, the next
    /// epoch (next data) of the made-up genesis as evaluated through `through`.
    fn complete_through(state: &mut State, genesis: &Genesis, best: u32, through: u32) {
        let cs = genesis.block.consensus_state();
        let vrf = &mut state.block_producer.as_mut().unwrap().vrf_evaluator;
        if through >= best {
            vrf.mark_epoch_evaluated(EpochFingerprint::new(
                best,
                &cs.staking_epoch_data.clone().into(),
            ));
        }
        if through > best {
            vrf.mark_epoch_evaluated(EpochFingerprint::new(
                best + 1,
                &cs.next_epoch_data.clone().into(),
            ));
        }
    }

    fn roundtrip(state: &mut State) {
        // Recorded state retains the context together with status. A normal
        // daemon restart instead constructs a fresh evaluator (tested below).
        let bytes = serde_json::to_vec(evaluator(state)).unwrap();
        state.block_producer.as_mut().unwrap().vrf_evaluator =
            serde_json::from_slice(&bytes).unwrap();
        invariant(state);
    }

    #[test]
    fn maximum_protocol_slot_finishes_without_requesting_a_successor() {
        use crate::block_producer::vrf_evaluator::PendingEvaluation;
        let genesis = genesis();
        let mut state = initialized(&genesis);
        let epoch = u32::MAX / 7140;
        let time = state.time();
        let vrf = &mut state.block_producer.as_mut().unwrap().vrf_evaluator;
        vrf.set_pending_evaluation(PendingEvaluation {
            epoch_number: epoch,
            epoch_data: genesis
                .block
                .consensus_state()
                .staking_epoch_data
                .clone()
                .into(),
            latest_evaluated_slot: u32::MAX,
            generation: vrf.generation(),
        });
        vrf.status = Status::SlotEvaluationReceived {
            time,
            global_slot: u32::MAX,
        };
        assert!(vrf.construct_vrf_input().is_none());
        let finish = only_step(
            &mut state,
            VrfAction::CheckEpochBounds {
                epoch_number: epoch,
                latest_evaluated_global_slot: u32::MAX,
            },
        );
        assert!(matches!(finish, VrfAction::FinishEpochEvaluation { .. }));
        assert!(step(&mut state, finish).is_empty());
        assert!(evaluator(&state).current_evaluation().is_none());
        assert_eq!(evaluator(&state).last_evaluated_epoch(), Some(epoch));
    }

    #[test]
    fn waiting_reasons_block_initialization_and_resume_when_data_is_ready() {
        let genesis = genesis();
        // Missing clock, wall clock ahead after sleep, unfinalized seed,
        // root/ledger lag, and both epochs already evaluated.
        for (current, best, root, finalized, completed) in [
            (None, 0, 0, false, None),
            (Some(2), 0, 0, false, None),
            (Some(2), 2, 2, false, Some(2)),
            (Some(2), 2, 1, true, Some(2)),
            (Some(2), 2, 2, true, Some(3)),
        ] {
            let mut state = initialized(&genesis);
            // Model the historical completion cursor; setup transitions below
            // are always passed through the real action enabling conditions.
            if let Some(epoch) = completed {
                complete_through(&mut state, &genesis, best, epoch);
            }
            let waiting = only_step(&mut state, check(&genesis, current, best, root, finalized));
            assert!(matches!(waiting, VrfAction::WaitForNextEvaluation));
            assert!(matches!(
                evaluator(&state).epoch_context(),
                EpochContext::Waiting
            ));
            let attempted_init = VrfAction::InitializeEpochEvaluation {
                best_tip_epoch: best,
                best_tip_slot: 10,
                best_tip_global_slot: 10,
                next_epoch_first_slot: Some(7140),
                staking_epoch_data: genesis
                    .block
                    .consensus_state()
                    .staking_epoch_data
                    .clone()
                    .into(),
                producer: AccountSecretKey::genesis_producer().public_key().into(),
            };
            assert!(!attempted_init.is_enabled(&state, state.time()));
            assert!(!complete_table().is_enabled(&state, state.time()));
            step(&mut state, waiting);
            roundtrip(&mut state);
            // A caught-up tip in a fresh epoch permits evaluation again.
            let init = only_step(&mut state, check(&genesis, Some(4), 4, 4, true));
            assert!(matches!(init, VrfAction::InitializeEpochEvaluation { .. }));
            // This fixture checks readiness with supplied epoch data. The
            // catch-up tests below also update the frontier and clock before
            // running table completion and its epoch-data revalidation.
        }
    }

    #[test]
    fn delayed_table_and_recorded_states_keep_current_or_next_context() {
        let genesis = genesis();
        for next_epoch in [false, true] {
            let mut state = initialized(&genesis);
            if next_epoch {
                complete_through(&mut state, &genesis, 0, 0);
            }
            let init = only_step(&mut state, check(&genesis, Some(0), 0, 0, true));
            let begin = only_step(&mut state, init);
            for action in [begin, complete_table()] {
                roundtrip(&mut state);
                // Even if a reorg, epoch change, or a sleep would select Waiting,
                // the check cannot run while the table is being constructed.
                for check in [
                    check(&genesis, Some(0), 0, 0, false),
                    check(&genesis, Some(2), 0, 0, true),
                    check(&genesis, Some(2), 2, 1, true),
                ] {
                    assert!(!check.is_enabled(&state, state.time()));
                }
                let actions = step(&mut state, action);
                if let Some(select) = actions.into_iter().next() {
                    assert!(matches!(select, VrfAction::SelectInitialSlot { .. }));
                    roundtrip(&mut state);
                    let begin = only_step(&mut state, select);
                    let continue_action = only_step(&mut state, begin);
                    let evaluate = only_step(&mut state, continue_action);
                    let VrfAction::EvaluateSlot { vrf_input } = &evaluate else {
                        panic!("{evaluate:?}")
                    };
                    // Current includes the wall-clock slot; Next begins at
                    // the next epoch's first global slot.
                    assert_eq!(vrf_input.global_slot, if next_epoch { 7140 } else { 10 });
                    step(&mut state, evaluate);
                }
            }
        }
    }

    #[test]
    fn interrupt_at_each_setup_boundary_invalidates_stale_actions() {
        let genesis = genesis();
        for boundary in 0..4 {
            let mut state = initialized(&genesis);
            let mut queued = only_step(&mut state, check(&genesis, Some(0), 0, 0, true));
            if boundary >= 1 {
                queued = only_step(&mut state, queued);
            }
            if boundary >= 2 {
                step(&mut state, queued);
                queued = complete_table();
            }
            if boundary >= 3 {
                queued = only_step(&mut state, queued);
            }
            step(
                &mut state,
                VrfAction::InterruptEpochEvaluation {
                    reason: InterruptReason::BestTipWithHigherEpoch,
                },
            );
            assert!(!queued.is_enabled(&state, state.time()));
            let wait = only_step(&mut state, check(&genesis, Some(2), 0, 0, true));
            assert!(matches!(
                evaluator(&state).epoch_context(),
                EpochContext::Waiting
            ));
            assert!(!queued.is_enabled(&state, state.time()));
            step(&mut state, wait);
            assert!(!complete_table().is_enabled(&state, state.time()));
            roundtrip(&mut state);
        }
    }

    #[test]
    fn epoch_completion_waits_for_seed_then_evaluates_next_epoch() {
        let genesis = genesis();
        let mut state = initialized(&genesis);
        reduce(
            &mut state,
            genesis.slot_time(7137),
            crate::CheckTimeoutsAction {},
        );
        let init = only_step(&mut state, check(&genesis, Some(0), 0, 0, false));
        let begin = only_step(&mut state, init);
        step(&mut state, begin);
        let select = only_step(&mut state, complete_table());
        let begin = only_step(&mut state, select);
        let mut continue_action = only_step(&mut state, begin);
        for slot in [7137, 7138, 7139] {
            let evaluate = only_step(&mut state, continue_action);
            let VrfAction::EvaluateSlot { vrf_input } = &evaluate else {
                panic!("{evaluate:?}")
            };
            let hash = vrf_input.staking_ledger_hash.clone();
            let generation = vrf_input.generation;
            assert_eq!(vrf_input.global_slot, slot);
            step(&mut state, evaluate);
            let bounds = only_step(
                &mut state,
                VrfAction::ProcessSlotEvaluationSuccess {
                    vrf_output: mina_vrf::VrfEvaluationOutput::SlotLost(slot),
                    staking_ledger_hash: hash,
                    generation,
                },
            );
            continue_action = only_step(&mut state, bounds);
        }
        assert!(matches!(
            continue_action,
            VrfAction::FinishEpochEvaluation { .. }
        ));
        step(&mut state, continue_action);
        assert_eq!(evaluator(&state).last_evaluated_epoch(), Some(0));
        let wait = only_step(&mut state, check(&genesis, Some(0), 0, 0, false));
        step(&mut state, wait);
        assert!(matches!(
            evaluator(&state).epoch_context(),
            EpochContext::Waiting
        ));
        let init = only_step(&mut state, check(&genesis, Some(0), 0, 0, true));
        assert!(matches!(
            evaluator(&state).epoch_context(),
            EpochContext::Next(_)
        ));
        let begin = only_step(&mut state, init);
        step(&mut state, begin);
        let select = only_step(&mut state, complete_table());
        let begin = only_step(&mut state, select);
        let next = only_step(&mut state, begin);
        let evaluate = only_step(&mut state, next);
        assert!(
            matches!(evaluate, VrfAction::EvaluateSlot { vrf_input } if vrf_input.global_slot == 7140)
        );
    }

    #[test]
    fn sleep_across_epochs_cannot_change_context_while_table_is_pending() {
        let genesis = genesis();
        let mut state = initialized(&genesis);
        let init = only_step(&mut state, check(&genesis, Some(0), 0, 0, true));
        let begin = only_step(&mut state, init);
        step(&mut state, begin);
        reduce(
            &mut state,
            genesis.slot_time(14281),
            crate::CheckTimeoutsAction {},
        );
        assert_eq!(state.current_epoch(), Some(2));
        // The recorded table request survives a round trip; a late table
        // could still be accepted for it.
        roundtrip(&mut state);
        assert!(complete_table().is_enabled(&state, state.time()));
        let actions = reduce_collect(
            &mut state,
            genesis.slot_time(14281),
            BlockProducerAction::BestTipUpdate {
                best_tip: genesis.block.clone(),
            },
        );
        // The clock has left the requested epoch: only an interruption, and
        // after it a readiness check, may run; nothing else of the setup.
        let mut interrupt = None;
        for action in actions {
            if let Action::BlockProducer(BlockProducerAction::VrfEvaluator(action)) = action {
                if matches!(action, VrfAction::InterruptEpochEvaluation { .. }) {
                    assert!(action.is_enabled(&state, state.time()));
                    interrupt = Some(action);
                } else if !matches!(action, VrfAction::CheckEpochEvaluability { .. }) {
                    assert!(!action.is_enabled(&state, state.time()), "{action:?}");
                }
            }
        }
        assert!(matches!(
            interrupt,
            Some(VrfAction::InterruptEpochEvaluation {
                reason: InterruptReason::EpochDataChanged
            })
        ));
        step(&mut state, interrupt.unwrap());
        // Two epochs past the tip, there is no data to evaluate with.
        let wait = only_step(&mut state, check(&genesis, Some(2), 0, 0, true));
        assert!(matches!(wait, VrfAction::WaitForNextEvaluation));
        assert!(matches!(
            evaluator(&state).epoch_context(),
            EpochContext::Waiting
        ));
        step(&mut state, wait);
        roundtrip(&mut state);
        // The late table of the expired epoch cannot start an evaluation.
        assert!(!complete_table().is_enabled(&state, state.time()));
    }

    #[test]
    fn fresh_restart_and_ledger_recovery_reset_status_with_context() {
        let genesis = genesis();
        let mut state = initialized(&genesis);
        let init = only_step(&mut state, check(&genesis, Some(0), 0, 0, true));
        let begin = only_step(&mut state, init);
        step(&mut state, begin);
        let stale_select = only_step(&mut state, complete_table());
        // Ledger recovery replaces the whole evaluator, including its context.
        reduce(
            &mut state,
            genesis.slot_time(10),
            BlockProducerAction::LedgerRecoveryAbort,
        );
        for state in [&state, &node(&genesis, genesis.slot_time(7141), &[])] {
            assert!(evaluator(state).is_idle());
            assert!(matches!(
                evaluator(state).epoch_context(),
                EpochContext::Waiting
            ));
            assert!(!stale_select.is_enabled(state, state.time()));
            assert!(!complete_table().is_enabled(state, state.time()));
        }
        let initialized = only_step(
            &mut state,
            VrfAction::InitializeEvaluator {
                best_tip: genesis.block.clone(),
            },
        );
        step(&mut state, initialized);
        let init = only_step(&mut state, check(&genesis, Some(0), 0, 0, true));
        assert!(matches!(init, VrfAction::InitializeEpochEvaluation { .. }));
    }
}

/// Scheduling expectations come from OCaml's clock-based epoch selection and
/// its current/past/future won-slot branches. Service responses are synthetic;
/// these tests exercise guards and reducers, not cryptographic eligibility.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/block_producer/block_producer.ml#L1133-L1209>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/consensus/proof_of_stake.ml#L2919-L2967>
mod vrf_catchup {
    use super::*;
    use crate::{
        block_producer::vrf_evaluator::{
            BlockProducerVrfEvaluatorAction as VrfAction, BlockProducerVrfEvaluatorState,
            BlockProducerVrfEvaluatorStatus as Status, VrfEvaluatorInput,
        },
        block_producer_effectful::{
            vrf_evaluator_effectful::BlockProducerVrfEvaluatorEffectfulAction as VrfEffect,
            BlockProducerEffectfulAction,
        },
        ledger::read::{LedgerReadAction, LedgerReadId, LedgerReadRequest, LedgerReadResponse},
        ledger_effectful::LedgerEffectfulAction,
    };

    const EPOCH: u32 = 7140;

    struct Driver {
        state: State,
        tables: Vec<LedgerReadId>,
        evaluations: Vec<VrfEvaluatorInput>,
        stats: crate::stats::block_producer::BlockProducerStats,
    }

    impl Driver {
        fn new(genesis: &Genesis, slot: u32) -> Self {
            let mut state = node(genesis, genesis.slot_time(slot), &[]);
            state.transition_frontier.genesis = TransitionFrontierGenesisState::ProveSuccess {
                time: state.time(),
                genesis: genesis.block.clone(),
            };
            Self {
                state,
                tables: vec![],
                evaluations: vec![],
                stats: Default::default(),
            }
        }

        fn evaluator(&self) -> &BlockProducerVrfEvaluatorState {
            self.state.block_producer.vrf_evaluator().unwrap()
        }

        // The store drains each reducer's queue before its next sibling action.
        // Recheck each guard at execution, and pause at service boundaries.
        fn dispatch(&mut self, action: impl Into<Action>) {
            let action = action.into();
            if !action.is_enabled(&self.state, self.state.time()) {
                return;
            }
            match action {
                Action::BlockProducer(_) | Action::Ledger(_) => {
                    let now = self.state.time();
                    for next in reduce_collect(&mut self.state, now, action) {
                        self.dispatch(next);
                    }
                }
                Action::LedgerEffects(LedgerEffectfulAction::ReadInit { id, request, .. })
                    if matches!(request, LedgerReadRequest::DelegatorTable(..)) =>
                {
                    self.tables.push(id);
                    self.dispatch(LedgerReadAction::Pending { id, request });
                }
                Action::BlockProducerEffectful(BlockProducerEffectfulAction::VrfEvaluator(
                    VrfEffect::EvaluateSlot { vrf_input },
                )) => self.evaluations.push(vrf_input),
                Action::BlockProducerEffectful(BlockProducerEffectfulAction::VrfEvaluator(
                    action @ VrfEffect::InitializeStats { .. },
                )) => action.record_epoch_stats(
                    &mut self.stats,
                    self.state.config.consensus_constants.slots_per_epoch,
                ),
                _ => (),
            }
        }

        fn wake(&mut self, genesis: &Genesis, slot: u32) {
            reduce(
                &mut self.state,
                genesis.slot_time(slot),
                crate::CheckTimeoutsAction {},
            );
        }

        fn tip(&mut self, genesis: &Genesis, slot: u32) {
            let tip = Self::tip_block(genesis, slot);
            self.set_tip(tip);
        }

        /// A tip whose next epoch data differ from [`Self::tip_block`]'s, as
        /// another fork's would: `changed` 0 the seed, 1 the ledger, 2 the
        /// total currency.
        fn tip_with_other_next_data(&mut self, genesis: &Genesis, slot: u32, changed: u32) {
            let mut tip = Self::tip_block(genesis, slot);
            let next = &mut std::sync::Arc::make_mut(&mut tip.block)
                .header
                .protocol_state
                .body
                .consensus_state
                .next_epoch_data;
            match changed {
                0 => {
                    next.seed = v2::EpochSeed::from(v2::MinaBaseEpochSeedStableV1(BigInt::from(
                        mina_curves::pasta::Fp::from(999u64),
                    )))
                }
                1 => {
                    next.ledger.hash = v2::LedgerHash::from(v2::MinaBaseLedgerHash0StableV1(
                        BigInt::from(mina_curves::pasta::Fp::from(999u64)),
                    ))
                }
                _ => next.ledger.total_currency = v2::CurrencyAmountStableV1(999u64.into()),
            }
            self.set_tip(tip);
        }

        fn set_tip(&mut self, tip: ArcBlockWithHash) {
            self.state.transition_frontier.best_chain = vec![AppliedBlock {
                block: tip.clone(),
                just_emitted_a_proof: false,
            }];
            self.dispatch(BlockProducerAction::BestTipUpdate { best_tip: tip });
        }

        fn tip_block(genesis: &Genesis, slot: u32) -> ArcBlockWithHash {
            let epoch = slot / EPOCH;
            let mut tip = block_at(
                genesis,
                slot,
                slot.saturating_add(2),
                &v2::ConsensusVrfOutputTruncatedStableV1::from(
                    &*vrf_won_slot(slot).won_slot.vrf_output,
                ),
                genesis.block.hash(),
            );
            let cs = &mut std::sync::Arc::make_mut(&mut tip.block)
                .header
                .protocol_state
                .body
                .consensus_state;
            cs.epoch_count = epoch.into();
            // Different seed, ledger and stake per epoch expose stale inputs.
            cs.staking_epoch_data.seed = v2::EpochSeed::from(v2::MinaBaseEpochSeedStableV1(
                BigInt::from(mina_curves::pasta::Fp::from(epoch as u64 + 1)),
            ));
            cs.staking_epoch_data.ledger.hash =
                v2::LedgerHash::from(v2::MinaBaseLedgerHash0StableV1(BigInt::from(
                    mina_curves::pasta::Fp::from(epoch as u64 + 1),
                )));
            cs.staking_epoch_data.ledger.total_currency =
                v2::CurrencyAmountStableV1((1000u64 + epoch as u64).into());
            cs.next_epoch_data.seed = v2::EpochSeed::from(v2::MinaBaseEpochSeedStableV1(
                BigInt::from(mina_curves::pasta::Fp::from(epoch as u64 + 2)),
            ));
            cs.next_epoch_data.ledger.hash = v2::LedgerHash::from(v2::MinaBaseLedgerHash0StableV1(
                BigInt::from(mina_curves::pasta::Fp::from(epoch as u64 + 2)),
            ));
            cs.next_epoch_data.ledger.total_currency =
                v2::CurrencyAmountStableV1((1001u64 + epoch as u64).into());
            tip
        }

        /// The staking ledger is not available: no table, not an empty one.
        fn missing_table(&mut self, id: LedgerReadId) {
            self.dispatch(LedgerReadAction::Success {
                id,
                response: LedgerReadResponse::DelegatorTable(None),
            });
        }

        /// A response for `slot` from the evaluation of `input`'s generation.
        fn respond(&mut self, input: &VrfEvaluatorInput, slot: u32, won: bool) {
            let vrf_output = if won {
                mina_vrf::VrfEvaluationOutput::SlotWon(vrf_won_slot(slot).won_slot)
            } else {
                mina_vrf::VrfEvaluationOutput::SlotLost(slot)
            };
            self.dispatch(VrfAction::ProcessSlotEvaluationSuccess {
                vrf_output,
                staking_ledger_hash: input.staking_ledger_hash.clone(),
                generation: input.generation,
            });
        }

        fn producing(&self) -> bool {
            matches!(
                self.state.block_producer.as_ref().unwrap().current,
                BlockProducerCurrentState::WonSlotTransactionsGet { .. }
            )
        }

        fn complete_table(&mut self, id: LedgerReadId) {
            self.dispatch(LedgerReadAction::Success {
                id,
                response: LedgerReadResponse::DelegatorTable(Some(Default::default())),
            });
        }

        fn assert_request(&self, slot: u32) {
            let input = self.evaluations.last().expect("evaluation must resume");
            let epoch = slot / EPOCH;
            assert_eq!(input.global_slot, slot, "do not skip the current slot");
            assert_eq!(self.evaluator().currently_evaluated_epoch(), Some(epoch));
            assert_eq!(
                input.epoch_seed,
                v2::EpochSeed::from(v2::MinaBaseEpochSeedStableV1(BigInt::from(
                    mina_curves::pasta::Fp::from(epoch as u64 + 1)
                )))
            );
            assert_eq!(
                input.staking_ledger_hash,
                v2::LedgerHash::from(v2::MinaBaseLedgerHash0StableV1(BigInt::from(
                    mina_curves::pasta::Fp::from(epoch as u64 + 1)
                )))
            );
            assert_eq!(input.total_currency, 1000 + epoch as u64);
        }

        fn win(&mut self, slot: u32) {
            let input = self.evaluations.last().unwrap().clone();
            assert_eq!(input.global_slot, slot);
            self.dispatch(VrfAction::ProcessSlotEvaluationSuccess {
                vrf_output: mina_vrf::VrfEvaluationOutput::SlotWon(vrf_won_slot(slot).won_slot),
                staking_ledger_hash: input.staking_ledger_hash,
                generation: input.generation,
            });
        }

        /// The data `tip` gives a block of `epoch`'s staking epoch: a seed,
        /// ledger and stake of their own per epoch.
        fn data(epoch: u32) -> EpochFingerprint {
            EpochFingerprint {
                epoch,
                ledger: v2::LedgerHash::from(v2::MinaBaseLedgerHash0StableV1(BigInt::from(
                    mina_curves::pasta::Fp::from(epoch as u64 + 1),
                ))),
                seed: v2::EpochSeed::from(v2::MinaBaseEpochSeedStableV1(BigInt::from(
                    mina_curves::pasta::Fp::from(epoch as u64 + 1),
                ))),
                total_currency: 1000 + epoch as u64,
            }
        }

        fn mark_evaluated(&mut self, epoch: u32) {
            self.state
                .block_producer
                .as_mut()
                .unwrap()
                .vrf_evaluator
                .mark_epoch_evaluated(Self::data(epoch));
        }

        fn assert_current_and_future_wins(&mut self, genesis: &Genesis, slot: u32) {
            self.assert_request(slot);
            self.win(slot);
            assert_eq!(
                self.state
                    .block_producer
                    .current_won_slot()
                    .unwrap()
                    .global_slot(),
                slot
            );
            assert!(
                BlockProducerAction::WonSlotProduceInit.is_enabled(&self.state, self.state.time())
            );
            // Preserve a future win while the current slot is selected.
            self.assert_request(slot + 1);
            self.win(slot + 1);
            assert!(self.evaluator().won_slots.contains_key(&(slot + 1)));
            self.wake(genesis, slot + 1);
            assert!(
                !BlockProducerAction::WonSlotProduceInit.is_enabled(&self.state, self.state.time())
            );
            self.dispatch(BlockProducerAction::WonSlotDiscard {
                reason: BlockProducerWonSlotDiscardReason::WonSlotExpired,
            });
            self.dispatch(BlockProducerAction::WonSlotSearch);
            assert_eq!(
                self.state
                    .block_producer
                    .current_won_slot()
                    .unwrap()
                    .global_slot(),
                slot + 1
            );
            self.dispatch(BlockProducerAction::WonSlotProduceInit);
            assert!(matches!(
                self.state.block_producer.as_ref().unwrap().current,
                BlockProducerCurrentState::WonSlotTransactionsGet { .. }
            ));
        }
    }

    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/vrf_evaluator/vrf_evaluator.ml#L313-L321>
    #[test]
    fn remaining_slot_stats_include_the_first_slot_requested_by_the_reducer() {
        let genesis = genesis();
        // OCaml's epoch bounds above allow all slots through 7139 inclusive.
        // Genesis slot zero is not evaluated by the Rust producer.
        for (now, first, remaining) in [
            (0, 1, 7139),
            (100, 100, 7040),
            (7139, 7139, 1),
            (7140, 7140, 7140),
        ] {
            let mut d = Driver::new(&genesis, now);
            d.tip(&genesis, now);
            d.complete_table(d.tables[0]);
            d.assert_request(first);
            let stats = &d.stats.vrf_evaluator[&(first / EPOCH)];
            assert_eq!(stats.total_slots, remaining, "first evaluated slot {first}");
            assert_eq!(stats.evaluated_slots, 0);
            assert_eq!(stats.total_currency, Some(1000 + u64::from(first / EPOCH)));
        }
    }

    /// The 15 cold starts of BP-MN-3: the clock in the epoch after the tip's,
    /// before (or just after) a block of it. OCaml selects the tip's next
    /// epoch data and evaluates at once; so does the producer, from the
    /// current slot, and produces on the tip. Unsolicited table or VRF
    /// responses still create nothing.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/consensus/proof_of_stake.ml#L2451-L2466>
    #[test]
    fn cold_start_after_an_epoch_boundary_evaluates_the_new_epoch_at_once() {
        let genesis = genesis();
        for tip_slot in [1, 4759, 4760, 7139, EPOCH + 1] {
            for offset in [0, 1, 77] {
                let epoch = tip_slot / EPOCH + 1;
                let slot = epoch * EPOCH + offset;
                let mut d = Driver::new(&genesis, slot);
                // Unsolicited responses before any request change nothing.
                d.dispatch(VrfAction::FinalizeDelegatorTableConstruction {
                    delegator_table: Some(Default::default()),
                });
                d.dispatch(VrfAction::ProcessSlotEvaluationSuccess {
                    vrf_output: mina_vrf::VrfEvaluationOutput::SlotWon(vrf_won_slot(slot).won_slot),
                    staking_ledger_hash: ledger_hash(),
                    generation: 0,
                });
                assert!(d.evaluator().won_slots.is_empty());
                d.tip(&genesis, tip_slot);
                assert_eq!(d.tables.len(), 1, "tip {tip_slot}, clock {slot}");
                assert!(d.evaluations.is_empty(), "no slot before the table");
                // The tip's next epoch data is the new epoch's staking data.
                assert_eq!(d.evaluator().target(), Some(&Driver::data(epoch)));
                assert!(
                    !BlockProducerAction::WonSlotProduceInit.is_enabled(&d.state, d.state.time())
                );
                d.complete_table(d.tables[0]);
                d.assert_current_and_future_wins(&genesis, slot);
            }
        }
    }

    /// #213 review, blocking 1: with the clock at slot 7140 and the tip at
    /// 4759, a replacement tip changes the next seed, ledger or total
    /// currency while the old evaluation's response is in flight. The old
    /// response is rejected even for the same slot and ledger; evaluation
    /// restarts with the new data, and only its win is produced.
    #[test]
    fn changed_next_seed_ledger_or_stake_restarts_an_early_evaluation() {
        let genesis = genesis();
        for changed in 0..3 {
            let mut d = Driver::new(&genesis, EPOCH);
            d.tip(&genesis, 4759);
            d.complete_table(d.tables[0]);
            d.assert_request(EPOCH);
            let old = d.evaluations.last().unwrap().clone();
            d.tip_with_other_next_data(&genesis, 4759, changed);
            let target = d.evaluator().target().cloned().expect("restarted");
            assert_ne!(target, Driver::data(1), "change {changed}");
            assert!(d.evaluator().generation() > old.generation);
            // The old, winning response arrives after the replacement.
            d.respond(&old, EPOCH, true);
            assert!(d.evaluator().won_slots.is_empty(), "change {changed}");
            assert!(d.state.block_producer.current_won_slot().is_none());
            d.complete_table(*d.tables.last().unwrap());
            let input = d.evaluations.last().unwrap().clone();
            assert_eq!(input.global_slot, EPOCH);
            assert_eq!(input.epoch_seed, target.seed);
            assert_eq!(input.staking_ledger_hash, target.ledger);
            assert_eq!(input.total_currency, target.total_currency);
            // Again, now that the new evaluation awaits the same slot: with
            // the same ledger only the generation tells the responses apart.
            d.respond(&old, EPOCH, true);
            assert!(d.evaluator().won_slots.is_empty(), "change {changed}");
            assert_eq!(d.evaluations.len(), 2, "still awaiting the new response");
            d.respond(&input, EPOCH, true);
            assert_eq!(
                d.evaluator().won_slots[&EPOCH].epoch_data.as_ref(),
                Some(&target)
            );
            d.dispatch(BlockProducerAction::WonSlotProduceInit);
            assert!(d.producing(), "change {changed}");
        }
    }

    /// #213 review, blocking 1 (completed epoch): an epoch fully evaluated
    /// with one tip's next data is evaluated again when another tip selects
    /// other data for it, and the old evaluation's future win is dropped.
    #[test]
    fn a_completed_epoch_is_evaluated_again_with_changed_inputs() {
        let genesis = genesis();
        for changed in 0..3 {
            let clock = 2 * EPOCH - 3;
            let mut d = Driver::new(&genesis, clock);
            d.tip(&genesis, EPOCH - 10);
            d.complete_table(d.tables[0]);
            for slot in clock..2 * EPOCH {
                let input = d.evaluations.last().unwrap().clone();
                assert_eq!(input.global_slot, slot);
                d.respond(&input, slot, slot == 2 * EPOCH - 1);
            }
            assert!(d.evaluator().is_evaluated(&Driver::data(1)));
            assert!(d.evaluator().won_slots.contains_key(&(2 * EPOCH - 1)));
            let requested = d.tables.len();
            d.tip_with_other_next_data(&genesis, EPOCH - 10, changed);
            assert_eq!(d.tables.len(), requested + 1, "change {changed}");
            assert!(!d.evaluator().is_evaluated(&Driver::data(1)));
            assert!(
                d.evaluator().won_slots.is_empty(),
                "the old data's win is dropped"
            );
            let tip = d.state.transition_frontier.best_tip().unwrap().clone();
            assert!(d.evaluator().next_won_slot(clock, &tip).is_none());
            d.complete_table(*d.tables.last().unwrap());
            let input = d.evaluations.last().unwrap();
            assert_eq!(input.global_slot, clock);
            assert_eq!(
                Some(&input.epoch_seed),
                d.evaluator().target().map(|t| &t.seed)
            );
        }
    }

    /// #213 review, blocking 2: a missing staking ledger is not an empty
    /// table. Nothing is evaluated (so nothing can be cached) until the
    /// ledger is there; the producer asks again after a while or on the next
    /// tip, and a fresh start evaluates with the real table.
    #[test]
    fn an_unavailable_ledger_is_never_evaluated_as_an_empty_table() {
        let genesis = genesis();
        // The tip's own epoch, and the clock in the next epoch.
        for (clock, tip_slot) in [(10, 9), (EPOCH, 4759)] {
            let mut d = Driver::new(&genesis, clock);
            d.tip(&genesis, tip_slot);
            d.missing_table(d.tables[0]);
            assert!(d.evaluations.is_empty(), "no VRF request from no table");
            assert!(d.evaluator().is_ledger_unavailable());
            assert!(d.evaluator().target().is_none());
            assert!(matches!(
                d.evaluator().status,
                Status::EpochEvaluationInterrupted {
                    reason: crate::block_producer::vrf_evaluator::InterruptReason::StakingLedgerUnavailable,
                    ..
                }
            ));
            let fake = VrfEvaluatorInput::new(
                Driver::data(clock / EPOCH).seed,
                Default::default(),
                clock,
                0,
                Driver::data(clock / EPOCH).ledger,
            )
            .with_generation(d.evaluator().generation());
            d.respond(&fake, clock, true);
            assert!(d.evaluator().won_slots.is_empty());
            // Not asked again at once, but after the retry interval.
            assert!(!BlockProducerAction::EvaluationRecheck.is_enabled(&d.state, d.state.time()));
            let later = at(genesis.slot_time(clock), 16);
            reduce(&mut d.state, later, crate::CheckTimeoutsAction {});
            d.dispatch(BlockProducerAction::EvaluationRecheck);
            assert_eq!(d.tables.len(), 2);
            d.missing_table(d.tables[1]);
            // A later tip asks too; then the ledger is there.
            d.tip(&genesis, tip_slot);
            assert_eq!(d.tables.len(), 3);
            d.complete_table(d.tables[2]);
            assert!(!d.evaluator().is_ledger_unavailable());
            d.assert_request(clock);
            // A fresh start after the ledger arrives evaluates the same slot.
            let mut restarted = Driver::new(&genesis, clock);
            restarted.tip(&genesis, tip_slot);
            restarted.complete_table(restarted.tables[0]);
            restarted.assert_current_and_future_wins(&genesis, clock);
        }
    }

    /// #213 review, blocking 1 (parent): a win is produced only on a parent
    /// that selects the data it was evaluated with. A best tip of another
    /// fork discards it before its block starts, and a parent changed since
    /// the last best tip update is caught when the block starts.
    #[test]
    fn a_win_is_produced_only_on_a_parent_selecting_its_epoch_data() {
        let genesis = genesis();
        for changed in 0..3 {
            // The parent changes without a best tip update.
            let mut d = Driver::new(&genesis, EPOCH);
            d.tip(&genesis, 4759);
            d.complete_table(d.tables[0]);
            d.win(EPOCH);
            assert_eq!(
                d.state
                    .block_producer
                    .current_won_slot()
                    .map(|w| w.global_slot()),
                Some(EPOCH)
            );
            let mut other = Driver::tip_block(&genesis, 4759);
            let next = &mut std::sync::Arc::make_mut(&mut other.block)
                .header
                .protocol_state
                .body
                .consensus_state
                .next_epoch_data;
            match changed {
                0 => next.seed = v2::EpochSeed::zero(),
                1 => next.ledger.hash = ledger_hash(),
                _ => next.ledger.total_currency = v2::CurrencyAmountStableV1(7u64.into()),
            }
            d.state.transition_frontier.best_chain = vec![AppliedBlock {
                block: other.clone(),
                just_emitted_a_proof: false,
            }];
            d.dispatch(BlockProducerAction::WonSlotProduceInit);
            assert!(!d.producing(), "change {changed}");
            assert!(matches!(
                d.state.block_producer.as_ref().unwrap().current,
                BlockProducerCurrentState::WonSlotDiscarded {
                    reason: BlockProducerWonSlotDiscardReason::EpochDataChanged,
                    ..
                }
            ));

            // With the best tip update, the win is dropped before that.
            let mut d = Driver::new(&genesis, EPOCH);
            d.tip(&genesis, 4759);
            d.complete_table(d.tables[0]);
            d.win(EPOCH);
            d.set_tip(other);
            assert!(!matches!(
                d.state.block_producer.as_ref().unwrap().current,
                BlockProducerCurrentState::WonSlot { .. }
                    | BlockProducerCurrentState::WonSlotWait { .. }
            ));
            assert!(!BlockProducerAction::WonSlotProduceInit.is_enabled(&d.state, d.state.time()));
            // The same slot, evaluated again with the new tip's data, is won
            // and produced: the dropped win was not the slot's attempt.
            d.complete_table(*d.tables.last().unwrap());
            let input = d.evaluations.last().unwrap().clone();
            assert_eq!(input.global_slot, EPOCH);
            d.respond(&input, EPOCH, true);
            d.dispatch(BlockProducerAction::WonSlotProduceInit);
            assert!(d.producing(), "change {changed}");
        }
    }

    /// A block of `slot` on a branch whose last block is `parent`: its
    /// staking data is `staking`, and its parent's hash is `parent`'s.
    fn sibling_after_empty_epoch(
        genesis: &Genesis,
        slot: u32,
        parent: &ArcBlockWithHash,
        staking: &v2::ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1,
    ) -> ArcBlockWithHash {
        let mut tip = Driver::tip_block(genesis, slot);
        tip.hash = v2::StateHash::from_fp(mina_curves::pasta::Fp::from(9_000u64 + slot as u64));
        let header = &mut std::sync::Arc::make_mut(&mut tip.block).header;
        header.protocol_state.previous_state_hash = parent.hash().clone();
        let cs = &mut header.protocol_state.body.consensus_state;
        cs.staking_epoch_data =
            serde_json::from_value(serde_json::to_value(staking).unwrap()).unwrap();
        cs.epoch_count = 1u32.into();
        tip
    }

    /// A win of epoch 2 kept from a fork with a block in epoch 1, whose next
    /// epoch data `next` the win was evaluated with, at clock `slot`.
    fn win_from_epoch_1_fork(
        genesis: &Genesis,
        slot: u32,
        next: &v2::ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1,
    ) -> Driver {
        let mut d = Driver::new(genesis, slot);
        let mut fork = Driver::tip_block(genesis, EPOCH + 100);
        std::sync::Arc::make_mut(&mut fork.block)
            .header
            .protocol_state
            .body
            .consensus_state
            .next_epoch_data = next.clone();
        d.set_tip(fork);
        d.complete_table(d.tables[0]);
        let input = d.evaluations.last().unwrap().clone();
        assert_eq!(input.global_slot, slot);
        d.respond(&input, slot, true);
        assert_eq!(
            d.state
                .block_producer
                .current_won_slot()
                .map(|w| w.global_slot()),
            Some(slot)
        );
        d
    }

    fn with_parent(d: &mut Driver, parent: &ArcBlockWithHash, tip: &ArcBlockWithHash) {
        d.state.transition_frontier.best_chain = [parent, tip]
            .into_iter()
            .map(|block| AppliedBlock {
                block: block.clone(),
                just_emitted_a_proof: false,
            })
            .collect();
    }

    /// #242 review, P1: a win of the best tip's slot is made on the tip's
    /// parent. On a branch with no block in epoch 1, that parent is in epoch
    /// 0 and selects no data for epoch 2 (select_epoch_data), but a child in
    /// epoch 2 still takes its next epoch data (update_pair), as the tip
    /// did. The win, evaluated with that data on another fork, is produced
    /// at once, not refused and found again.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/consensus/proof_of_stake.ml#L1062-L1109>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/block_producer/block_producer.ml#L714-L747>
    #[test]
    fn a_same_slot_win_after_an_empty_epoch_is_made_on_the_tips_parent() {
        let genesis = genesis();
        let slot = 2 * EPOCH + 5;
        let parent = Driver::tip_block(&genesis, EPOCH - 10);
        let next = parent.consensus_state().next_epoch_data.clone();
        let mut d = win_from_epoch_1_fork(&genesis, slot, &next);
        let tip = sibling_after_empty_epoch(&genesis, slot, &parent, &next);
        d.set_tip(tip.clone());
        with_parent(&mut d, &parent, &tip);
        let won = d.state.block_producer.current_won_slot().unwrap().clone();
        assert!(
            crate::block_producer::vrf_evaluator::EpochFingerprint::select(
                parent.consensus_state(),
                2
            )
            .is_none(),
            "two epochs past the parent: no data to evaluate with"
        );
        assert!(won.epoch_data_matches(parent.consensus_state()));
        d.dispatch(BlockProducerAction::WonSlotProduceInit);
        assert!(d.producing());
        let BlockProducerCurrentState::WonSlotTransactionsGet { chain, .. } =
            &d.state.block_producer.as_ref().unwrap().current
        else {
            unreachable!()
        };
        assert_eq!(chain.last().unwrap().hash(), parent.hash());
    }

    /// A win refused for its parent's epoch data is not chosen again for
    /// that parent, which would refuse it again at once. Here the tip's own
    /// staking data, made up to disagree with its parent's next epoch data,
    /// lets the search through. Another parent may still take it.
    #[test]
    fn a_win_refused_for_its_parent_is_not_chosen_again_for_that_parent() {
        let genesis = genesis();
        let slot = 2 * EPOCH + 5;
        let consistent = Driver::tip_block(&genesis, EPOCH - 10);
        let next = consistent.consensus_state().next_epoch_data.clone();
        let mut d = win_from_epoch_1_fork(&genesis, slot, &next);
        let mut other = Driver::tip_block(&genesis, EPOCH - 20);
        other.hash = v2::StateHash::from_fp(mina_curves::pasta::Fp::from(8_000u64));
        std::sync::Arc::make_mut(&mut other.block)
            .header
            .protocol_state
            .body
            .consensus_state
            .next_epoch_data
            .seed = v2::EpochSeed::zero();
        let tip = sibling_after_empty_epoch(&genesis, slot, &other, &next);
        d.set_tip(tip.clone());
        with_parent(&mut d, &other, &tip);
        d.dispatch(BlockProducerAction::WonSlotProduceInit);
        assert!(!d.producing());
        assert!(matches!(
            d.state.block_producer.as_ref().unwrap().current,
            BlockProducerCurrentState::WonSlotDiscarded {
                reason: BlockProducerWonSlotDiscardReason::EpochDataChanged,
                ..
            }
        ));
        assert!(
            d.evaluator().won_slots.contains_key(&slot),
            "kept for another parent"
        );
        assert!(!BlockProducerAction::WonSlotSearch.is_enabled(&d.state, d.state.time()));
        assert!(!BlockProducerAction::WonSlotProduceInit.is_enabled(&d.state, d.state.time()));
        // On a parent that takes the win's data, it is produced.
        let consistent_tip = sibling_after_empty_epoch(&genesis, slot, &consistent, &next);
        with_parent(&mut d, &consistent, &consistent_tip);
        d.dispatch(BlockProducerAction::WonSlotSearch);
        d.dispatch(BlockProducerAction::WonSlotProduceInit);
        assert!(d.producing());
    }

    /// OCaml checks the VRF again when the epoch ends, with no new block. A
    /// producer whose next epoch could not be evaluated ahead, here with the
    /// seed not yet final, evaluates it when the clock crosses the boundary.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/block_producer/block_producer.ml#L1117-L1125>
    #[test]
    fn crossing_the_epoch_boundary_without_a_block_evaluates_the_new_epoch() {
        let genesis = genesis();
        let mut d = Driver::new(&genesis, 100);
        d.mark_evaluated(0);
        d.tip(&genesis, 99);
        assert!(d.tables.is_empty(), "seed not final: no precomputation");
        assert!(!BlockProducerAction::EvaluationRecheck.is_enabled(&d.state, d.state.time()));
        d.wake(&genesis, EPOCH + 5);
        assert!(BlockProducerAction::EvaluationRecheck.is_enabled(&d.state, d.state.time()));
        d.dispatch(BlockProducerAction::EvaluationRecheck);
        assert_eq!(d.evaluator().target(), Some(&Driver::data(1)));
        assert!(!BlockProducerAction::EvaluationRecheck.is_enabled(&d.state, d.state.time()));
        d.complete_table(d.tables[0]);
        d.assert_current_and_future_wins(&genesis, EPOCH + 5);
    }

    /// An evaluation of an epoch the clock has left is restarted on the
    /// next epoch instead of finishing the past slots first.
    #[test]
    fn an_evaluation_the_clock_has_passed_is_replaced() {
        let genesis = genesis();
        let mut d = Driver::new(&genesis, 7000);
        d.tip(&genesis, 6999);
        d.complete_table(d.tables[0]);
        d.assert_request(7000);
        let old = d.evaluations.last().unwrap().clone();
        d.wake(&genesis, EPOCH + 2);
        d.dispatch(BlockProducerAction::EvaluationRecheck);
        assert_eq!(d.evaluator().target(), Some(&Driver::data(1)));
        d.respond(&old, 7000, false);
        assert_eq!(d.evaluations.len(), 1, "no further epoch-0 slot");
        d.complete_table(*d.tables.last().unwrap());
        d.assert_current_and_future_wins(&genesis, EPOCH + 2);
    }

    #[test]
    fn fix_round_1_terminal_epoch_tip_starts_without_overflow_or_next_epoch() {
        let genesis = genesis();
        // Exercise BestTipUpdate itself, rather than an injected pending state.
        // The clock stays representable; the protocol slot is deliberately extreme.
        for tip_slot in [u32::MAX / EPOCH * EPOCH, u32::MAX] {
            // Slot 6000 also opens the seed-finalized precomputation gate.
            let mut d = Driver::new(&genesis, 6000);
            d.tip(&genesis, tip_slot);
            assert!(d.evaluator().is_delegator_table_requested());
            assert_eq!(d.tables.len(), 1);
            d.dispatch(VrfAction::InterruptEpochEvaluation {
                reason: crate::block_producer::vrf_evaluator::InterruptReason::EpochDataStale,
            });
            d.mark_evaluated(tip_slot / EPOCH);
            d.tip(&genesis, tip_slot);
            assert!(matches!(
                d.evaluator().status,
                Status::WaitingForNextEvaluation { .. }
            ));
            assert_eq!(d.tables.len(), 1, "the terminal epoch has no next table");
        }
    }

    #[test]
    fn epoch_bounds_include_maximum_protocol_slot() {
        // Compiled OCaml Global_slot.slot outputs, retained by BP-MN-3.
        for (slot, expected) in [
            (0, "Beginning"),
            (7139, "End"),
            (7140, "Beginning"),
            (u32::MAX, "Within"),
        ] {
            assert_eq!(
                format!(
                    "{:?}",
                    BlockProducerVrfEvaluatorState::evaluate_epoch_bounds(&slot)
                ),
                expected
            );
        }
    }

    #[test]
    fn late_current_table_restarts_after_epoch_catchup() {
        let genesis = genesis();
        // A boundary during sleep, then a sleep lasting more than an epoch.
        for wake in [EPOCH, 2 * EPOCH + 1] {
            for catchup_before_table in [false, true] {
                let mut d = Driver::new(&genesis, 10);
                d.tip(&genesis, 9);
                let old = d.tables[0];
                d.wake(&genesis, wake);
                if catchup_before_table {
                    d.tip(&genesis, wake - 1 + u32::from(wake == EPOCH));
                }
                d.complete_table(old);
                assert!(
                    d.evaluations.is_empty(),
                    "old epoch must never evaluate new slots: {:?}",
                    d.evaluations
                );
                if !catchup_before_table {
                    if wake == EPOCH {
                        // The clock is in the tip's next epoch: OCaml evaluates
                        // it now with the tip's next epoch data.
                        assert!(d.evaluator().is_delegator_table_requested());
                        assert_eq!(d.evaluator().target(), Some(&Driver::data(1)));
                    } else {
                        assert!(matches!(
                            d.evaluator().status,
                            Status::WaitingForNextEvaluation { .. }
                        ));
                    }
                    d.tip(&genesis, wake - 1 + u32::from(wake == EPOCH));
                }
                assert!(d.evaluator().is_delegator_table_requested());
                let current = *d.tables.last().unwrap();
                assert_ne!(old, current);
                d.complete_table(current);
                d.assert_current_and_future_wins(&genesis, wake);
            }
        }
    }

    #[test]
    fn late_table_within_epoch_keeps_the_current_slot() {
        let genesis = genesis();
        let mut d = Driver::new(&genesis, 10);
        d.tip(&genesis, 9);
        d.wake(&genesis, 100);
        d.complete_table(d.tables[0]);
        d.assert_current_and_future_wins(&genesis, 100);
    }

    #[test]
    fn next_epoch_table_remains_valid_when_that_epoch_becomes_current() {
        let genesis = genesis();
        for wake in [EPOCH, EPOCH + 100] {
            let mut d = Driver::new(&genesis, EPOCH - 2);
            d.mark_evaluated(0);
            d.tip(&genesis, EPOCH - 3);
            assert!(d.evaluator().is_delegator_table_requested());
            d.wake(&genesis, wake);
            d.tip(&genesis, wake);
            d.complete_table(d.tables[0]);
            assert_eq!(d.tables.len(), 1, "retain the still-valid next-epoch table");
            d.assert_current_and_future_wins(&genesis, wake);
        }
    }

    #[test]
    fn restart_during_table_construction_uses_current_epoch_and_slot() {
        let genesis = genesis();
        for recorded in [false, true] {
            let mut d = Driver::new(&genesis, 10);
            d.tip(&genesis, 9);
            let old = d.tables[0];
            let bytes = serde_json::to_vec(d.evaluator()).unwrap();
            let wake = 2 * EPOCH + 10;
            if recorded {
                d.state.block_producer.as_mut().unwrap().vrf_evaluator =
                    serde_json::from_slice(&bytes).unwrap();
                d.wake(&genesis, wake);
                d.tip(&genesis, wake - 1);
                d.complete_table(old);
            } else {
                d = Driver::new(&genesis, wake);
                d.tip(&genesis, wake - 1);
            }
            d.complete_table(*d.tables.last().unwrap());
            d.assert_current_and_future_wins(&genesis, wake);
        }
    }

    #[test]
    fn a_late_past_win_is_skipped_without_losing_current_or_future_wins() {
        let genesis = genesis();
        let mut d = Driver::new(&genesis, 10);
        d.tip(&genesis, 9);
        d.complete_table(d.tables[0]);
        // The VRF worker's response, instead of the ledger response, is late.
        let requested = d.evaluations.last().unwrap().global_slot;
        d.wake(&genesis, 100);
        for slot in requested..100 {
            d.win(slot);
        }
        assert!(d.state.block_producer.current_won_slot().is_none());
        d.assert_current_and_future_wins(&genesis, 100);
    }

    #[test]
    fn slot_selection_uses_execution_time_after_a_queued_action_stalls() {
        let genesis = genesis();
        for wake in [100, 2 * EPOCH + 1] {
            let mut d = Driver::new(&genesis, 10);
            d.tip(&genesis, 9);
            let now = d.state.time();
            let select = reduce_collect(
                &mut d.state,
                now,
                VrfAction::FinalizeDelegatorTableConstruction {
                    delegator_table: Some(Default::default()),
                },
            )
            .into_iter()
            .find(|a| {
                matches!(
                    a,
                    Action::BlockProducer(BlockProducerAction::VrfEvaluator(
                        VrfAction::SelectInitialSlot { .. }
                    ))
                )
            })
            .unwrap();
            // No clock tick or best-tip event between these two actions.
            for next in reduce_collect(&mut d.state, genesis.slot_time(wake), select) {
                d.dispatch(next);
            }
            if wake < EPOCH {
                d.assert_current_and_future_wins(&genesis, wake);
            } else {
                assert!(d.evaluations.is_empty());
                assert!(matches!(
                    d.evaluator().status,
                    Status::WaitingForNextEvaluation { .. }
                ));
            }
        }
    }

    #[test]
    fn an_expired_next_epoch_table_is_replaced_after_a_multi_epoch_sleep() {
        let genesis = genesis();
        let mut d = Driver::new(&genesis, EPOCH - 2);
        d.mark_evaluated(0);
        d.tip(&genesis, EPOCH - 3);
        d.wake(&genesis, 2 * EPOCH + 10);
        d.complete_table(d.tables[0]);
        assert!(d.evaluations.is_empty());
        d.tip(&genesis, 2 * EPOCH + 9);
        d.complete_table(*d.tables.last().unwrap());
        d.assert_current_and_future_wins(&genesis, 2 * EPOCH + 10);
    }

    #[test]
    fn a_table_response_from_an_interrupted_request_cannot_replace_the_new_table() {
        let genesis = genesis();
        let mut d = Driver::new(&genesis, 10);
        d.tip(&genesis, 9);
        let old = d.tables[0];
        d.dispatch(VrfAction::InterruptEpochEvaluation {
            reason: crate::block_producer::vrf_evaluator::InterruptReason::BestTipWithHigherEpoch,
        });
        d.wake(&genesis, 2 * EPOCH + 10);
        d.tip(&genesis, 2 * EPOCH + 9);
        let current = *d.tables.last().unwrap();
        assert_ne!(old, current);
        d.complete_table(old);
        assert!(d.evaluations.is_empty());
        assert!(d.evaluator().is_delegator_table_requested());
        d.complete_table(current);
        d.assert_current_and_future_wins(&genesis, 2 * EPOCH + 10);
    }

    #[test]
    fn seed_ledger_and_stake_are_revalidated_even_within_the_same_epoch() {
        let genesis = genesis();
        for changed in 0..3 {
            let mut d = Driver::new(&genesis, 10);
            d.tip(&genesis, 9);
            let mut tip = d.state.transition_frontier.best_tip().unwrap().clone();
            let epoch_data = &mut std::sync::Arc::make_mut(&mut tip.block)
                .header
                .protocol_state
                .body
                .consensus_state
                .staking_epoch_data;
            match changed {
                0 => {
                    epoch_data.seed =
                        v2::EpochSeed::from(v2::MinaBaseEpochSeedStableV1(BigInt::zero()))
                }
                1 => epoch_data.ledger.hash = ledger_hash(),
                _ => epoch_data.ledger.total_currency = v2::CurrencyAmountStableV1(2000u64.into()),
            }
            let expected = epoch_data.clone();
            d.state.transition_frontier.best_chain[0].block = tip.clone();
            d.dispatch(BlockProducerAction::BestTipUpdate { best_tip: tip });
            // The update restarts the evaluation at once. A table depends on
            // the ledger and key alone: the pending read of the same ledger
            // serves the new request, and another ledger is read anew.
            assert!(d.evaluator().is_delegator_table_requested());
            assert_eq!(
                d.evaluator().target().map(|t| &t.seed),
                Some(&expected.seed)
            );
            assert_eq!(d.tables.len(), if changed == 1 { 2 } else { 1 });
            d.complete_table(d.tables[0]);
            if changed == 1 {
                assert!(d.evaluations.is_empty());
                assert!(d.evaluator().is_delegator_table_requested());
                d.complete_table(d.tables[1]);
            }
            let input = d.evaluations.last().unwrap();
            assert_eq!(input.global_slot, 10);
            assert_eq!(input.epoch_seed, expected.seed);
            assert_eq!(input.staking_ledger_hash, expected.ledger.hash);
            assert_eq!(
                input.total_currency,
                expected.ledger.total_currency.as_u64()
            );
            d.win(10);
            assert!(BlockProducerAction::WonSlotProduceInit.is_enabled(&d.state, d.state.time()));
        }
    }

    #[test]
    fn a_table_arriving_in_the_last_slot_does_not_evaluate_the_next_epoch() {
        let genesis = genesis();
        let mut d = Driver::new(&genesis, 10);
        d.tip(&genesis, 9);
        d.wake(&genesis, EPOCH - 1);
        d.complete_table(d.tables[0]);
        d.assert_request(EPOCH - 1);
        d.win(EPOCH - 1);
        assert_eq!(d.evaluations.len(), 1);
        assert_eq!(d.evaluator().last_evaluated_epoch(), Some(0));
        assert!(BlockProducerAction::WonSlotProduceInit.is_enabled(&d.state, d.state.time()));
    }
}

/// The existing frontier commit guard protects parent capture independently
/// of pending pool reconciliation.
#[test]
fn transaction_pool_pending_reconciliation_preserves_the_parent_commit_guard() {
    use crate::{
        transaction_pool::TransactionPoolEffectfulAction,
        transition_frontier::sync::TransitionFrontierSyncState, TransactionPoolAction,
    };

    fn dispatch(state: &mut State, time: Timestamp, action: Action, ran: &mut Vec<Action>) {
        if !action.is_enabled(state, time) {
            return;
        }
        ran.push(action.clone());
        let mut dispatcher = Dispatcher::new();
        crate::reducer(
            state,
            &ActionMeta::zero_custom(time).with_action(action),
            &mut dispatcher,
        );
        while let Some(next) = dispatcher.pop() {
            dispatch(state, time, next, ran);
        }
    }

    let genesis = genesis();
    let slot = 3;
    let now = genesis.slot_time(slot);
    let mut state = node(&genesis, now, &[slot]);
    state.transition_frontier.genesis = TransitionFrontierGenesisState::ProveSuccess {
        time: now,
        genesis: genesis.block.clone(),
    };
    state.block_producer.as_mut().unwrap().current = BlockProducerCurrentState::WonSlotWait {
        time: now,
        won_slot: genesis.won_slot(slot),
    };
    assert!(BlockProducerAction::WonSlotProduceInit.is_enabled(&state, now));
    let mut ran = Vec::new();
    dispatch(
        &mut state,
        now,
        TransactionPoolAction::BestTipChanged {
            best_tip_hash: ledger_hash(),
            diff: None,
        }
        .into(),
        &mut ran,
    );
    let _id = ran
        .iter()
        .find_map(|a| match a {
            Action::TransactionPoolEffect(TransactionPoolEffectfulAction::FetchAccounts {
                pending_id,
                ..
            }) => *pending_id,
            _ => None,
        })
        .unwrap();
    assert!(BlockProducerAction::WonSlotProduceInit.is_enabled(&state, now));
    assert!(matches!(
        state.block_producer.as_ref().unwrap().current,
        BlockProducerCurrentState::WonSlotWait { .. }
    ));
    state.transition_frontier.sync = TransitionFrontierSyncState::CommitPending {
        time: now,
        chain: vec![],
        root_snarked_ledger_updates: Default::default(),
        needed_protocol_states: Default::default(),
    };
    assert!(
        !BlockProducerAction::WonSlotProduceInit.is_enabled(&state, now),
        "the existing frontier commit guard still applies"
    );
    let new_tip = block_at(
        &genesis,
        2,
        3,
        &genesis.block.consensus_state().last_vrf_output,
        &genesis.hash,
    );
    state.transition_frontier.best_chain = vec![AppliedBlock {
        block: new_tip,
        just_emitted_a_proof: false,
    }];
    state.transition_frontier.sync = TransitionFrontierSyncState::Synced { time: now };
    let expected_parent = state.transition_frontier.best_tip().unwrap().clone();
    ran.clear();
    dispatch(
        &mut state,
        now,
        BlockProducerAction::WonSlotProduceInit.into(),
        &mut ran,
    );
    let BlockProducerCurrentState::WonSlotTransactionsSuccess { chain, .. } =
        &state.block_producer.as_ref().unwrap().current
    else {
        panic!("selection must finish in the same dispatch")
    };
    assert!(std::sync::Arc::ptr_eq(
        &chain.last().unwrap().block.block,
        &expected_parent.block
    ));
    assert!(ran.iter().any(|a| matches!(
        a,
        Action::BlockProducerEffectful(
            crate::BlockProducerEffectfulAction::StagedLedgerDiffCreateInit
        )
    )));
}

/// Keeps the review's withheld-result/slot-expiry reproduction, with the
/// required behavior: the pool contributes zero wait to parent capture and
/// selection. Ledger/prover completions below are controlled state-machine
/// inputs, not a measurement of cryptographic proving or a stuck ledger worker.
#[test]
fn review_198_stalled_reconciliation_does_not_discard_a_won_slot() {
    use crate::{
        transaction_pool::{
            account_read_tests::{accounts, command},
            submission_tests::{run, synced_state},
            TransactionPoolEffectfulAction, MAX_ACCOUNT_READS,
        },
        TransactionPoolAction,
    };
    use mina_core::transaction::TransactionPoolMessageSource;

    fn dispatch(state: &mut State, time: Timestamp, action: Action, ran: &mut Vec<Action>) {
        if !action.is_enabled(state, time) {
            return;
        }
        ran.push(action.clone());
        let mut dispatcher = Dispatcher::new();
        crate::reducer(
            state,
            &ActionMeta::zero_custom(time).with_action(action),
            &mut dispatcher,
        );
        while let Some(next) = dispatcher.pop() {
            dispatch(state, time, next, ran);
        }
    }
    fn read_ids(ran: &[Action]) -> Vec<u32> {
        ran.iter()
            .filter_map(|a| match a {
                Action::TransactionPoolEffect(TransactionPoolEffectfulAction::FetchAccounts {
                    pending_id,
                    ..
                }) => *pending_id,
                _ => None,
            })
            .collect()
    }

    let genesis = genesis();
    let slot = 3;
    let now = genesis.slot_time(slot);
    for scenario in ["withheld", "late", "cancelled", "failed"] {
        let mut state = node(&genesis, now, &[slot]);
        state.transition_frontier.genesis = TransitionFrontierGenesisState::ProveSuccess {
            time: now,
            genesis: genesis.block.clone(),
        };
        state.block_producer.as_mut().unwrap().current = BlockProducerCurrentState::WonSlotWait {
            time: now,
            won_slot: genesis.won_slot(slot),
        };
        // A real OCaml command already admitted before reconciliation started.
        let mut ready = synced_state();
        let pending = run(
            &mut ready,
            TransactionPoolAction::VerifySuccess {
                valids: vec![command()],
                from_source: TransactionPoolMessageSource::None,
            },
        );
        run(
            &mut ready,
            TransactionPoolAction::AccountsFetched {
                pending_id: read_ids(&pending)[0],
                accounts: Ok(accounts()),
            },
        );
        assert_eq!(ready.transaction_pool.size(), 1);
        state.transaction_pool = ready.transaction_pool;
        let expected = state.transaction_pool.list_includable_transactions(10);
        assert_eq!(expected.len(), 1);
        let mut ran = Vec::new();
        let mut cancelled = Vec::new();
        if scenario == "cancelled" {
            for _ in 0..MAX_ACCOUNT_READS {
                ran.clear();
                dispatch(
                    &mut state,
                    now,
                    TransactionPoolAction::VerifySuccess {
                        valids: vec![command()],
                        from_source: TransactionPoolMessageSource::None,
                    }
                    .into(),
                    &mut ran,
                );
                cancelled.extend(read_ids(&ran));
            }
            assert_eq!(cancelled.len(), MAX_ACCOUNT_READS);
        }
        ran.clear();
        dispatch(
            &mut state,
            now,
            TransactionPoolAction::BestTipChanged {
                best_tip_hash: ledger_hash(),
                diff: None,
            }
            .into(),
            &mut ran,
        );
        let reconcile = read_ids(&ran).first().copied();
        if scenario == "cancelled" {
            assert!(
                reconcile.is_none(),
                "cancelled reads still occupy all four slots"
            );
        } else {
            assert!(reconcile.is_some());
        }
        if scenario == "failed" {
            dispatch(
                &mut state,
                now,
                TransactionPoolAction::AccountsFetched {
                    pending_id: reconcile.unwrap(),
                    accounts: Err("Ledger unavailable".into()),
                }
                .into(),
                &mut ran,
            );
        }
        assert!(BlockProducerAction::WonSlotProduceInit.is_enabled(&state, now));
        let parent = state.transition_frontier.best_tip().unwrap().clone();
        ran.clear();
        dispatch(
            &mut state,
            now,
            BlockProducerAction::WonSlotProduceInit.into(),
            &mut ran,
        );
        let BlockProducerCurrentState::WonSlotTransactionsSuccess {
            time,
            chain,
            transactions_by_fee,
            ..
        } = &state.block_producer.as_ref().unwrap().current
        else {
            panic!("selection was delayed: {scenario}")
        };
        assert_eq!(*time, now, "zero pool wait budget");
        assert!(std::sync::Arc::ptr_eq(
            &chain.last().unwrap().block.block,
            &parent.block
        ));
        assert_eq!(transactions_by_fee, &vec![expected[0].data.clone()]);
        assert!(
            read_ids(&ran).is_empty(),
            "production must not request pool accounts"
        );
        assert!(ran.iter().any(|a| matches!(
            a,
            Action::BlockProducerEffectful(
                crate::BlockProducerEffectfulAction::StagedLedgerDiffCreateInit
            )
        )));

        // The external block worker/prover completes at +6 s, the native
        // producer's existing estimate, with 84 s left in the 90 s slot.
        // No pool result is supplied before this completion.
        let completed = at(now, 6);
        let BlockProducerCurrentState::BlockProvePending {
            won_slot,
            chain,
            mut block,
            ..
        } = proving(&genesis, completed, slot)
        else {
            unreachable!()
        };
        block
            .protocol_state
            .body
            .consensus_state
            .curr_global_slot_since_hard_fork
            .slot_number =
            v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(slot.into());
        block.protocol_state.body.consensus_state.blockchain_length =
            (genesis.block.height() + 1).into();
        block.protocol_state.body.blockchain_state.timestamp.0 =
            (u64::from(now) / 1_000_000).into();
        block.protocol_state.previous_state_hash = genesis.hash.clone();
        let block_hash = block.protocol_state.try_hash().unwrap();
        state.block_producer.as_mut().unwrap().current =
            BlockProducerCurrentState::BlockProveSuccess {
                time: completed,
                won_slot,
                chain,
                block,
                block_hash,
                proof: genesis.block.header().protocol_state_proof.clone(),
            };
        reduce(&mut state, completed, BlockProducerAction::BlockProduced);
        assert!(BlockProducerAction::BlockInject.is_enabled(&state, completed));
        reduce(&mut state, completed, BlockProducerAction::BlockInjected);
        assert!(matches!(
            state.block_producer.as_ref().unwrap().current,
            BlockProducerCurrentState::Injected { .. }
        ));
        let end = genesis.won_slot(slot).next_slot_time();
        assert_eq!(end.checked_sub(completed).unwrap().as_secs(), 84);
        assert!(!BlockProducerAction::WonSlotDiscard {
            reason: BlockProducerWonSlotDiscardReason::WonSlotExpired
        }
        .is_enabled(&state, end));
        if scenario == "late" {
            dispatch(
                &mut state,
                at(end, 1),
                TransactionPoolAction::AccountsFetched {
                    pending_id: reconcile.unwrap(),
                    accounts: Ok(accounts()),
                }
                .into(),
                &mut ran,
            );
        } else if scenario == "cancelled" {
            for id in cancelled {
                dispatch(
                    &mut state,
                    at(end, 1),
                    TransactionPoolAction::AccountsFetched {
                        pending_id: id,
                        accounts: Err("transaction account read cancelled".into()),
                    }
                    .into(),
                    &mut ran,
                );
            }
        }
        assert!(matches!(
            state.block_producer.as_ref().unwrap().current,
            BlockProducerCurrentState::Injected { .. }
        ));
    }
}
