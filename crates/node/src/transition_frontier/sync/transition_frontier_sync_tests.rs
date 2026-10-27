//! The ledger offered first to copy when the root ledger sync changes
//! target, and the root ledger's target across best tip updates.

use std::sync::Arc;

use mina_core::block::{AppliedBlock, ArcBlockWithHash, Block, BlockWithHash};
use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    binprot::BinProtRead,
    list::List,
    rpc_kernel::ResponsePayload,
    v2::{LedgerHash, StateHash},
};
use redux::Timestamp;

use crate::{
    ledger::write::LedgersToKeep,
    transition_frontier::{
        genesis::{GenesisConfig, NonStakers},
        TransitionFrontierConfig, TransitionFrontierState,
    },
};

use super::{
    ledger::{
        snarked::TransitionFrontierSyncLedgerSnarkedState,
        staged::TransitionFrontierSyncLedgerStagedState, SyncLedgerTarget,
        TransitionFrontierSyncLedgerState,
    },
    transition_frontier_sync_effects::root_ledger_copy_candidates,
    transition_frontier_sync_reducer::staking_ledger_change_requires_sync,
    TransitionFrontierSyncLedgerPending, TransitionFrontierSyncState,
};

#[path = "fresh_sync_tests.rs"]
mod fresh_sync_tests;

#[path = "no_outcome_sync_tests.rs"]
mod no_outcome_sync_tests;

/// Mesa mainnet block 563900, from a `get_transition_chain` answer captured
/// from an OCaml node: the template for the blocks below.
fn template_block() -> Block {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../p2p-messages/tests/files/mesa/get_transition_chain_v2_563900_zkapp.bin"
    );
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let payload =
        ResponsePayload::<Option<List<Block>>>::binprot_read(&mut &bytes[..]).expect("decodes");
    let blocks = payload.0.expect("ok response").0.expect("some blocks");
    blocks.iter().next().expect("one block").clone()
}

fn ledger_hash(n: u64) -> LedgerHash {
    LedgerHash::from_fp(Fp::from(n))
}

/// A block `id` on top of `parent`, whose snarked ledger is `snarked`. Only
/// the fields the sync looks at are set; `id` also makes its staged ledger
/// hash its own.
fn block(
    template: &Block,
    id: u64,
    parent: Option<&ArcBlockWithHash>,
    snarked: u64,
) -> ArcBlockWithHash {
    let mut block = template.clone();
    let protocol_state = &mut block.header.protocol_state;
    if let Some(parent) = parent {
        protocol_state.previous_state_hash = parent.hash().clone();
    }
    let blockchain_state = &mut protocol_state.body.blockchain_state;
    blockchain_state
        .ledger_proof_statement
        .target
        .first_pass_ledger = ledger_hash(snarked);
    blockchain_state.staged_ledger_hash.non_snark.ledger_hash = ledger_hash(1_000 + id);
    BlockWithHash {
        hash: StateHash::from_fp(Fp::from(id)),
        block: Arc::new(block),
    }
}

/// A root ledger sync for `root`, its snarked ledger synced and the staged
/// ledger parts fetched: the staged ledger is being rebuilt.
fn reconstructing(
    root: &ArcBlockWithHash,
    best_tip: &ArcBlockWithHash,
) -> TransitionFrontierSyncLedgerPending {
    let target = SyncLedgerTarget::root(root).with_staged().unwrap();
    TransitionFrontierSyncLedgerPending {
        time: Timestamp::ZERO,
        best_tip: best_tip.clone(),
        root_block: root.clone(),
        blocks_inbetween: vec![],
        root_block_updates: vec![],
        ledger: TransitionFrontierSyncLedgerStagedState::ReconstructEmpty {
            time: Timestamp::ZERO,
            target,
        }
        .into(),
    }
}

/// R14: while the root ledger syncs during a bootstrap, the ledger offered
/// first to copy is the sync target root's, not the frontier root's (the
/// genesis block's).
#[test]
fn the_previous_root_is_the_sync_target_root_while_syncing() {
    let template = template_block();
    let genesis = block(&template, 1, None, 5);
    let sync_root = block(&template, 2, None, 7);
    let tip = block(&template, 10, None, 7);

    let config = GenesisConfig::Counts {
        whales: 1,
        fish: 1,
        non_stakers: NonStakers::None,
        constants: GenesisConfig::default_constants(0),
    };
    let mut frontier =
        TransitionFrontierState::new(TransitionFrontierConfig::new(Arc::new(config)), false);
    assert_eq!(frontier.previous_root_snarked_ledger_hash(), None);

    frontier.best_chain = vec![AppliedBlock {
        block: genesis.clone(),
        just_emitted_a_proof: false,
    }];
    assert_eq!(
        frontier.previous_root_snarked_ledger_hash(),
        Some(ledger_hash(5))
    );

    frontier.sync =
        TransitionFrontierSyncState::RootLedgerPending(reconstructing(&sync_root, &tip));
    assert_eq!(
        frontier.previous_root_snarked_ledger_hash(),
        Some(ledger_hash(7))
    );

    frontier.sync = TransitionFrontierSyncState::Synced {
        time: Timestamp::ZERO,
    };
    assert_eq!(
        frontier.previous_root_snarked_ledger_hash(),
        Some(ledger_hash(5))
    );
}

/// What the `BestTipUpdate` reducer does in `RootLedgerPending` for a best
/// tip that keeps both epoch ledgers.
fn best_tip_update(sync: &mut TransitionFrontierSyncLedgerPending, root: &ArcBlockWithHash) {
    let old_root = std::mem::replace(&mut sync.root_block, root.clone());
    sync.update_root_ledger_target(Timestamp::ZERO, old_root);
}

fn staged_target_block(sync: &TransitionFrontierSyncLedgerPending) -> Option<&StateHash> {
    match &sync.ledger {
        TransitionFrontierSyncLedgerState::Staged(
            TransitionFrontierSyncLedgerStagedState::ReconstructEmpty { target, .. },
        ) => Some(&target.staged.block_hash),
        _ => None,
    }
}

fn hashes(blocks: &[ArcBlockWithHash]) -> Vec<&StateHash> {
    blocks.iter().map(|block| block.hash()).collect()
}

