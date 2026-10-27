//! P6e: every transaction submission is answered exactly once, and none is
//! judged before the node has a synced best tip.
//!
//! These run the node's reducers on a real `State`, without effects or
//! services: a test plays the services' part by turning the effectful
//! actions it cares about into the actions their services would dispatch.

use std::{collections::BTreeMap, sync::Arc};

use mina_core::{
    block::{AppliedBlock, ArcBlockWithHash, BlockWithHash},
    consensus::ConsensusConstants,
    constants::constraint_constants,
    network::NetworkConfig,
    transaction::TransactionPoolMessageSource,
};
use mina_p2p_messages::{
    binprot::BinProtRead,
    rpc::GetBestTipV2Response,
    rpc_kernel::ResponsePayload,
    v2::{MinaBaseUserCommandStableV2, MinaBlockBlockStableV2},
};
use mina_snark::{
    user_command_verify::SnarkUserCommandVerifyAction,
    user_command_verify_effectful::{
        SnarkUserCommandVerifyEffectfulAction, SnarkUserCommandVerifyError,
    },
    BlockVerifier, SnarkAction, SnarkConfig, TransactionVerifier,
};
use redux::{ActionMeta, EnablingCondition, Timestamp};

use crate::{
    config::GlobalConfig,
    p2p::{
        channels::ChannelId, identity::SecretKey, P2pConfig, P2pLimits, P2pMeshsubConfig,
        P2pTimeouts,
    },
    rpc::{RpcId, TRANSACTION_INJECT_BOOTSTRAPPING},
    rpc_effectful::RpcEffectfulAction,
    transaction_pool::TransactionPoolEffectfulAction,
    transition_frontier::{
        genesis::TransitionFrontierGenesisState, sync::TransitionFrontierSyncState,
    },
    Action, BuildEnv, Config, LedgerConfig, RpcAction, State, TransactionPoolAction,
    TransitionFrontierConfig,
};

/// Mesa mainnet block 564003, captured from an OCaml 4.0.2 node.
pub(crate) fn fixture_block() -> MinaBlockBlockStableV2 {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../p2p-messages/tests/files/mesa/get_best_tip_v3.bin"
    );
    let bytes = std::fs::read(path).unwrap();
    let payload = ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut &bytes[..]).unwrap();
    payload.0.unwrap().0.unwrap().data
}

/// A signed payment from the fixture block.
fn payment() -> MinaBaseUserCommandStableV2 {
    let diff = fixture_block().body.staged_ledger_diff.diff.clone();
    diff.0
        .commands
        .into_iter()
        .chain(diff.1.into_iter().flat_map(|d| d.commands))
        .map(|c| c.data)
        .find(|c| matches!(c, MinaBaseUserCommandStableV2::SignedCommand(_)))
        .expect("a signed command")
}

pub(crate) fn block() -> ArcBlockWithHash {
    BlockWithHash::try_new(Arc::new(fixture_block())).unwrap()
}

