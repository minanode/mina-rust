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
            seed_only: false,
            sharing: Default::default(),
            meshsub: P2pMeshsubConfig::default(),
            catchup_rpc: Default::default(),
        },
        transition_frontier: TransitionFrontierConfig::new(genesis),
        archive: None,
        block_producer: None,
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
    dispatch(state, action.into(), &mut ran, false);
    ran
}

/// As `run`, but also returns the actions dispatched while disabled, such as
/// a pubsub verdict on a message the test did not put in the message cache.
fn run_attempted(state: &mut State, action: impl Into<Action>) -> Vec<Action> {
    let mut ran = Vec::new();
    dispatch(state, action.into(), &mut ran, true);
    ran
}

fn dispatch(state: &mut State, action: Action, ran: &mut Vec<Action>, record_disabled: bool) {
    let time = now();
    if !action.is_enabled(state, time) {
        if record_disabled {
            ran.push(action);
        }
        return;
    }
    ran.push(action.clone());
    let action = ActionMeta::zero_custom(time).with_action(action);
    let mut dispatcher = redux::Dispatcher::new();
    crate::reducer(state, &action, &mut dispatcher);
    while let Some(next) = dispatcher.pop() {
        dispatch(state, next, ran, record_disabled);
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
    fetched_with(state, ran, accounts, |state, action| run(state, action))
}

fn fetched_with(
    state: &mut State,
    ran: &[Action],
    accounts: Result<BTreeMap<ledger::AccountId, ledger::Account>, String>,
    runner: impl FnOnce(&mut State, Action) -> Vec<Action>,
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
    runner(state, action)
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
        digest: [0; 32],
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
        digest: [0; 32],
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

/// The first two signed payments of the fixture block, a payout's
/// consecutive nonces from one sender, with the sender's account at the first.
fn two_payments() -> (
    [MinaBaseUserCommandStableV2; 2],
    BTreeMap<ledger::AccountId, ledger::Account>,
) {
    use ledger::scan_state::{currency::Balance, transaction_logic::valid::UserCommand};

    let diff = fixture_block().body.staged_ledger_diff.diff.clone();
    let picked = diff
        .0
        .commands
        .into_iter()
        .chain(diff.1.into_iter().flat_map(|d| d.commands))
        .map(|c| c.data)
        .filter(|c| matches!(c, MinaBaseUserCommandStableV2::SignedCommand(_)))
        .take(2)
        .collect::<Vec<_>>();
    let [first, second]: [MinaBaseUserCommandStableV2; 2] = picked
        .try_into()
        .expect("two payments in the fixture block");
    let (a, b) = (
        UserCommand::try_from(&first).unwrap(),
        UserCommand::try_from(&second).unwrap(),
    );
    assert_eq!(a.fee_payer(), b.fee_payer());
    assert_eq!(b.nonce().unwrap(), a.nonce().unwrap().incr());
    let id = a.fee_payer();
    let mut account =
        ledger::Account::create_with(id.clone(), Balance::from_u64(1_000_000_000_000));
    account.nonce = a.nonce().unwrap();
    ([first, second], BTreeMap::from([(id, account)]))
}

/// The sources of the verified diffs that reached the pool.
fn applied_sources(ran: &[Action]) -> Vec<TransactionPoolMessageSource> {
    ran.iter()
        .filter_map(|action| match action {
            Action::TransactionPool(TransactionPoolAction::ApplyVerifiedDiff {
                from_source,
                ..
            }) => Some(*from_source),
            _ => None,
        })
        .collect()
}

fn pubsub_verdicts(ran: &[Action]) -> usize {
    use crate::p2p::{P2pAction, P2pNetworkAction, P2pNetworkPubsubAction};
    ran.iter()
        .filter(|action| {
            matches!(
                action,
                Action::P2p(P2pAction::Network(P2pNetworkAction::Pubsub(
                    P2pNetworkPubsubAction::BroadcastValidatedMessage { .. }
                        | P2pNetworkPubsubAction::RejectMessage { .. }
                        | P2pNetworkPubsubAction::IgnoreMessage { .. }
                )))
            )
        })
        .count()
}

/// A gossip message overlapping an earlier one is verified whole, from its
/// own commands, and forwarded on its own verdict: the command it shares is a
/// duplicate in the pool, and its new command is admitted.
#[test]
fn an_overlapping_gossip_message_is_verified_whole_and_forwarded() {
    use crate::transaction_pool::candidate::TransactionPoolCandidateAction;
    use mina_core::transaction::TransactionWithHash;

    let ([shared, own], accounts) = two_payments();
    let (shared, own) = (
        TransactionWithHash::try_new(shared).unwrap(),
        TransactionWithHash::try_new(own).unwrap(),
    );
    let origin = crate::p2p::libp2p_identity::PeerId::random();
    let message = |seqno| crate::p2p::P2pNetworkPubsubMessageCacheId {
        source: origin,
        seqno,
        digest: [0; 32],
    };
    let peer = crate::p2p::identity::SecretKey::rand()
        .public_key()
        .peer_id();
    let mut state = synced_state();
    state.p2p.initialize(&mina_core::MAINNET_CHAIN_ID).unwrap();
    for (seqno, transactions) in [(1, vec![shared.clone()]), (2, vec![shared, own])] {
        run(
            &mut state,
            TransactionPoolAction::Candidate(
                TransactionPoolCandidateAction::Libp2pTransactionsReceived {
                    peer_id: peer,
                    transactions,
                    message_id: message(seqno),
                },
            ),
        );
    }
    // Every action of one verification, from `VerifyNext` to the pool.
    let verify_next = |state: &mut State| {
        let mut all = run(
            state,
            TransactionPoolAction::Candidate(TransactionPoolCandidateAction::VerifyNext),
        );
        let ran = fetched(state, &all, Ok(accounts.clone()));
        all.extend(ran.iter().cloned());
        let ran = verified(state, &ran, true);
        all.extend(ran.iter().cloned());
        all.extend(fetched_with(
            state,
            &ran,
            Ok(accounts.clone()),
            |state, action| run_attempted(state, action),
        ));
        all
    };

    let telemetry = |state: &State| {
        let t = &state.transaction_pool.telemetry;
        (t.transactions_added, t.diffs_broadcasted)
    };

    let ran = verify_next(&mut state);
    assert!(matches!(
        applied_sources(&ran)[..],
        [TransactionPoolMessageSource::Pubsub { id }] if id == message(1)
    ));
    assert_eq!(state.transaction_pool.size(), 1);
    assert_eq!(pubsub_verdicts(&ran), 1, "message 1 got no verdict");
    assert!(ran.iter().any(|action| matches!(
        action,
        Action::P2p(crate::p2p::P2pAction::Network(crate::p2p::P2pNetworkAction::Pubsub(
            crate::p2p::P2pNetworkPubsubAction::BroadcastValidatedMessage {
                message_id: crate::p2p::BroadcastMessageId::MessageId { message_id },
            }
        ))) if *message_id == message(1)
    )));
    // One command added, and one diff forwarded: the message itself.
    assert_eq!(telemetry(&state), (1, 1));

    let ran = verify_next(&mut state);
    assert!(matches!(
        applied_sources(&ran)[..],
        [TransactionPoolMessageSource::Pubsub { id }] if id == message(2)
    ));
    assert_eq!(state.transaction_pool.size(), 2);
    assert_eq!(pubsub_verdicts(&ran), 1, "message 2 got no single verdict");
    assert!(ran.iter().any(|action| matches!(
        action,
        Action::P2p(crate::p2p::P2pAction::Network(crate::p2p::P2pNetworkAction::Pubsub(
            crate::p2p::P2pNetworkPubsubAction::BroadcastValidatedMessage {
                message_id: crate::p2p::BroadcastMessageId::MessageId { message_id },
            }
        ))) if *message_id == message(2)
    )));
    assert!(!ran.iter().any(|action| matches!(
        action,
        Action::TransactionPool(TransactionPoolAction::Rebroadcast { .. })
    )));
    // One more command added, and one more diff forwarded: message 2.
    assert_eq!(telemetry(&state), (2, 2));
    assert!(state
        .transaction_pool
        .candidates
        .get_batch_to_verify()
        .is_none());
}

/// Two copies of one payment share a hash and differ in their signature,
/// which the hash leaves out. The queue verifies the forged copy first and
/// the verifier refuses it: the valid copy, from another peer, must still be
/// queued, and reach the pool in its own turn.
#[test]
fn a_refused_copy_does_not_take_a_valid_copy_with_it() {
    use crate::transaction_pool::candidate::TransactionPoolCandidateAction;
    use mina_core::transaction::TransactionWithHash;

    let ([good, other], accounts) = two_payments();
    let MinaBaseUserCommandStableV2::SignedCommand(mut forged) = good.clone() else {
        unreachable!("two_payments() returns payments")
    };
    let MinaBaseUserCommandStableV2::SignedCommand(other) = other else {
        unreachable!("two_payments() returns payments")
    };
    forged.signature = other.signature;
    let forged =
        TransactionWithHash::try_new(MinaBaseUserCommandStableV2::SignedCommand(forged)).unwrap();
    let good = TransactionWithHash::try_new(good).unwrap();
    assert_eq!(forged.hash(), good.hash());
    assert_ne!(forged.body(), good.body());

    let origin = crate::p2p::libp2p_identity::PeerId::random();
    let message = |seqno| crate::p2p::P2pNetworkPubsubMessageCacheId {
        source: origin,
        seqno,
        digest: [0; 32],
    };
    let mut state = synced_state();
    state.p2p.initialize(&mina_core::MAINNET_CHAIN_ID).unwrap();
    for (seqno, copy) in [(1, forged), (2, good)] {
        run(
            &mut state,
            TransactionPoolAction::Candidate(
                TransactionPoolCandidateAction::Libp2pTransactionsReceived {
                    peer_id: crate::p2p::identity::SecretKey::rand()
                        .public_key()
                        .peer_id(),
                    transactions: vec![copy],
                    message_id: message(seqno),
                },
            ),
        );
    }
    let verify_next =
        || TransactionPoolAction::Candidate(TransactionPoolCandidateAction::VerifyNext);

    // The forged copy: its signature does not verify.
    let ran = run(&mut state, verify_next());
    let ran = fetched(&mut state, &ran, Ok(accounts.clone()));
    verified(&mut state, &ran, false);
    assert_eq!(state.transaction_pool.size(), 0);

    // The valid copy is still queued, and reaches the pool.
    let ran = run(&mut state, verify_next());
    let ran = fetched(&mut state, &ran, Ok(accounts.clone()));
    let ran = verified(&mut state, &ran, true);
    fetched(&mut state, &ran, Ok(accounts));
    assert_eq!(state.transaction_pool.size(), 1, "the valid copy was lost");
    assert!(state
        .transaction_pool
        .candidates
        .get_batch_to_verify()
        .is_none());
}

/// Two copies of one payment share a hash and differ in their signature,
/// which the hash leaves out. The queue verifies the forged copy first and
/// the verifier refuses it: the valid copy, from the same relay, must still
/// be queued, and reach the pool in its own turn.
#[test]
fn a_refused_copy_from_one_relay_does_not_take_its_valid_copy() {
    use crate::transaction_pool::candidate::TransactionPoolCandidateAction;
    use mina_core::transaction::TransactionWithHash;

    let ([good, other], accounts) = two_payments();
    let MinaBaseUserCommandStableV2::SignedCommand(mut forged) = good.clone() else {
        unreachable!("two_payments() returns payments")
    };
    let MinaBaseUserCommandStableV2::SignedCommand(other) = other else {
        unreachable!("two_payments() returns payments")
    };
    forged.signature = other.signature;
    let forged =
        TransactionWithHash::try_new(MinaBaseUserCommandStableV2::SignedCommand(forged)).unwrap();
    let good = TransactionWithHash::try_new(good).unwrap();
    assert_eq!(forged.hash(), good.hash());
    assert_ne!(forged.body(), good.body());

    let origin = crate::p2p::libp2p_identity::PeerId::random();
    let message = |seqno| crate::p2p::P2pNetworkPubsubMessageCacheId {
        source: origin,
        seqno,
        digest: [0; 32],
    };
    let mut state = synced_state();
    state.p2p.initialize(&mina_core::MAINNET_CHAIN_ID).unwrap();
    let relay = crate::p2p::identity::SecretKey::rand()
        .public_key()
        .peer_id();
    for (seqno, copy) in [(1, forged), (2, good)] {
        run(
            &mut state,
            TransactionPoolAction::Candidate(
                TransactionPoolCandidateAction::Libp2pTransactionsReceived {
                    peer_id: relay,
                    transactions: vec![copy],
                    message_id: message(seqno),
                },
            ),
        );
    }
    let verify_next =
        || TransactionPoolAction::Candidate(TransactionPoolCandidateAction::VerifyNext);

    // The forged copy: its signature does not verify.
    let ran = run(&mut state, verify_next());
    let ran = fetched(&mut state, &ran, Ok(accounts.clone()));
    verified(&mut state, &ran, false);
    assert_eq!(state.transaction_pool.size(), 0);

    // The valid copy is still queued, and reaches the pool.
    let ran = run(&mut state, verify_next());
    let ran = fetched(&mut state, &ran, Ok(accounts.clone()));
    let ran = verified(&mut state, &ran, true);
    fetched(&mut state, &ran, Ok(accounts));
    assert_eq!(state.transaction_pool.size(), 1, "the valid copy was lost");
    assert!(state
        .transaction_pool
        .candidates
        .get_batch_to_verify()
        .is_none());
}