/// No producer or ledger service: exercise the real sync reducer on a
/// previously applied frontier. Synthetic ancestry is a scheduling fixture,
/// not a cryptographic oracle. The expected reuse policy is OCaml's
/// transition_handler/processor.ml at 84b8d969, lines 242-297: attach to a
/// parent still in the frontier rather than bootstrap an older root.
fn sync_test_node(best_chain: Vec<AppliedBlock>) -> crate::State {
    use mina_snark::{get_srs, BlockVerifier, TransactionVerifier};

    let constants = mina_core::consensus::ConsensusConstants::create(
        mina_core::constants::constraint_constants(),
        &mina_core::constants::PROTOCOL_CONSTANTS,
    );
    let config = crate::Config {
        ledger: crate::config::LedgerConfig {},
        snark: crate::config::SnarkConfig {
            block_verifier_index: BlockVerifier::make(),
            block_verifier_srs: get_srs(),
            work_verifier_index: TransactionVerifier::make(),
            work_verifier_srs: get_srs(),
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
        transition_frontier: TransitionFrontierConfig::new(crate::DEVNET_CONFIG.clone()),
        archive: None,
        block_producer: None,
        global: crate::config::GlobalConfig {
            build: Box::new(crate::BuildEnv::get()),
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
    let mut state = crate::State::new(config, &constants, Timestamp::ZERO);
    state.transition_frontier.best_chain = best_chain;
    state.transition_frontier.sync = TransitionFrontierSyncState::Synced {
        time: Timestamp::ZERO,
    };
    state
}

#[test]
fn a_short_fork_with_a_retired_root_reuses_the_applied_frontier() {
    for pending in [false, true] {
        for snarked in [7, 8] {
            check_retired_root_short_fork(pending, snarked);
        }
    }
}

fn check_retired_root_short_fork(pending: bool, snarked: u64) {
    use super::{TransitionFrontierSyncAction, TransitionFrontierSyncBlockState};
    use redux::{ActionMeta, Dispatcher};

    let template = template_block();
    let retired = block(&template, 1, None, 7);
    let root = block(&template, 2, Some(&retired), snarked);
    let parent = block(&template, 3, Some(&root), 7);
    let competitor = block(&template, 4, Some(&parent), 7);
    let candidate = block(&template, 5, Some(&parent), 7);
    let applied = |block: &ArcBlockWithHash| AppliedBlock {
        block: block.clone(),
        just_emitted_a_proof: false,
    };
    let mut state = sync_test_node(vec![applied(&root), applied(&parent), applied(&competitor)]);
    if pending {
        state.transition_frontier.sync = TransitionFrontierSyncState::BlocksPending {
            time: Timestamp::ZERO,
            chain: state
                .transition_frontier
                .best_chain
                .iter()
                .cloned()
                .map(|block| TransitionFrontierSyncBlockState::ApplySuccess {
                    time: Timestamp::ZERO,
                    block,
                })
                .collect(),
            root_snarked_ledger_updates: Default::default(),
            needed_protocol_states: Default::default(),
        };
    }
    let action = TransitionFrontierSyncAction::BestTipUpdate {
        previous_root_snarked_ledger_hash: Some(ledger_hash(snarked)),
        best_tip: candidate.clone(),
        root_block: retired,
        blocks_inbetween: vec![root.hash().clone(), parent.hash().clone()],
        on_success: None,
    };
    // Reproduce the accepted action emitted when a proof completes after a
    // sibling has committed. Consensus admission is independent of root reuse.
    crate::reducer(
        &mut state,
        &ActionMeta::zero_custom(Timestamp::ZERO).with_action(action.into()),
        &mut Dispatcher::new(),
    );
    let TransitionFrontierSyncState::BlocksPending { chain, .. } = &state.transition_frontier.sync
    else {
        panic!("short fork must reuse applied ancestry, not sync a ledger");
    };
    assert_eq!(chain.len(), 3);
    assert_eq!(chain[0].block_hash(), root.hash());
    assert!(chain[0].is_apply_success());
    assert_eq!(chain[1].block_hash(), parent.hash());
    assert!(chain[1].is_apply_success());
    assert!(
        matches!(&chain[2], TransitionFrontierSyncBlockState::FetchSuccess { block, .. } if block.hash() == candidate.hash())
    );
    assert!(state.transition_frontier.sync.ledger().is_none());
}

#[test]
fn a_disconnected_fork_still_syncs_its_root_ledger() {
    use super::TransitionFrontierSyncAction;
    use redux::{ActionMeta, Dispatcher};

    let template = template_block();
    let root = block(&template, 1, None, 7);
    let tip = block(&template, 2, Some(&root), 7);
    let other_root = block(&template, 3, None, 8);
    let other_tip = block(&template, 4, Some(&other_root), 8);
    let mut state = sync_test_node(
        [root, tip]
            .into_iter()
            .map(|block| AppliedBlock {
                block,
                just_emitted_a_proof: false,
            })
            .collect(),
    );
    let action = TransitionFrontierSyncAction::BestTipUpdate {
        previous_root_snarked_ledger_hash: Some(ledger_hash(7)),
        best_tip: other_tip,
        root_block: other_root.clone(),
        blocks_inbetween: vec![],
        on_success: None,
    };
    crate::reducer(
        &mut state,
        &ActionMeta::zero_custom(Timestamp::ZERO).with_action(action.into()),
        &mut Dispatcher::new(),
    );
    let TransitionFrontierSyncState::RootLedgerPending(sync) = &state.transition_frontier.sync
    else {
        panic!("a disconnected root must still sync its ledger");
    };
    assert_eq!(sync.root_block.hash(), other_root.hash());
    assert!(matches!(
        sync.ledger,
        TransitionFrontierSyncLedgerState::Snarked(
            TransitionFrontierSyncLedgerSnarkedState::NumAccountsPending { .. }
        )
    ));
}

/// Replay public blocks captured by the no-key native observer. The candidate
/// and both chains were validated on devnet; only the submitted ancestry is
/// deliberately stale, exactly as with a producer finishing after a sibling.
#[test]
#[ignore = "needs a native devnet capture in MINA_FORK_REPLAY"]
fn replay_retired_root_from_native_devnet() {
    use super::{TransitionFrontierSyncAction, TransitionFrontierSyncBlockState};
    use redux::{ActionMeta, Dispatcher, EnablingCondition};

    #[derive(serde::Deserialize)]
    struct Capture {
        applied_chain: Vec<AppliedBlock>,
        captured_chain: Vec<AppliedBlock>,
        candidate: ArcBlockWithHash,
    }
    let path = std::env::var("MINA_FORK_REPLAY").expect("capture path");
    let capture: Capture =
        serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(path).unwrap()))
            .unwrap();
    let current_root = capture.applied_chain.first().unwrap().hash().clone();
    let retired_root = capture.captured_chain.first().unwrap().block.clone();
    let parent = capture.captured_chain.last().unwrap().hash().clone();
    assert_ne!(current_root, *retired_root.hash());
    assert_eq!(capture.candidate.pred_hash(), &parent);
    assert_eq!(capture.applied_chain.last().unwrap().pred_hash(), &parent);
    let candidate_hash = capture.candidate.hash().clone();
    let mut state = sync_test_node(capture.applied_chain);
    let action = TransitionFrontierSyncAction::BestTipUpdate {
        previous_root_snarked_ledger_hash: state
            .transition_frontier
            .previous_root_snarked_ledger_hash(),
        best_tip: capture.candidate,
        root_block: retired_root,
        blocks_inbetween: capture
            .captured_chain
            .iter()
            .skip(1)
            .map(|b| b.hash().clone())
            .collect(),
        on_success: None,
    };
    assert!(action.is_enabled(&state, Timestamp::ZERO));
    crate::reducer(
        &mut state,
        &ActionMeta::zero_custom(Timestamp::ZERO).with_action(action.into()),
        &mut Dispatcher::new(),
    );
    let TransitionFrontierSyncState::BlocksPending { chain, .. } = &state.transition_frontier.sync
    else {
        panic!("public sibling requested ledger sync despite its retained parent");
    };
    assert_eq!(chain.first().unwrap().block_hash(), &current_root);
    assert!(chain[..chain.len() - 1]
        .iter()
        .all(|b| b.is_apply_success()));
    assert!(
        matches!(chain.last().unwrap(), TransitionFrontierSyncBlockState::FetchSuccess { block, .. } if block.hash() == &candidate_hash)
    );
    assert!(state.transition_frontier.sync.ledger().is_none());
    eprintln!("retained root {current_root}, parent {parent}, sibling {candidate_hash}; {} applied ancestors, zero ledger sync", chain.len() - 1);
}

/// R4's ledgerless run: during the rebuild a best tip moved the root by one
/// block, which kept the staged target, then a sibling of that best tip came
/// with the same root. That reset the target, and the staged ledger parts
/// were fetched again (74 s) and the rebuild so far discarded (18 s).
#[test]
fn a_sibling_best_tip_keeps_the_staged_target() {
    let template = template_block();
    let root_0 = block(&template, 1, None, 7);
    let root_1 = block(&template, 2, Some(&root_0), 7);
    let tip_0 = block(&template, 10, None, 7);
    let mut sync = reconstructing(&root_0, &tip_0);

    // The root's extension is absorbed: the rebuild goes on for root 0.
    best_tip_update(&mut sync, &root_1);
    assert_eq!(hashes(&sync.root_block_updates), vec![root_0.hash()]);
    assert_eq!(staged_target_block(&sync), Some(root_0.hash()));

    // A sibling tip, with the same root.
    best_tip_update(&mut sync, &root_1);
    assert_eq!(sync.root_block.hash(), root_1.hash());
    assert_eq!(hashes(&sync.root_block_updates), vec![root_0.hash()]);
    assert_eq!(staged_target_block(&sync), Some(root_0.hash()));
}

#[test]
fn a_root_moved_back_drops_the_later_updates() {
    let template = template_block();
    let root_0 = block(&template, 1, None, 7);
    let root_1 = block(&template, 2, Some(&root_0), 7);
    let root_2 = block(&template, 3, Some(&root_1), 7);
    let tip = block(&template, 10, None, 7);
    let mut sync = reconstructing(&root_0, &tip);

    best_tip_update(&mut sync, &root_1);
    best_tip_update(&mut sync, &root_2);
    assert_eq!(
        hashes(&sync.root_block_updates),
        vec![root_0.hash(), root_1.hash()]
    );

    best_tip_update(&mut sync, &root_1);
    assert_eq!(hashes(&sync.root_block_updates), vec![root_0.hash()]);
    assert_eq!(staged_target_block(&sync), Some(root_0.hash()));

    best_tip_update(&mut sync, &root_0);
    assert!(sync.root_block_updates.is_empty());
    assert_eq!(staged_target_block(&sync), Some(root_0.hash()));
}

#[test]
fn a_root_on_another_branch_changes_the_target() {
    let template = template_block();
    let root_0 = block(&template, 1, None, 7);
    let root_1 = block(&template, 2, Some(&root_0), 7);
    let other_root = block(&template, 3, None, 7);
    let tip = block(&template, 10, None, 7);

    // Same snarked ledger: only the staged ledger starts again.
    let mut sync = reconstructing(&root_0, &tip);
    best_tip_update(&mut sync, &root_1);
    best_tip_update(&mut sync, &other_root);
    assert!(sync.root_block_updates.is_empty());
    let TransitionFrontierSyncLedgerState::Snarked(
        TransitionFrontierSyncLedgerSnarkedState::Success { target, .. },
    ) = &sync.ledger
    else {
        panic!("expected the snarked ledger kept, got {:?}", sync.ledger);
    };
    assert_eq!(
        target.staged.as_ref().map(|staged| &staged.block_hash),
        Some(other_root.hash())
    );

    // Another snarked ledger: the root ledger sync starts again.
    let mut sync = reconstructing(&root_0, &tip);
    best_tip_update(&mut sync, &block(&template, 4, None, 8));
    let TransitionFrontierSyncLedgerState::Init { target, .. } = &sync.ledger else {
        panic!("expected a new root ledger sync, got {:?}", sync.ledger);
    };
    assert_eq!(target.snarked_ledger_hash, ledger_hash(8));
}

const GENESIS_LEDGER: u64 = 1;
const STAKING_LEDGER: u64 = 2;
const NEXT_EPOCH_LEDGER: u64 = 3;

/// `block`, with the genesis, staking and next epoch ledgers of the tests
/// below.
fn in_epoch(block: ArcBlockWithHash) -> ArcBlockWithHash {
    let mut inner = (*block.block).clone();
    let body = &mut inner.header.protocol_state.body;
    body.blockchain_state.genesis_ledger_hash = ledger_hash(GENESIS_LEDGER);
    body.consensus_state.staking_epoch_data.ledger.hash = ledger_hash(STAKING_LEDGER);
    body.consensus_state.next_epoch_data.ledger.hash = ledger_hash(NEXT_EPOCH_LEDGER);
    BlockWithHash {
        hash: block.hash.clone(),
        block: Arc::new(inner),
    }
}

/// OCaml frontier_root_transition retains the old epoch ledgers until the
/// root crosses. The synthetic hashes identify that retention policy only.
#[test]
fn epoch_ledgers_survive_a_tip_boundary_until_the_root_crosses() {
    let template = template_block();
    let old_root = in_epoch(block(&template, 1, None, 11));
    let mut new_tip = (*in_epoch(block(&template, 2, Some(&old_root), 12)).block).clone();
    new_tip
        .header
        .protocol_state
        .body
        .consensus_state
        .blockchain_length =
        mina_p2p_messages::v2::UnsignedExtendedUInt32StableV1((old_root.height() + 1).into());
    new_tip
        .header
        .protocol_state
        .body
        .consensus_state
        .staking_epoch_data
        .ledger
        .hash = ledger_hash(NEXT_EPOCH_LEDGER);
    new_tip
        .header
        .protocol_state
        .body
        .consensus_state
        .next_epoch_data
        .ledger
        .hash = ledger_hash(4);
    let new_tip = BlockWithHash {
        hash: StateHash::from_fp(Fp::from(2)),
        block: Arc::new(new_tip),
    };
    let keep: LedgersToKeep = [&old_root, &new_tip].into_iter().collect();
    assert!(keep.contains(&ledger_hash(STAKING_LEDGER)));
    assert!(keep.contains(&ledger_hash(NEXT_EPOCH_LEDGER)));
    assert!(keep.contains(&ledger_hash(4)));

    assert!(!staking_ledger_change_requires_sync(&old_root, &new_tip));
    assert!(staking_ledger_change_requires_sync(&new_tip, &old_root));

    let crossed: LedgersToKeep = [&new_tip].into_iter().collect();
    assert!(!crossed.contains(&ledger_hash(STAKING_LEDGER)));
}

/// A restored node may hold only the newer epoch's ledgers. Even with an
/// applied root, switching its tip back must ask peers for the older staking
/// ledger before commit can check that ledger's hash.
#[test]
fn backward_epoch_tip_switch_enters_staking_ledger_sync() {
    use super::{TransitionFrontierSyncAction, TransitionFrontierSyncBlockState};
    use redux::{ActionMeta, Dispatcher};

    let template = template_block();
    let epoch_block = |id, parent: Option<&ArcBlockWithHash>, staking, next| {
        let mut block = block(&template, id, parent, 7);
        let inner = Arc::make_mut(&mut block.block);
        inner
            .header
            .protocol_state
            .body
            .consensus_state
            .staking_epoch_data
            .ledger
            .hash = ledger_hash(staking);
        inner
            .header
            .protocol_state
            .body
            .consensus_state
            .next_epoch_data
            .ledger
            .hash = ledger_hash(next);
        block
    };
    let root = epoch_block(1, None, 2, 3);
    let current_tip = epoch_block(2, Some(&root), 3, 4);
    let old_epoch_tip = epoch_block(3, Some(&root), 2, 3);
    let applied = |block: &ArcBlockWithHash| AppliedBlock {
        block: block.clone(),
        just_emitted_a_proof: false,
    };

    for pending in [false, true] {
        let mut state = sync_test_node(vec![applied(&root), applied(&current_tip)]);
        if pending {
            state.transition_frontier.sync = TransitionFrontierSyncState::BlocksPending {
                time: Timestamp::ZERO,
                chain: state
                    .transition_frontier
                    .best_chain
                    .iter()
                    .cloned()
                    .map(|block| TransitionFrontierSyncBlockState::ApplySuccess {
                        time: Timestamp::ZERO,
                        block,
                    })
                    .collect(),
                root_snarked_ledger_updates: Default::default(),
                needed_protocol_states: Default::default(),
            };
        }
        let action = TransitionFrontierSyncAction::BestTipUpdate {
            previous_root_snarked_ledger_hash: Some(ledger_hash(7)),
            best_tip: old_epoch_tip.clone(),
            root_block: root.clone(),
            blocks_inbetween: vec![],
            on_success: None,
        };
        crate::reducer(
            &mut state,
            &ActionMeta::zero_custom(Timestamp::ZERO).with_action(action.into()),
            &mut Dispatcher::new(),
        );
        assert!(matches!(
            state.transition_frontier.sync,
            TransitionFrontierSyncState::StakingLedgerPending(_)
        ));
        assert_eq!(
            state
                .transition_frontier
                .sync
                .ledger_target()
                .unwrap()
                .snarked_ledger_hash,
            ledger_hash(2)
        );
    }
}

/// Drive the backward switch through the sync reducer and its peer selection.
/// OCaml 4.0.2 sync_handler.ml:get_ledger_by_hash serves the old staking
/// snapshot until consensus/proof_of_stake.ml:frontier_root_transition rotates
/// the snapshots. A peer whose best tip has crossed can therefore answer even
/// though that tip no longer advertises the old hash.
#[test]
fn backward_epoch_switch_uses_local_ledger_or_moves_to_another_peer() {
    use super::{
        ledger::{
            snarked::{
                LedgerAddressQueryPending, PeerLedgerQueryError, PeerLedgerQueryResponse,
                PeerRpcState, TransitionFrontierSyncLedgerSnarkedAction as SnarkedAction,
                TransitionFrontierSyncLedgerSnarkedState,
            },
            TransitionFrontierSyncLedgerAction as LedgerAction, TransitionFrontierSyncLedgerState,
        },
        TransitionFrontierSyncAction,
    };
    use crate::p2p::{
        channels::{
            rpc::{
                P2pChannelsRpcAction, P2pChannelsRpcState, P2pRpcLocalState, P2pRpcRemoteState,
                P2pRpcRequest,
            },
            P2pChannelsAction,
        },
        P2pAction, P2pPeerState, P2pPeerStatus, P2pPeerStatusReady, PeerId,
    };
    use mina_p2p_messages::v2::MinaLedgerSyncLedgerQueryStableV1;
    use redux::{ActionMeta, Dispatcher, EnablingCondition};

    fn reduce(
        state: &mut crate::State,
        action: impl Into<crate::Action>,
    ) -> Dispatcher<crate::Action, crate::State> {
        let mut dispatcher = Dispatcher::new();
        crate::reducer(
            state,
            &ActionMeta::zero_custom(Timestamp::ZERO).with_action(action.into()),
            &mut dispatcher,
        );
        dispatcher
    }

    let template = template_block();
    let contents_hash = ledger_hash(42);
    let old_staking = crate::ledger::complete_num_accounts_tree_with_empties(&contents_hash, 1)
        .expect("valid hash");
    let epoch_block =
        |id, parent: Option<&ArcBlockWithHash>, staking: LedgerHash, next: LedgerHash| {
            let mut block = block(&template, id, parent, 7);
            let consensus = &mut Arc::make_mut(&mut block.block)
                .header
                .protocol_state
                .body
                .consensus_state;
            consensus.staking_epoch_data.ledger.hash = staking;
            consensus.next_epoch_data.ledger.hash = next;
            block
        };
    let root = epoch_block(1, None, old_staking.clone(), ledger_hash(3));
    let current_tip = epoch_block(2, Some(&root), ledger_hash(3), ledger_hash(4));
    let old_tip = epoch_block(3, Some(&root), old_staking.clone(), ledger_hash(3));
    let applied = |block: &ArcBlockWithHash| AppliedBlock {
        block: block.clone(),
        just_emitted_a_proof: false,
    };
    let switch = || TransitionFrontierSyncAction::BestTipUpdate {
        previous_root_snarked_ledger_hash: Some(ledger_hash(7)),
        best_tip: old_tip.clone(),
        root_block: root.clone(),
        blocks_inbetween: vec![],
        on_success: None,
    };

    // Forward rotation and a same-epoch fork keep the applied root path.
    let mut forward = sync_test_node(vec![applied(&root), applied(&old_tip)]);
    reduce(
        &mut forward,
        TransitionFrontierSyncAction::BestTipUpdate {
            previous_root_snarked_ledger_hash: Some(ledger_hash(7)),
            best_tip: current_tip.clone(),
            root_block: root.clone(),
            blocks_inbetween: vec![],
            on_success: None,
        },
    );
    assert!(matches!(
        forward.transition_frontier.sync,
        TransitionFrontierSyncState::BlocksPending { .. }
    ));
    let same_epoch_fork = epoch_block(4, Some(&root), old_staking.clone(), ledger_hash(3));
    let mut fork = sync_test_node(vec![applied(&root), applied(&old_tip)]);
    reduce(
        &mut fork,
        TransitionFrontierSyncAction::BestTipUpdate {
            previous_root_snarked_ledger_hash: Some(ledger_hash(7)),
            best_tip: same_epoch_fork,
            root_block: root.clone(),
            blocks_inbetween: vec![],
            on_success: None,
        },
    );
    assert!(matches!(
        fork.transition_frontier.sync,
        TransitionFrontierSyncState::BlocksPending { .. }
    ));

    // Exercise the local service check and reducer together. A fresh copy
    // needs no second mask copy; an older partial working mask is replaced.
    struct LocalService {
        now: redux::Instant,
        held: LedgerHash,
        copies: std::cell::Cell<usize>,
    }
    impl redux::Service for LocalService {}
    impl redux::TimeService for LocalService {
        fn monotonic_time(&mut self) -> redux::Instant {
            self.now
        }
    }
    impl super::ledger::snarked::TransitionFrontierSyncLedgerSnarkedService for LocalService {
        fn contains_snarked_ledger(&self, hash: &LedgerHash) -> bool {
            hash == &self.held
        }
        fn copy_snarked_ledger_contents_for_sync(
            &self,
            _origin: Vec<LedgerHash>,
            _target: LedgerHash,
            _overwrite: bool,
        ) -> Result<bool, String> {
            self.copies.set(self.copies.get() + 1);
            Ok(true)
        }
        fn compute_snarked_ledger_hashes(&self, _: &LedgerHash) -> Result<(), String> {
            unreachable!()
        }
        fn child_hashes_get(
            &self,
            _: LedgerHash,
            _: &crate::ledger::LedgerAddress,
            _: usize,
        ) -> Result<Vec<LedgerHash>, String> {
            unreachable!()
        }
        fn accounts_set(
            &self,
            _: LedgerHash,
            _: &crate::ledger::LedgerAddress,
            _: &LedgerHash,
            _: Vec<mina_p2p_messages::v2::MinaBaseAccountBinableArgStableV2>,
        ) -> Result<LedgerHash, String> {
            unreachable!()
        }
    }
    for (prepared_copy, expected_copies) in [(true, 0), (false, 1)] {
        let mut local = sync_test_node(vec![applied(&root), applied(&current_tip)]);
        reduce(&mut local, switch());
        let now = redux::Instant::now();
        let mut store = redux::Store::new(
            crate::reducer,
            |_, _| {},
            LocalService {
                now,
                held: old_staking.clone(),
                copies: std::cell::Cell::new(0),
            },
            redux::SystemTime::UNIX_EPOCH,
            local,
        );
        assert!(super::ledger::try_complete_snarked_ledger_locally(
            &mut store,
            prepared_copy,
        ));
        assert_eq!(store.service.copies.get(), expected_copies);
        assert!(matches!(
            store.state().transition_frontier.sync.ledger(),
            Some(TransitionFrontierSyncLedgerState::Snarked(
                TransitionFrontierSyncLedgerSnarkedState::Success { .. }
            ))
        ));
        assert!(!SnarkedAction::PeersQuery.is_enabled(store.state(), Timestamp::ZERO));
        assert!(store.dispatch(LedgerAction::Success));
        assert!(store.dispatch(TransitionFrontierSyncAction::LedgerStakingSuccess));
        assert!(matches!(
            store.state().transition_frontier.sync,
            TransitionFrontierSyncState::StakingLedgerSuccess { .. }
        ));
    }

    let mut missing = sync_test_node(vec![applied(&root), applied(&current_tip)]);
    let chain_id = serde_json::from_str(
        r#""0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886""#,
    )
    .unwrap();
    missing.p2p.initialize(&chain_id).unwrap();
    let peers = [PeerId::from_bytes([5; 32]), PeerId::from_bytes([6; 32])];
    for peer_id in peers {
        let mut ready = P2pPeerStatusReady::new(false, Timestamp::ZERO, &Default::default());
        ready.best_tip = Some(current_tip.clone());
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
        missing.p2p.ready_mut().unwrap().peers.insert(
            peer_id,
            P2pPeerState {
                is_libp2p: false,
                dial_opts: None,
                identify: None,
                status: P2pPeerStatus::Ready(Box::new(ready)),
            },
        );
    }
    reduce(&mut missing, switch());
    reduce(&mut missing, SnarkedAction::Pending);
    let mut query = reduce(&mut missing, SnarkedAction::PeersQuery);
    let Some(crate::Action::TransitionFrontier(
        crate::transition_frontier::TransitionFrontierAction::Sync(
            TransitionFrontierSyncAction::Ledger(LedgerAction::Snarked(
                SnarkedAction::PeerQueryNumAccountsInit { peer_id: first },
            )),
        ),
    )) = query.pop()
    else {
        panic!("old staking hash must be queried");
    };
    assert!(peers.contains(&first));
    let mut outbound = reduce(
        &mut missing,
        SnarkedAction::PeerQueryNumAccountsInit { peer_id: first },
    );
    let Some(crate::Action::P2p(P2pAction::Channels(P2pChannelsAction::Rpc(
        P2pChannelsRpcAction::RequestSend { request, .. },
    )))) = outbound.pop()
    else {
        panic!("old staking query was not sent");
    };
    assert!(matches!(
        *request,
        P2pRpcRequest::LedgerQuery(ref hash, MinaLedgerSyncLedgerQueryStableV1::NumAccounts)
            if hash == &old_staking
    ));
    reduce(
        &mut missing,
        SnarkedAction::PeerQueryNumAccountsPending {
            peer_id: first,
            rpc_id: 0,
        },
    );
    let mut retry = reduce(
        &mut missing,
        SnarkedAction::PeerQueryNumAccountsError {
            peer_id: first,
            rpc_id: 0,
            error: PeerLedgerQueryError::DataUnavailable,
        },
    );
    assert!(retry.pop().is_some());
    assert!(!SnarkedAction::PeerQueryNumAccountsRetry { peer_id: first }
        .is_enabled(&missing, Timestamp::ZERO));
    let mut query = reduce(&mut missing, SnarkedAction::PeersQuery);
    let Some(crate::Action::TransitionFrontier(
        crate::transition_frontier::TransitionFrontierAction::Sync(
            TransitionFrontierSyncAction::Ledger(LedgerAction::Snarked(
                SnarkedAction::PeerQueryNumAccountsInit { peer_id: second },
            )),
        ),
    )) = query.pop()
    else {
        panic!("must move to second peer");
    };
    assert_ne!(first, second);
    reduce(
        &mut missing,
        SnarkedAction::PeerQueryNumAccountsInit { peer_id: second },
    );
    reduce(
        &mut missing,
        SnarkedAction::PeerQueryNumAccountsPending {
            peer_id: second,
            rpc_id: 1,
        },
    );
    let mut response = reduce(
        &mut missing,
        SnarkedAction::PeerQueryNumAccountsSuccess {
            peer_id: second,
            rpc_id: 1,
            response: PeerLedgerQueryResponse::NumAccounts(1, contents_hash.clone()),
        },
    );
    assert!(response.pop().is_some());
    reduce(
        &mut missing,
        SnarkedAction::NumAccountsReceived {
            num_accounts: 1,
            contents_hash: contents_hash.clone(),
            sender: second,
        },
    );
    reduce(
        &mut missing,
        SnarkedAction::NumAccountsAccepted {
            num_accounts: 1,
            contents_hash: contents_hash.clone(),
            sender: second,
        },
    );
    reduce(
        &mut missing,
        SnarkedAction::NumAccountsSuccess {
            num_accounts: 1,
            contents_hash,
        },
    );
    reduce(&mut missing, SnarkedAction::MerkleTreeSyncPending);
    let Some(TransitionFrontierSyncLedgerState::Snarked(
        TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncPending {
            unavailable_peers,
            queue,
            ..
        },
    )) = missing.transition_frontier.sync.ledger()
    else {
        panic!("address sync");
    };
    assert!(unavailable_peers.contains(&first));
    assert!(!unavailable_peers.contains(&second));
    let (address, expected_hash) = queue.first_key_value().expect("address to fetch");
    let address_query = |peer_id| SnarkedAction::PeerQueryAddressInit {
        address: address.clone(),
        expected_hash: expected_hash.clone(),
        peer_id,
    };
    assert!(!address_query(first).is_enabled(&missing, Timestamp::ZERO));
    assert!(address_query(second).is_enabled(&missing, Timestamp::ZERO));

    // The address Retry action must accept a different unadvertised peer.
    let Some(TransitionFrontierSyncLedgerState::Snarked(
        TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncPending {
            queue,
            pending_addresses,
            ..
        },
    )) = missing.transition_frontier.sync.ledger_mut()
    else {
        panic!("address sync");
    };
    let (address, expected_hash) = queue.pop_first().unwrap();
    pending_addresses.insert(
        address.clone(),
        LedgerAddressQueryPending {
            time: Timestamp::ZERO,
            expected_hash,
            attempts: [(
                first,
                PeerRpcState::Error {
                    time: Timestamp::ZERO,
                    rpc_id: 2,
                    error: PeerLedgerQueryError::LedgerUnavailable,
                },
            )]
            .into_iter()
            .collect(),
        },
    );
    assert!(SnarkedAction::PeerQueryAddressRetry {
        address: address.clone(),
        peer_id: second,
    }
    .is_enabled(&missing, Timestamp::ZERO));
    reduce(
        &mut missing,
        SnarkedAction::PeerQueryAddressRetry {
            address: address.clone(),
            peer_id: second,
        },
    );
    reduce(
        &mut missing,
        SnarkedAction::PeerQueryAddressPending {
            address: address.clone(),
            peer_id: second,
            rpc_id: 3,
        },
    );
    reduce(
        &mut missing,
        SnarkedAction::PeerQueryAddressError {
            peer_id: second,
            rpc_id: 3,
            error: PeerLedgerQueryError::LedgerUnavailable,
        },
    );
    // Follow the scheduler action queued by the error reducer. A full pass
    // clears exclusions, but the SY1 delay still bounds retries.
    reduce(&mut missing, SnarkedAction::PeersQuery);
    let ledger = missing
        .transition_frontier
        .sync
        .ledger()
        .unwrap()
        .snarked()
        .unwrap();
    assert!(!ledger.peer_unavailable(&first));
    assert!(!ledger.peer_unavailable(&second));
    let retry = SnarkedAction::PeerQueryAddressRetry {
        address,
        peer_id: first,
    };
    assert!(!retry.is_enabled(&missing, Timestamp::ZERO));
    assert!(retry.is_enabled(
        &missing,
        Timestamp::ZERO + std::time::Duration::from_secs(5)
    ));

    // A previously excluded peer becomes eligible again if the peer that
    // answered the account-count query disconnects before address sync.
    missing.p2p.ready_mut().unwrap().peers.remove(&second);
    if let Some(TransitionFrontierSyncLedgerState::Snarked(
        TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncPending {
            unavailable_peers, ..
        },
    )) = missing.transition_frontier.sync.ledger_mut()
    {
        unavailable_peers.insert(first);
    }
    reduce(&mut missing, SnarkedAction::PeersQuery);
    assert!(!missing
        .transition_frontier
        .sync
        .ledger()
        .unwrap()
        .snarked()
        .unwrap()
        .peer_unavailable(&first));
}

/// A timeout during ordinary ledger sync must leave the only serving peer
/// retryable. Exercise account-count and address queries through the reducer.
#[test]
fn normal_ledger_sync_retries_sole_peer_after_timeout() {
    normal_ledger_sync_retries_peer_error(super::ledger::snarked::PeerLedgerQueryError::Timeout);
}

#[test]
fn normal_ledger_sync_bounds_refusals_and_recovers_after_backoff() {
    use super::ledger::snarked::PeerLedgerQueryError;
    for error in [
        PeerLedgerQueryError::LedgerUnavailable,
        PeerLedgerQueryError::DataUnavailable,
    ] {
        normal_ledger_sync_retries_peer_error(error);
    }
}

fn normal_ledger_sync_retries_peer_error(error: super::ledger::snarked::PeerLedgerQueryError) {
    use super::ledger::{
        snarked::{
            PeerLedgerQueryError, PeerLedgerQueryResponse,
            TransitionFrontierSyncLedgerSnarkedAction as SnarkedAction,
            TransitionFrontierSyncLedgerSnarkedState as SnarkedState,
        },
        SyncLedgerTargetKind,
    };
    use crate::p2p::{
        channels::rpc::{P2pChannelsRpcState, P2pRpcLocalState, P2pRpcRemoteState},
        P2pPeerState, P2pPeerStatus, P2pPeerStatusReady, PeerId,
    };
    use redux::{ActionMeta, Dispatcher, EnablingCondition};

    let now = std::cell::Cell::new(Timestamp::ZERO);
    let delay = if matches!(error, PeerLedgerQueryError::Timeout) {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_secs(5)
    };
    let reduce = |state: &mut crate::State, action: SnarkedAction| {
        crate::reducer(
            state,
            &ActionMeta::zero_custom(now.get()).with_action(action.into()),
            &mut Dispatcher::new(),
        );
    };

    let template = template_block();
    let contents_hash = ledger_hash(43);
    let target_hash = crate::ledger::complete_num_accounts_tree_with_empties(&contents_hash, 1)
        .expect("valid hash");
    let mut root = block(&template, 100, None, 7);
    {
        let protocol = &mut Arc::make_mut(&mut root.block).header.protocol_state;
        protocol
            .body
            .blockchain_state
            .ledger_proof_statement
            .target
            .first_pass_ledger = target_hash.clone();
        protocol.body.consensus_state.staking_epoch_data.ledger.hash = target_hash.clone();
        protocol.body.consensus_state.next_epoch_data.ledger.hash = target_hash.clone();
    }
    let peer_id = PeerId::from_bytes([11; 32]);
    let chain_id = serde_json::from_str(
        r#""0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886""#,
    )
    .unwrap();

    for kind in [
        SyncLedgerTargetKind::StakingEpoch,
        SyncLedgerTargetKind::NextEpoch,
        SyncLedgerTargetKind::Root,
    ] {
        now.set(Timestamp::ZERO);
        let mut state = sync_test_node(vec![AppliedBlock {
            block: root.clone(),
            just_emitted_a_proof: false,
        }]);
        state.p2p.initialize(&chain_id).unwrap();
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
            peer_id,
            P2pPeerState {
                is_libp2p: false,
                dial_opts: None,
                identify: None,
                status: P2pPeerStatus::Ready(Box::new(ready)),
            },
        );
        let mut target = match kind {
            SyncLedgerTargetKind::StakingEpoch => SyncLedgerTarget::staking_epoch(&root),
            SyncLedgerTargetKind::NextEpoch => SyncLedgerTarget {
                kind,
                snarked_ledger_hash: target_hash.clone(),
                staged: None,
            },
            SyncLedgerTargetKind::Root => SyncLedgerTarget::root(&root),
        };
        // Root sync uses its snarked stage after the staged target is prepared.
        target.staged = None;
        let pending = TransitionFrontierSyncLedgerPending {
            time: Timestamp::ZERO,
            best_tip: root.clone(),
            root_block: root.clone(),
            blocks_inbetween: vec![],
            root_block_updates: vec![],
            ledger: SnarkedState::pending(Timestamp::ZERO, target).into(),
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

        let init = SnarkedAction::PeerQueryNumAccountsInit { peer_id };
        assert!(init.is_enabled(&state, now.get()), "{kind:?}");
        reduce(&mut state, init);
        reduce(
            &mut state,
            SnarkedAction::PeerQueryNumAccountsPending { peer_id, rpc_id: 0 },
        );
        assert!(
            !SnarkedAction::PeerQueryNumAccountsRetry { peer_id }.is_enabled(&state, now.get()),
            "a pending attempt must not be overwritten"
        );
        reduce(
            &mut state,
            SnarkedAction::PeerQueryNumAccountsError {
                peer_id,
                rpc_id: 0,
                error: error.clone(),
            },
        );
        let retry = SnarkedAction::PeerQueryNumAccountsRetry { peer_id };
        assert_eq!(
            retry.is_enabled(&state, now.get()),
            delay.is_zero(),
            "{kind:?}"
        );
        now.set(now.get() + delay);
        // No incoming RPC is needed to wake the retry after the delay.
        let mut wakeup = Dispatcher::new();
        crate::reducer(
            &mut state,
            &ActionMeta::zero_custom(now.get()).with_action(crate::CheckTimeoutsAction {}.into()),
            &mut wakeup,
        );
        use crate::ActionKindGet;
        let mut scheduled = false;
        while let Some(action) = wakeup.pop() {
            scheduled |=
                action.kind() == crate::ActionKind::TransitionFrontierSyncLedgerSnarkedPeersQuery;
        }
        assert!(scheduled);
        assert!(retry.is_enabled(&state, now.get()), "{kind:?}");
        reduce(&mut state, retry);
        reduce(
            &mut state,
            SnarkedAction::PeerQueryNumAccountsPending { peer_id, rpc_id: 1 },
        );
        reduce(
            &mut state,
            SnarkedAction::PeerQueryNumAccountsSuccess {
                peer_id,
                rpc_id: 1,
                response: PeerLedgerQueryResponse::NumAccounts(1, contents_hash.clone()),
            },
        );
        reduce(
            &mut state,
            SnarkedAction::NumAccountsReceived {
                num_accounts: 1,
                contents_hash: contents_hash.clone(),
                sender: peer_id,
            },
        );
        reduce(
            &mut state,
            SnarkedAction::NumAccountsAccepted {
                num_accounts: 1,
                contents_hash: contents_hash.clone(),
                sender: peer_id,
            },
        );
        reduce(
            &mut state,
            SnarkedAction::NumAccountsSuccess {
                num_accounts: 1,
                contents_hash: contents_hash.clone(),
            },
        );
        reduce(&mut state, SnarkedAction::MerkleTreeSyncPending);
        reduce(&mut state, SnarkedAction::PeersQuery);
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
        let address = address.clone();
        let init = SnarkedAction::PeerQueryAddressInit {
            address: address.clone(),
            expected_hash: expected_hash.clone(),
            peer_id,
        };
        assert!(init.is_enabled(&state, now.get()), "{kind:?}");
        reduce(&mut state, init);
        reduce(
            &mut state,
            SnarkedAction::PeerQueryAddressPending {
                address: address.clone(),
                peer_id,
                rpc_id: 2,
            },
        );
        reduce(
            &mut state,
            SnarkedAction::PeerQueryAddressError {
                peer_id,
                rpc_id: 2,
                error: error.clone(),
            },
        );
        reduce(&mut state, SnarkedAction::PeersQuery);
        assert!(!state
            .transition_frontier
            .sync
            .ledger()
            .unwrap()
            .snarked()
            .unwrap()
            .peer_unavailable(&peer_id));
        let retry = SnarkedAction::PeerQueryAddressRetry {
            address: address.clone(),
            peer_id,
        };
        assert_eq!(
            retry.is_enabled(&state, now.get()),
            delay.is_zero(),
            "{kind:?}"
        );
        now.set(now.get() + delay);
        assert!(retry.is_enabled(&state, now.get()), "{kind:?}");
        reduce(&mut state, retry);
        reduce(
            &mut state,
            SnarkedAction::PeerQueryAddressPending {
                address: address.clone(),
                peer_id,
                rpc_id: 3,
            },
        );
        reduce(
            &mut state,
            SnarkedAction::PeerQueryAddressSuccess {
                peer_id,
                rpc_id: 3,
                response: PeerLedgerQueryResponse::ChildAccounts(vec![]),
            },
        );
        reduce(
            &mut state,
            SnarkedAction::ChildAccountsAccepted {
                address,
                count: 1,
                sender: peer_id,
            },
        );
        reduce(&mut state, SnarkedAction::MerkleTreeSyncSuccess);
        reduce(&mut state, SnarkedAction::Success);
        assert!(
            matches!(
                state
                    .transition_frontier
                    .sync
                    .ledger()
                    .unwrap()
                    .snarked()
                    .unwrap(),
                SnarkedState::Success { .. }
            ),
            "{kind:?}"
        );
    }
}

/// R14: a best tip that changes the root's snarked ledger during a bootstrap
/// starts the new root ledger sync from the previous target's ledger, then
/// the epoch ledgers. The frontier root, the genesis block, is left out: its
/// ledger is the fork's, or absent.
#[test]
fn a_retarget_during_a_bootstrap_offers_the_previous_target_first() {
    let template = template_block();
    let genesis = in_epoch(block(&template, 1, None, GENESIS_LEDGER));
    let new_root = in_epoch(block(&template, 3, None, 8));
    let tip = in_epoch(block(&template, 10, None, 8));
    let previous_target = Some(ledger_hash(7));
    let expected = [7, NEXT_EPOCH_LEDGER, STAKING_LEDGER].map(ledger_hash);

    assert_eq!(
        root_ledger_copy_candidates(previous_target.clone(), Some(&genesis), &tip, &new_root),
        expected
    );
    assert_eq!(
        root_ledger_copy_candidates(previous_target, None, &tip, &new_root),
        expected
    );
}

/// A synced node that has to sync a new root ledger while catching up: the
/// frontier root's ledger, which it has committed, comes right after the
/// previous sync target's, which it may not have.
#[test]
fn a_synced_node_offers_its_frontier_root_next() {
    let template = template_block();
    let frontier_root = in_epoch(block(&template, 2, None, 9));
    let new_root = in_epoch(block(&template, 3, None, 8));
    let tip = in_epoch(block(&template, 10, None, 8));

    assert_eq!(
        root_ledger_copy_candidates(Some(ledger_hash(6)), Some(&frontier_root), &tip, &new_root),
        [6, 9, NEXT_EPOCH_LEDGER, STAKING_LEDGER].map(ledger_hash)
    );
    // Synced, the previous root is the frontier root: offered once.
    assert_eq!(
        root_ledger_copy_candidates(Some(ledger_hash(9)), Some(&frontier_root), &tip, &new_root),
        [9, NEXT_EPOCH_LEDGER, STAKING_LEDGER].map(ledger_hash)
    );
}

/// A node syncing `kind`'s ledger at `root`, with ready peers `sender` and
/// `serving`, whose only pending address `address` was answered by `sender`.
fn pending_address_sync_state(
    kind: super::ledger::SyncLedgerTargetKind,
    root: &ArcBlockWithHash,
    address: &crate::ledger::LedgerAddress,
    sender: crate::p2p::PeerId,
    serving: crate::p2p::PeerId,
) -> crate::State {
    use super::ledger::{
        snarked::{LedgerAddressQueryPending, PeerRpcState},
        SyncLedgerTargetKind,
    };
    use crate::p2p::{
        channels::rpc::{P2pChannelsRpcState, P2pRpcLocalState, P2pRpcRemoteState},
        P2pPeerState, P2pPeerStatus, P2pPeerStatusReady,
    };

    let chain_id = serde_json::from_str(
        r#""0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886""#,
    )
    .unwrap();
    let mut state = sync_test_node(vec![AppliedBlock {
        block: root.clone(),
        just_emitted_a_proof: false,
    }]);
    state.p2p.initialize(&chain_id).unwrap();
    for id in [sender, serving] {
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
            id,
            P2pPeerState {
                is_libp2p: false,
                dial_opts: None,
                identify: None,
                status: P2pPeerStatus::Ready(Box::new(ready)),
            },
        );
    }
    let ledger = TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncPending {
        time: Timestamp::ZERO,
        target: SyncLedgerTarget {
            kind,
            snarked_ledger_hash: root.snarked_ledger_hash().clone(),
            staged: None,
        },
        total_accounts_expected: 1,
        synced_accounts_count: 0,
        synced_hashes_count: 0,
        queue: Default::default(),
        unavailable_peers: Default::default(),
        peer_query_errors: Default::default(),
        pending_addresses: [(
            address.clone(),
            LedgerAddressQueryPending {
                time: Timestamp::ZERO,
                expected_hash: root.snarked_ledger_hash().clone(),
                attempts: [(
                    sender,
                    PeerRpcState::Success {
                        time: Timestamp::ZERO,
                        rpc_id: 1,
                    },
                )]
                .into(),
            },
        )]
        .into(),
    };
    let pending = TransitionFrontierSyncLedgerPending {
        time: Timestamp::ZERO,
        best_tip: root.clone(),
        root_block: root.clone(),
        blocks_inbetween: vec![],
        root_block_updates: vec![],
        ledger: ledger.into(),
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
    state
}

/// Accounts the ledger service can't take (`accounts_set` fails, e.g. a field
/// element out of range) are a rejected answer, retried from another peer.
#[test]
fn ledger_accounts_the_service_cannot_set_are_rejected() {
    use super::ledger::{
        snarked::{
            TransitionFrontierSyncLedgerSnarkedAction as SnarkedAction,
            TransitionFrontierSyncLedgerSnarkedService,
        },
        SyncLedgerTargetKind,
    };
    use crate::{ledger::LedgerAddress, p2p::PeerId};
    use redux::ActionMeta;

    struct FailingService(redux::Instant);
    impl redux::Service for FailingService {}
    impl redux::TimeService for FailingService {
        fn monotonic_time(&mut self) -> redux::Instant {
            self.0
        }
    }
    impl TransitionFrontierSyncLedgerSnarkedService for FailingService {
        fn contains_snarked_ledger(&self, _: &LedgerHash) -> bool {
            unreachable!()
        }
        fn copy_snarked_ledger_contents_for_sync(
            &self,
            _: Vec<LedgerHash>,
            _: LedgerHash,
            _: bool,
        ) -> Result<bool, String> {
            unreachable!()
        }
        fn compute_snarked_ledger_hashes(&self, _: &LedgerHash) -> Result<(), String> {
            unreachable!()
        }
        fn child_hashes_get(
            &self,
            _: LedgerHash,
            _: &LedgerAddress,
            _: usize,
        ) -> Result<Vec<LedgerHash>, String> {
            unreachable!()
        }
        fn accounts_set(
            &self,
            _: LedgerHash,
            _: &LedgerAddress,
            _: &LedgerHash,
            _: Vec<mina_p2p_messages::v2::MinaBaseAccountBinableArgStableV2>,
        ) -> Result<LedgerHash, String> {
            Err("InvalidBigInt".to_owned())
        }
    }

    let template = template_block();
    let root = block(&template, 100, None, 7);
    let sender = PeerId::from_bytes([71; 32]);
    let serving = PeerId::from_bytes([72; 32]);
    let address = LedgerAddress::first(29);
    let state =
        pending_address_sync_state(SyncLedgerTargetKind::Root, &root, &address, sender, serving);
    let mut store = redux::Store::new(
        crate::reducer,
        |_, _| {},
        FailingService(redux::Instant::now()),
        redux::SystemTime::UNIX_EPOCH,
        state,
    );
    SnarkedAction::ChildAccountsReceived {
        address: address.clone(),
        accounts: vec![],
        sender,
    }
    .effects(&ActionMeta::zero_custom(Timestamp::ZERO), &mut store);
    let ledger = store
        .state()
        .transition_frontier
        .sync
        .ledger()
        .unwrap()
        .snarked()
        .unwrap();
    // The store's clock is the process's, so check the outcome, not when the
    // retry is due: the answer was rejected, and the address awaits another.
    let pending = ledger.fetch_pending().unwrap().get(&address).unwrap();
    assert!(pending
        .attempts
        .get(&sender)
        .is_none_or(|attempt| !attempt.is_success()));
}

/// A received RPC is not a completed address until its Merkle proof passes.
/// SY1's live stall left rejected sibling answers in Success forever.
#[test]
fn rejected_ledger_answers_remain_retryable() {
    use super::ledger::{
        snarked::TransitionFrontierSyncLedgerSnarkedAction as SnarkedAction, SyncLedgerTargetKind,
    };
    use crate::{ledger::LedgerAddress, p2p::PeerId};
    use redux::{ActionMeta, Dispatcher, EnablingCondition};

    let template = template_block();
    let root = block(&template, 100, None, 7);
    let sender = PeerId::from_bytes([71; 32]);
    let serving = PeerId::from_bytes([72; 32]);
    for kind in [
        SyncLedgerTargetKind::StakingEpoch,
        SyncLedgerTargetKind::NextEpoch,
        SyncLedgerTargetKind::Root,
    ] {
        for accounts in [false, true] {
            let address = LedgerAddress::first(if accounts { 29 } else { 25 });
            let mut state = pending_address_sync_state(kind, &root, &address, sender, serving);
            let action = if accounts {
                SnarkedAction::ChildAccountsRejected {
                    address: address.clone(),
                    sender,
                }
            } else {
                SnarkedAction::ChildHashesRejected {
                    address: address.clone(),
                    hashes: vec![],
                    sender,
                }
            };
            assert!(action.is_enabled(&state, Timestamp::ZERO));
            crate::reducer(
                &mut state,
                &ActionMeta::zero_custom(Timestamp::ZERO).with_action(action.into()),
                &mut Dispatcher::new(),
            );
            let ledger = state
                .transition_frontier
                .sync
                .ledger()
                .unwrap()
                .snarked()
                .unwrap();
            assert_eq!(
                ledger
                    .sync_address_retry_iter(Timestamp::ZERO)
                    .collect::<Vec<_>>(),
                vec![address.clone()],
                "{kind:?}, accounts={accounts}"
            );
            assert!(
                !ledger.peer_unavailable(&sender),
                "invalid data isn't an unavailable-ledger exclusion"
            );
            let retry = |peer_id| SnarkedAction::PeerQueryAddressRetry {
                address: address.clone(),
                peer_id,
            };
            assert!(!retry(sender).is_enabled(&state, Timestamp::ZERO));
            assert!(retry(serving).is_enabled(&state, Timestamp::ZERO));
            assert!(retry(sender)
                .is_enabled(&state, Timestamp::ZERO + std::time::Duration::from_secs(5)));
            // Same-ledger retargeting must preserve the retry, expected hash,
            // and peer delay while the best tip moves.
            let target = ledger.target().clone();
            state
                .transition_frontier
                .sync
                .ledger_mut()
                .unwrap()
                .update_target(Timestamp::ZERO, target);
            assert!(retry(serving).is_enabled(&state, Timestamp::ZERO));
            let mut reduce = |action: SnarkedAction| {
                crate::reducer(
                    &mut state,
                    &ActionMeta::zero_custom(Timestamp::ZERO).with_action(action.into()),
                    &mut Dispatcher::new(),
                )
            };
            reduce(retry(serving));
            reduce(SnarkedAction::PeerQueryAddressPending {
                address: address.clone(),
                peer_id: serving,
                rpc_id: 2,
            });
            // Synthetic accepted reducer inputs test scheduling/completion;
            // the Merkle answer validation is exercised by live bootstraps.
            reduce(SnarkedAction::PeerQueryAddressSuccess {
                peer_id: serving,
                rpc_id: 2,
                response: if accounts {
                    super::ledger::snarked::PeerLedgerQueryResponse::ChildAccounts(vec![])
                } else {
                    super::ledger::snarked::PeerLedgerQueryResponse::ChildHashes(vec![])
                },
            });
            if accounts {
                reduce(SnarkedAction::ChildAccountsAccepted {
                    address,
                    count: 1,
                    sender: serving,
                });
            } else {
                let hashes = vec![root.snarked_ledger_hash().clone(); 2];
                reduce(SnarkedAction::ChildHashesAccepted {
                    address,
                    hashes: hashes.clone(),
                    previous_hashes: hashes,
                    sender: serving,
                });
            }
            reduce(SnarkedAction::MerkleTreeSyncSuccess);
            assert!(matches!(
                state.transition_frontier.sync.ledger().unwrap().snarked(),
                Some(TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncSuccess { .. })
            ));
        }
    }
}

/// The Raspberry Pi 3 stall (2026-10-25): a staking ledger sync with every
/// address fetched but one, whose only attempt never got an outcome, waited
/// 45 minutes with nothing to do. An attempt left `Init` (request never sent),
/// `Pending` (answer consumed without a callback) or `Success` (follow-up
/// disabled) wasn't retryable, so no peer was asked again and the ledger never
/// reached `MerkleTreeSyncSuccess`. Once stale it is retried: the periodic
/// `PeersQuery` asks a peer again, and the ledger completes.
#[test]
fn a_ledger_query_without_an_outcome_is_retried_once_stale() {
    use super::{
        ledger::snarked::{
            LedgerAddressQueryPending, PeerRpcState,
            TransitionFrontierSyncLedgerSnarkedAction as SnarkedAction, LEDGER_QUERY_STALE_AFTER,
        },
        TransitionFrontierSyncAction,
    };
    use crate::{
        ledger::LedgerAddress,
        p2p::{
            channels::rpc::{P2pChannelsRpcState, P2pRpcLocalState, P2pRpcRemoteState},
            P2pPeerState, P2pPeerStatus, P2pPeerStatusReady, PeerId,
        },
    };
    use redux::{ActionMeta, Dispatcher, EnablingCondition};

    let root = block(&template_block(), 100, None, 7);
    let lost = PeerId::from_bytes([91; 32]);
    let healthy = PeerId::from_bytes([92; 32]);
    let chain_id = serde_json::from_str(
        r#""0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886""#,
    )
    .unwrap();
    let start = Timestamp::ZERO + std::time::Duration::from_secs(1000);
    let soon = start + std::time::Duration::from_secs(10);
    let stale = start + LEDGER_QUERY_STALE_AFTER;
    let attempts = [
        PeerRpcState::Init { time: start },
        PeerRpcState::Pending {
            time: start,
            rpc_id: 1,
        },
        PeerRpcState::Success {
            time: start,
            rpc_id: 1,
        },
    ];
    for (attempt, accounts) in attempts
        .into_iter()
        .flat_map(|a| [(a.clone(), false), (a, true)])
    {
        let mut state = sync_test_node(vec![AppliedBlock {
            block: root.clone(),
            just_emitted_a_proof: false,
        }]);
        state.p2p.initialize(&chain_id).unwrap();
        for id in [lost, healthy] {
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
                id,
                P2pPeerState {
                    is_libp2p: false,
                    dial_opts: None,
                    identify: None,
                    status: P2pPeerStatus::Ready(Box::new(ready)),
                },
            );
        }
        // Every other address is done: the queue is empty, one is pending.
        let address = LedgerAddress::first(if accounts { 29 } else { 25 });
        let ledger = TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncPending {
            time: start,
            target: SyncLedgerTarget {
                kind: super::ledger::SyncLedgerTargetKind::StakingEpoch,
                snarked_ledger_hash: root.snarked_ledger_hash().clone(),
                staged: None,
            },
            total_accounts_expected: 1,
            synced_accounts_count: 0,
            synced_hashes_count: 0,
            queue: Default::default(),
            unavailable_peers: Default::default(),
            peer_query_errors: Default::default(),
            pending_addresses: [(
                address.clone(),
                LedgerAddressQueryPending {
                    time: start,
                    expected_hash: root.snarked_ledger_hash().clone(),
                    attempts: [(lost, attempt.clone())].into(),
                },
            )]
            .into(),
        };
        state.transition_frontier.sync = TransitionFrontierSyncState::StakingLedgerPending(
            TransitionFrontierSyncLedgerPending {
                time: start,
                best_tip: root.clone(),
                root_block: root.clone(),
                blocks_inbetween: vec![],
                root_block_updates: vec![],
                ledger: ledger.into(),
            },
        );
        let case = format!("{attempt:?}, accounts={accounts}");
        let snarked = |state: &crate::State| {
            state
                .transition_frontier
                .sync
                .ledger()
                .unwrap()
                .snarked()
                .unwrap()
                .clone()
        };
        let retry = |peer_id| SnarkedAction::PeerQueryAddressRetry {
            address: address.clone(),
            peer_id,
        };
        // What the periodic `PeersQuery` (every `CheckTimeouts`) asks for.
        let peers_query = |state: &mut crate::State, time| {
            let mut queued = Dispatcher::new();
            crate::reducer(
                state,
                &ActionMeta::zero_custom(time).with_action(SnarkedAction::PeersQuery.into()),
                &mut queued,
            );
            let mut asked = vec![];
            while let Some(action) = queued.pop() {
                if let crate::Action::TransitionFrontier(
                    crate::transition_frontier::TransitionFrontierAction::Sync(
                        TransitionFrontierSyncAction::Ledger(
                            super::ledger::TransitionFrontierSyncLedgerAction::Snarked(
                                SnarkedAction::PeerQueryAddressRetry { peer_id, .. },
                            ),
                        ),
                    ),
                ) = action
                {
                    asked.push(peer_id);
                }
            }
            asked
        };

        // Within the stale bound the attempt still counts: no retry.
        assert!(
            snarked(&state)
                .sync_address_retry_iter(soon)
                .next()
                .is_none(),
            "{case}"
        );
        assert!(!retry(healthy).is_enabled(&state, soon), "{case}");
        assert!(peers_query(&mut state, soon).is_empty(), "{case}");
        assert!(!SnarkedAction::MerkleTreeSyncSuccess.is_enabled(&state, soon));

        // Once stale the address is asked again, of either peer.
        assert_eq!(
            snarked(&state)
                .sync_address_retry_iter(stale)
                .collect::<Vec<_>>(),
            vec![address.clone()],
            "{case}"
        );
        assert!(retry(healthy).is_enabled(&state, stale), "{case}");
        let asked = peers_query(&mut state, stale);
        assert_eq!(asked.len(), 1, "{case}: {asked:?}");
        let peer = asked[0];

        let mut reduce = |action: SnarkedAction| {
            assert!(action.is_enabled(&state, stale), "{case}: {action:?}");
            crate::reducer(
                &mut state,
                &ActionMeta::zero_custom(stale).with_action(action.into()),
                &mut Dispatcher::new(),
            )
        };
        reduce(retry(peer));
        reduce(SnarkedAction::PeerQueryAddressPending {
            address: address.clone(),
            peer_id: peer,
            rpc_id: 2,
        });
        reduce(SnarkedAction::PeerQueryAddressSuccess {
            peer_id: peer,
            rpc_id: 2,
            response: if accounts {
                super::ledger::snarked::PeerLedgerQueryResponse::ChildAccounts(vec![])
            } else {
                super::ledger::snarked::PeerLedgerQueryResponse::ChildHashes(vec![])
            },
        });
        if accounts {
            // An empty range whose hash checked is accepted too: before, a
            // count of 0 left the attempt `Success` for good.
            reduce(SnarkedAction::ChildAccountsAccepted {
                address: address.clone(),
                count: 0,
                sender: peer,
            });
        } else {
            let hashes = vec![root.snarked_ledger_hash().clone(); 2];
            reduce(SnarkedAction::ChildHashesAccepted {
                address: address.clone(),
                hashes: hashes.clone(),
                previous_hashes: hashes,
                sender: peer,
            });
        }
        reduce(SnarkedAction::MerkleTreeSyncSuccess);
        reduce(SnarkedAction::Success);
        assert!(
            matches!(
                state.transition_frontier.sync.ledger().unwrap().snarked(),
                Some(TransitionFrontierSyncLedgerSnarkedState::Success { .. })
            ),
            "{case}"
        );
    }
}