/// A node just started: no genesis block, no best tip.
pub(crate) fn fresh_state() -> State {
    let _ = NetworkConfig::init("mainnet");
    let genesis = crate::config::DEVNET_CONFIG.clone();
    let protocol_constants = genesis.protocol_constants().unwrap();
    let consensus_constants =
        ConsensusConstants::create(constraint_constants(), &protocol_constants);
    let srs = mina_snark::get_srs();
    let config = Config {
        ledger: LedgerConfig {},
        snark: SnarkConfig {
            block_verifier_index: BlockVerifier::make(),
            block_verifier_srs: srs.clone(),
            work_verifier_index: TransactionVerifier::make(),
            work_verifier_srs: srs,
        },
        p2p: P2pConfig {
            libp2p_port: None,
            listen_port: None,
            identity_pub_key: SecretKey::rand().public_key(),
            initial_peers: vec![],
            external_addrs: vec![],
            enabled_channels: ChannelId::iter_all().collect(),
            timeouts: P2pTimeouts::default(),
            limits: P2pLimits::default(),
            peer_discovery: false,
            meshsub: P2pMeshsubConfig::default(),
            catchup_rpc: Default::default(),
        },
        transition_frontier: TransitionFrontierConfig::new(genesis),
        archive: None,
        block_producer: None,
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
    State::new(config, &consensus_constants, now())
}

/// Just after the fixture block, which the tests use as the best tip.
pub(crate) fn now() -> Timestamp {
    block().timestamp() + std::time::Duration::from_secs(60)
}

/// A node following mainnet: the fixture block is its genesis (for the
/// pool's slot arithmetic), its best tip and its pool's best tip.
pub(crate) fn synced_state() -> State {
    let mut state = fresh_state();
    let block = block();
    state.transition_frontier.genesis = TransitionFrontierGenesisState::ProveSuccess {
        time: now(),
        genesis: block.clone(),
    };
    state.transition_frontier.best_chain = vec![AppliedBlock {
        block: block.clone(),
        just_emitted_a_proof: false,
    }];
    state.transition_frontier.sync = TransitionFrontierSyncState::Synced { time: now() };
    state.transaction_pool.best_tip_hash = Some(block.merkle_root_hash().clone());
    assert!(!block.is_genesis());
    state
}

/// Runs `action` and everything its reducers dispatch, depth first and with
/// enabling conditions, as the store does, but runs no effects. Returns every
/// action that ran.
pub(crate) fn run(state: &mut State, action: impl Into<Action>) -> Vec<Action> {
    let mut ran = Vec::new();
    dispatch(state, action.into(), &mut ran);
    ran
}

fn dispatch(state: &mut State, action: Action, ran: &mut Vec<Action>) {
    let time = now();
    if !action.is_enabled(state, time) {
        return;
    }
    ran.push(action.clone());
    let action = ActionMeta::zero_custom(time).with_action(action);
    let mut dispatcher = redux::Dispatcher::new();
    crate::reducer(state, &action, &mut dispatcher);
    while let Some(next) = dispatcher.pop() {
        dispatch(state, next, ran);
    }
}

fn our_rpc_id() -> RpcId {
    RpcId::new_unchecked(7, 1)
}

fn inject(commands: Vec<MinaBaseUserCommandStableV2>) -> RpcAction {
    RpcAction::TransactionInjectInit {
        rpc_id: our_rpc_id(),
        commands,
    }
}

/// The answers the RPC service would send for our request.
fn answers(ran: &[Action]) -> Vec<String> {
    ran.iter()
        .filter_map(|action| match action {
            Action::RpcEffectful(RpcEffectfulAction::TransactionInjectSuccess {
                rpc_id, ..
            }) if *rpc_id == our_rpc_id() => Some("success".to_owned()),
            Action::RpcEffectful(RpcEffectfulAction::TransactionInjectRejected {
                rpc_id,
                response,
            }) if *rpc_id == our_rpc_id() => Some(format!(
                "rejected: {:?}",
                response.iter().map(|(_, e)| e).collect::<Vec<_>>()
            )),
            Action::RpcEffectful(RpcEffectfulAction::TransactionInjectFailure {
                rpc_id,
                errors,
            }) if *rpc_id == our_rpc_id() => Some(format!("failure: {}", errors.join("; "))),
            _ => None,
        })
        .collect()
}

/// Plays the ledger service: answers the last account fetch with `accounts`.
fn fetched(
    state: &mut State,
    ran: &[Action],
    accounts: Result<BTreeMap<ledger::AccountId, ledger::Account>, String>,
) -> Vec<Action> {
    type Fetched = (
        Result<BTreeMap<ledger::AccountId, ledger::Account>, String>,
        Option<u32>,
        TransactionPoolMessageSource,
    );
    let fetch: (
        redux::Callback<Fetched>,
        Option<u32>,
        TransactionPoolMessageSource,
    ) = ran
        .iter()
        .rev()
        .find_map(|action| match action {
            Action::TransactionPoolEffect(TransactionPoolEffectfulAction::FetchAccounts {
                on_result,
                pending_id,
                from_source,
                ..
            }) => Some((on_result.clone(), *pending_id, *from_source)),
            _ => None,
        })
        .expect("an account fetch");
    let (on_result, pending_id, from_source) = fetch;
    let action: Action = on_result.call((accounts, pending_id, from_source));
    run(state, action)
}

/// Plays the verifier service: finishes the last verification.
fn verified(state: &mut State, ran: &[Action], ok: bool) -> Vec<Action> {
    let (req_id, commands) = ran
        .iter()
        .rev()
        .find_map(|action| match action {
            Action::Snark(SnarkAction::UserCommandVerifyEffect(
                SnarkUserCommandVerifyEffectfulAction::Init { req_id, commands },
            )) => Some((*req_id, commands.clone())),
            _ => None,
        })
        .expect("a verification");
    let action = if ok {
        let commands = commands
            .into_iter()
            .map(|cmd| match cmd.data {
                ledger::scan_state::transaction_logic::verifiable::UserCommand::SignedCommand(
                    c,
                ) => ledger::scan_state::transaction_logic::valid::UserCommand::SignedCommand(c),
                _ => unreachable!("the tests send payments"),
            })
            .collect();
        SnarkUserCommandVerifyAction::Success { req_id, commands }
    } else {
        SnarkUserCommandVerifyAction::Error {
            req_id,
            error: SnarkUserCommandVerifyError::Invalid("Invalid_signature".to_owned()),
        }
    };
    run(state, Action::Snark(SnarkAction::UserCommandVerify(action)))
}

#[test]
fn before_the_genesis_block_a_submission_is_answered_bootstrapping() {
    let mut state = fresh_state();
    let ran = run(&mut state, inject(vec![payment()]));
    assert_eq!(
        answers(&ran),
        vec![format!("failure: {TRANSACTION_INJECT_BOOTSTRAPPING}")]
    );
    assert!(!ran.iter().any(|a| matches!(
        a,
        Action::TransactionPool(TransactionPoolAction::StartVerify { .. })
    )));
}

/// The fallback in the pool itself, should a submission reach it before
/// the genesis block exists.
#[test]
fn before_the_genesis_block_the_pool_answers_a_submission() {
    let mut state = fresh_state();
    state.rpc.requests.insert(
        our_rpc_id(),
        crate::rpc::RpcRequestState {
            req: crate::rpc::RpcRequest::TransactionInject(vec![payment()]),
            status: crate::rpc::RpcRequestStatus::Pending { time: now() },
            data: Default::default(),
        },
    );
    let ran = run(
        &mut state,
        TransactionPoolAction::StartVerify {
            commands: [mina_core::transaction::TransactionWithHash::try_new(payment()).unwrap()]
                .into_iter()
                .collect(),
            from_source: TransactionPoolMessageSource::rpc(our_rpc_id()),
        },
    );
    assert_eq!(
        answers(&ran),
        vec![format!("failure: {TRANSACTION_INJECT_BOOTSTRAPPING}")]
    );
}

#[test]
fn an_empty_submission_is_answered() {
    let mut state = synced_state();
    let ran = run(&mut state, inject(vec![]));
    assert_eq!(answers(&ran), vec!["failure: no transaction to submit"]);
}

/// The genesis block exists but there is no best tip yet. (An old genesis
/// best tip, mainnet before its first sync, is the `bootstrapping` rule's
/// unit test: no fixture block here is a genesis block.)
#[test]
fn with_a_genesis_block_but_no_best_tip_a_submission_is_answered_bootstrapping() {
    let mut state = synced_state();
    state.transition_frontier.best_chain.clear();
    let ran = run(&mut state, inject(vec![payment()]));
    assert_eq!(
        answers(&ran),
        vec![format!("failure: {TRANSACTION_INJECT_BOOTSTRAPPING}")]
    );
}

/// A re-bootstrap: the old chain is still the best chain while the sync
/// fetches ledgers.
#[test]
fn during_a_re_bootstrap_a_submission_is_answered_bootstrapping() {
    let mut state = synced_state();
    let block = block();
    state.transition_frontier.sync = TransitionFrontierSyncState::StakingLedgerSuccess {
        time: now(),
        best_tip: block.clone(),
        root_block: block,
        blocks_inbetween: vec![],
        needed_protocol_states: Default::default(),
    };
    let ran = run(&mut state, inject(vec![payment()]));
    assert_eq!(
        answers(&ran),
        vec![format!("failure: {TRANSACTION_INJECT_BOOTSTRAPPING}")]
    );
}

#[test]
fn when_synced_a_submission_reaches_the_pool() {
    let mut state = synced_state();
    let ran = run(&mut state, inject(vec![payment()]));
    assert_eq!(answers(&ran), Vec::<String>::new(), "not answered yet");
    assert!(ran.iter().any(|a| matches!(
        a,
        Action::TransactionPoolEffect(TransactionPoolEffectfulAction::FetchAccounts { .. })
    )));
}

/// The verifier's error callback was a TODO: a bad signature hung the
/// request.
#[test]
fn an_invalid_signature_is_answered() {
    let mut state = synced_state();
    let ran = run(&mut state, inject(vec![payment()]));
    let ran = fetched(&mut state, &ran, Ok(BTreeMap::new()));
    let ran = verified(&mut state, &ran, false);
    assert_eq!(answers(&ran), vec!["failure: Invalid_signature"]);
}

/// A missing fee payer rejected the whole diff with an `Err` that nothing
/// answered (C6). Now it is that command's `Fee_payer_account_not_found`.
#[test]
fn a_missing_fee_payer_is_answered() {
    let mut state = synced_state();
    let ran = run(&mut state, inject(vec![payment()]));
    let ran = fetched(&mut state, &ran, Ok(BTreeMap::new()));
    let ran = verified(&mut state, &ran, true);
    let ran = fetched(&mut state, &ran, Ok(BTreeMap::new()));
    assert_eq!(
        answers(&ran),
        vec!["rejected: [FeePayerAccountNotFound]".to_owned()]
    );
}

#[test]
fn a_ledger_error_is_answered() {
    let mut state = synced_state();
    let ran = run(&mut state, inject(vec![payment()]));
    let ran = fetched(&mut state, &ran, Err("no such ledger".to_owned()));
    assert_eq!(answers(&ran), vec!["failure: no such ledger"]);
    assert!(state.transaction_pool.pending_actions.is_empty());
}

/// A gossiped diff while bootstrapping is ignored at once, not queued until
/// the sync ends.
#[test]
fn gossip_while_bootstrapping_is_ignored_not_queued() {
    let mut state = synced_state();
    state.p2p.initialize(&mina_core::MAINNET_CHAIN_ID).unwrap();
    let block = block();
    state.transition_frontier.sync = TransitionFrontierSyncState::StakingLedgerSuccess {
        time: now(),
        best_tip: block.clone(),
        root_block: block,
        blocks_inbetween: vec![],
        needed_protocol_states: Default::default(),
    };
    let message_id = crate::p2p::P2pNetworkPubsubMessageCacheId {
        source: crate::p2p::libp2p_identity::PeerId::random(),
        seqno: 1,
    };
    let ran = run(
        &mut state,
        TransactionPoolAction::Candidate(
            crate::transaction_pool::candidate::TransactionPoolCandidateAction::Libp2pTransactionsReceived {
                peer_id: crate::p2p::identity::SecretKey::rand().public_key().peer_id(),
                transactions: vec![
                    mina_core::transaction::TransactionWithHash::try_new(payment()).unwrap()
                ],
                message_id,
            },
        ),
    );
    assert_eq!(state.transaction_pool.telemetry.diffs_received, 1);
    assert!(ran.iter().any(|a| matches!(
        a,
        Action::P2p(crate::p2p::P2pAction::Network(
            crate::p2p::P2pNetworkAction::Pubsub(
                crate::p2p::P2pNetworkPubsubAction::IgnoreMessage { .. }
            )
        ))
    )));
    assert!(state
        .transaction_pool
        .candidates
        .get_batch_to_verify()
        .is_none());
}

#[test]
fn one_rebroadcast_bundle_increments_one_diff_counter() {
    let mut state = synced_state();
    let command = ledger::transaction_pool::transaction_hash::hash_command(
        ledger::scan_state::transaction_logic::valid::UserCommand::try_from(&payment()).unwrap(),
    );
    run(
        &mut state,
        TransactionPoolAction::Rebroadcast {
            accepted: vec![command.clone(), command],
            rejected: vec![],
            is_local: true,
        },
    );
    assert_eq!(state.transaction_pool.telemetry.diffs_broadcasted, 1);
}

#[test]
fn admitted_payment_increments_added_counter() {
    use ledger::scan_state::{currency::Balance, transaction_logic::valid::UserCommand};

    let payment = payment();
    let command = UserCommand::try_from(&payment).unwrap();
    let id = command.fee_payer();
    let mut account =
        ledger::Account::create_with(id.clone(), Balance::from_u64(1_000_000_000_000));
    account.nonce = command.nonce().unwrap();
    let accounts = BTreeMap::from([(id, account)]);

    let mut state = synced_state();
    let ran = run(&mut state, inject(vec![payment]));
    let ran = fetched(&mut state, &ran, Ok(accounts.clone()));
    let ran = verified(&mut state, &ran, true);
    let ran = fetched(&mut state, &ran, Ok(accounts));
    assert_eq!(answers(&ran), vec!["success"]);
    assert_eq!(state.transaction_pool.size(), 1);
    assert_eq!(state.transaction_pool.telemetry.transactions_added, 1);
}

#[test]
fn admitted_gossip_payment_counts_one_forwarded_diff() {
    use ledger::scan_state::{currency::Balance, transaction_logic::valid::UserCommand};

    let payment = payment();
    let command = UserCommand::try_from(&payment).unwrap();
    let id = command.fee_payer();
    let mut account =
        ledger::Account::create_with(id.clone(), Balance::from_u64(1_000_000_000_000));
    account.nonce = command.nonce().unwrap();
    let accounts = BTreeMap::from([(id, account)]);
    let message_id = crate::p2p::P2pNetworkPubsubMessageCacheId {
        source: crate::p2p::libp2p_identity::PeerId::random(),
        seqno: 1,
    };

    let mut state = synced_state();
    let ran = run(
        &mut state,
        TransactionPoolAction::StartVerify {
            commands: [mina_core::transaction::TransactionWithHash::try_new(payment).unwrap()]
                .into_iter()
                .collect(),
            from_source: TransactionPoolMessageSource::pubsub(message_id),
        },
    );
    let ran = fetched(&mut state, &ran, Ok(accounts.clone()));
    let ran = verified(&mut state, &ran, true);
    fetched(&mut state, &ran, Ok(accounts));
    assert_eq!(state.transaction_pool.size(), 1);
    assert_eq!(state.transaction_pool.telemetry.transactions_added, 1);
    assert_eq!(state.transaction_pool.telemetry.diffs_broadcasted, 1);
}