#[test]
fn ledger_callback_rejects_wrong_num_accounts_answer_variants() {
    ledger_callback_checks_query_kind_and_rpc_id(0);
}

#[test]
fn ledger_callback_rejects_wrong_child_hashes_answer_variants() {
    ledger_callback_checks_query_kind_and_rpc_id(1);
}

#[test]
fn ledger_callback_rejects_wrong_child_accounts_answer_variants() {
    ledger_callback_checks_query_kind_and_rpc_id(2);
}

/// Drive the real transport reducer and node callback. The transport has
/// already answered, so a wrong constructor must fail the original query;
/// no transport timeout can repair it. Valid cases test routing, not hashing.
fn ledger_callback_checks_query_kind_and_rpc_id(query_kind: usize) {
    use super::{
        ledger::snarked::{
            LedgerAddressQueryPending, LedgerNumAccountsQueryPending, PeerLedgerQueryError,
            PeerRpcState, TransitionFrontierSyncLedgerSnarkedAction as SnarkedAction,
            TransitionFrontierSyncLedgerSnarkedState as SnarkedState,
        },
        TransitionFrontierSyncAction,
    };
    use crate::{
        ledger::LedgerAddress,
        p2p::{
            callbacks::P2pCallbacksAction,
            channels::rpc::{
                P2pChannelsRpcAction, P2pChannelsRpcState, P2pRpcLocalState, P2pRpcRemoteState,
                P2pRpcRequest, P2pRpcResponse,
            },
            P2pPeerState, P2pPeerStatus, P2pPeerStatusReady, PeerId,
        },
    };
    use mina_p2p_messages::v2::{
        MinaLedgerSyncLedgerAnswerStableV2 as Answer, MinaLedgerSyncLedgerQueryStableV1 as Query,
    };
    use redux::{ActionMeta, Dispatcher, EnablingCondition};

    let root = block(&template_block(), 100, None, 7);
    let peer_id = PeerId::from_bytes([81; 32]);
    let healthy = PeerId::from_bytes([82; 32]);
    let rpc_id = 7;
    let address = LedgerAddress::first(if query_kind == 2 { 29 } else { 25 });
    let query = match query_kind {
        0 => Query::NumAccounts,
        1 => Query::WhatChildHashes(address.clone().into(), 1u64.into()),
        _ => Query::WhatContents(address.clone().into()),
    };
    let hash = root.snarked_ledger_hash().clone();
    let request = P2pRpcRequest::LedgerQuery(hash.clone(), query);
    let chain_id = serde_json::from_str(
        r#""0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886""#,
    )
    .unwrap();
    for answer_kind in 0..3 {
        let mut state = sync_test_node(vec![AppliedBlock {
            block: root.clone(),
            just_emitted_a_proof: false,
        }]);
        state.p2p.initialize(&chain_id).unwrap();
        for id in [peer_id, healthy] {
            let mut ready = P2pPeerStatusReady::new(false, Timestamp::ZERO, &Default::default());
            ready.best_tip = Some(root.clone());
            ready.channels.rpc = P2pChannelsRpcState::Ready {
                time: Timestamp::ZERO,
                local: if id == peer_id {
                    P2pRpcLocalState::Requested {
                        time: Timestamp::ZERO,
                        id: rpc_id,
                        request: Box::new(request.clone()),
                    }
                } else {
                    P2pRpcLocalState::WaitingForRequest {
                        time: Timestamp::ZERO,
                    }
                },
                remote: P2pRpcRemoteState {
                    pending_requests: Default::default(),
                    last_responded: Timestamp::ZERO,
                },
            };
            state.p2p.ready_mut().unwrap().peers.insert(
                id,
                P2pPeerState {
                    is_libp2p: false,
                    dial_opts: None,
                    identify: None,
                    status: P2pPeerStatus::Ready(Box::new(ready)),
                },
            );
        }
        let attempt = PeerRpcState::Pending {
            time: Timestamp::ZERO,
            rpc_id,
        };
        let target = SyncLedgerTarget {
            kind: super::ledger::SyncLedgerTargetKind::StakingEpoch,
            snarked_ledger_hash: hash.clone(),
            staged: None,
        };
        let ledger = if query_kind == 0 {
            SnarkedState::NumAccountsPending {
                time: Timestamp::ZERO,
                target,
                pending_num_accounts: LedgerNumAccountsQueryPending {
                    attempts: [(peer_id, attempt)].into(),
                },
            }
        } else {
            SnarkedState::MerkleTreeSyncPending {
                time: Timestamp::ZERO,
                target,
                total_accounts_expected: 1,
                synced_accounts_count: 0,
                synced_hashes_count: 0,
                queue: Default::default(),
                unavailable_peers: Default::default(),
                peer_query_errors: Default::default(),
                pending_addresses: [(
                    address.clone(),
                    LedgerAddressQueryPending {
                        time: Timestamp::ZERO,
                        expected_hash: hash.clone(),
                        attempts: [(peer_id, attempt)].into(),
                    },
                )]
                .into(),
            }
        };
        state.transition_frontier.sync = TransitionFrontierSyncState::StakingLedgerPending(
            TransitionFrontierSyncLedgerPending {
                time: Timestamp::ZERO,
                best_tip: root.clone(),
                root_block: root.clone(),
                blocks_inbetween: vec![],
                root_block_updates: vec![],
                ledger: ledger.into(),
            },
        );
        let answer = match answer_kind {
            0 => Answer::NumAccounts(1u64.into(), hash.clone()),
            1 => Answer::ChildHashesAre(vec![hash.clone(); 2].into_iter().collect()),
            _ => Answer::ContentsAre(Vec::new().into_iter().collect()),
        };
        let response = Some(Box::new(P2pRpcResponse::LedgerQuery(answer)));
        let transport = P2pChannelsRpcAction::ResponseReceived {
            peer_id,
            id: rpc_id,
            response: response.clone(),
        };
        assert!(transport.is_enabled(&state, Timestamp::ZERO));
        crate::reducer(
            &mut state,
            &ActionMeta::zero_custom(Timestamp::ZERO).with_action(transport.into()),
            &mut Dispatcher::new(),
        );
        assert!(state
            .p2p
            .get_ready_peer(&peer_id)
            .unwrap()
            .channels
            .rpc
            .pending_local_rpc_id()
            .is_none());
        assert!(state
            .p2p
            .get_ready_peer(&peer_id)
            .unwrap()
            .channels
            .rpc
            .can_send_request());
        let callback = |id| P2pCallbacksAction::P2pChannelsRpcResponseReceived {
            peer_id,
            id,
            response: response.clone(),
        };
        let mut routed = Dispatcher::new();
        crate::reducer(
            &mut state,
            &ActionMeta::zero_custom(Timestamp::ZERO).with_action(callback(rpc_id).into()),
            &mut routed,
        );
        let mut handled = false;
        while let Some(action) = routed.pop() {
            let crate::Action::TransitionFrontier(
                crate::transition_frontier::TransitionFrontierAction::Sync(
                    TransitionFrontierSyncAction::Ledger(
                        super::ledger::TransitionFrontierSyncLedgerAction::Snarked(ref a),
                    ),
                ),
            ) = action
            else {
                continue;
            };
            if !matches!(
                a,
                SnarkedAction::PeerQueryNumAccountsError { .. }
                    | SnarkedAction::PeerQueryAddressError { .. }
                    | SnarkedAction::PeerQueryNumAccountsSuccess { .. }
                    | SnarkedAction::PeerQueryAddressSuccess { .. }
            ) {
                continue;
            }
            assert!(!handled, "one outcome per query");
            handled = true;
            if query_kind != answer_kind {
                assert!(
                    matches!(a,
                    SnarkedAction::PeerQueryNumAccountsError { error: PeerLedgerQueryError::InvalidResponse, rpc_id: id, .. }
                    | SnarkedAction::PeerQueryAddressError { error: PeerLedgerQueryError::InvalidResponse, rpc_id: id, .. }
                    if *id == rpc_id),
                    "query={query_kind}, answer={answer_kind}: {a:?}"
                );
            }
            assert!(a.is_enabled(&state, Timestamp::ZERO));
            crate::reducer(
                &mut state,
                &ActionMeta::zero_custom(Timestamp::ZERO).with_action(action),
                &mut Dispatcher::new(),
            );
        }
        assert!(handled);
        let ledger = state
            .transition_frontier
            .sync
            .ledger()
            .unwrap()
            .snarked()
            .unwrap();
        let attempt = if query_kind == 0 {
            ledger
                .num_accounts_pending()
                .unwrap()
                .attempts
                .get(&peer_id)
                .unwrap()
        } else {
            ledger
                .fetch_pending()
                .unwrap()
                .get(&address)
                .unwrap()
                .attempts
                .get(&peer_id)
                .unwrap()
        };
        if query_kind == answer_kind {
            assert!(attempt.is_success());
        } else {
            assert!(matches!(
                attempt,
                PeerRpcState::Error {
                    error: PeerLedgerQueryError::InvalidResponse,
                    ..
                }
            ));
            assert!(!ledger.peer_unavailable(&peer_id));
            let retry = if query_kind == 0 {
                SnarkedAction::PeerQueryNumAccountsInit { peer_id: healthy }
            } else {
                SnarkedAction::PeerQueryAddressRetry {
                    address: address.clone(),
                    peer_id: healthy,
                }
            };
            assert!(
                retry.is_enabled(&state, Timestamp::ZERO),
                "healthy peer can immediately recover"
            );
        }
        // An old callback must never use the request recorded for another id.
        let mut stale = Dispatcher::new();
        crate::reducer(
            &mut state,
            &ActionMeta::zero_custom(Timestamp::ZERO).with_action(callback(rpc_id + 1).into()),
            &mut stale,
        );
        while let Some(action) = stale.pop() {
            use crate::ActionKindGet;
            assert!(!matches!(action.kind(), crate::ActionKind::TransitionFrontierSyncLedgerSnarkedPeerQueryNumAccountsSuccess
                | crate::ActionKind::TransitionFrontierSyncLedgerSnarkedPeerQueryAddressSuccess
                | crate::ActionKind::TransitionFrontierSyncLedgerSnarkedPeerQueryNumAccountsError
                | crate::ActionKind::TransitionFrontierSyncLedgerSnarkedPeerQueryAddressError));
        }
    }
}
