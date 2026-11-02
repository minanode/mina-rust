//! Injects the backend event through the production node event mapper and
//! reducers. Transport effects are observed; no network task is started.
use mina_node::{
    core::{consensus::ConsensusConstants, constants, MAINNET_CHAIN_ID},
    event_source::{event_source_effects, Event, EventSourceAction},
    p2p::{
        channels::{
            rpc::{P2pChannelsRpcAction, P2pChannelsRpcState},
            ChannelId,
        },
        disconnection::{P2pDisconnectionAction, P2pDisconnectionReason},
        disconnection_effectful::P2pDisconnectionEffectfulAction,
        identity::SecretKey,
        service_impl::TaskSpawner,
        P2pAction, P2pChannelEvent, P2pConnectionEvent, P2pEffectfulAction, P2pEvent, P2pPeerState,
        P2pPeerStatus, P2pPeerStatusReady,
    },
    snark::{get_srs, BlockVerifier, TransactionVerifier},
    Action, ActionWithMeta, Config, State, Store,
};
use mina_node_native::{NodeService, NodeServiceBuilder};
use redux::{EnablingCondition, Timestamp};
use std::{cell::RefCell, future::Future};

thread_local! {
    static ACTIONS: RefCell<Vec<P2pDisconnectionAction>> = const { RefCell::new(Vec::new()) };
    static CLOSES: RefCell<usize> = const { RefCell::new(0) };
}
#[derive(Clone)]
struct NoNetwork;
impl TaskSpawner for NoNetwork {
    fn spawn_main<F>(&self, _: &str, _: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
    }
}
fn effects(store: &mut Store<NodeService>, action: ActionWithMeta) {
    let (action, meta) = action.split();
    match action {
        Action::EventSource(event) => event_source_effects(store, meta.with_action(event)),
        Action::P2p(P2pAction::Disconnection(a)) => ACTIONS.with(|v| v.borrow_mut().push(a)),
        Action::P2pEffectful(P2pEffectfulAction::Disconnection(
            P2pDisconnectionEffectfulAction::Init { .. },
        )) => {
            CLOSES.with(|v| *v.borrow_mut() += 1);
        }
        _ => {}
    }
}

#[test]
fn opened_error_crosses_event_boundary_and_settles_once() {
    let constants = ConsensusConstants::create(
        constants::constraint_constants(),
        &constants::PROTOCOL_CONSTANTS,
    );
    let config = Config {
        ledger: mina_node::LedgerConfig {},
        snark: mina_node::SnarkConfig {
            block_verifier_index: BlockVerifier::make(),
            block_verifier_srs: get_srs(),
            work_verifier_index: TransactionVerifier::make(),
            work_verifier_srs: get_srs(),
        },
        p2p: mina_node::P2pConfig {
            libp2p_port: None,
            listen_port: None,
            identity_pub_key: SecretKey::deterministic(0).public_key(),
            initial_peers: vec![],
            external_addrs: vec![],
            enabled_channels: ChannelId::without_signaling().collect(),
            timeouts: Default::default(),
            limits: Default::default(),
            peer_discovery: false,
            seed_only: false,
            meshsub: Default::default(),
            catchup_rpc: Default::default(),
        },
        transition_frontier: mina_node::TransitionFrontierConfig::new(
            mina_node::config::DEVNET_CONFIG.clone(),
        ),
        archive: None,
        block_producer: None,
        global: mina_node::GlobalConfig {
            snark_work_fee: None,
            build: Box::new(mina_node::BuildEnv::get()),
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
    state.p2p.initialize(&MAINNET_CHAIN_ID).unwrap();
    let peer_id = SecretKey::deterministic(1).public_key().peer_id();
    let mut ready = P2pPeerStatusReady::new(
        true,
        Timestamp::ZERO,
        &ChannelId::without_signaling().collect(),
    );
    ready.channels.rpc = P2pChannelsRpcState::Pending {
        time: Timestamp::ZERO,
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
    let mut builder = NodeServiceBuilder::new([0; 32]);
    builder
        .ledger_init()
        .p2p_init_with_custom_task_spawner(SecretKey::deterministic(0), NoNetwork);
    let mut store = Store::new(
        mina_node::reducer,
        effects,
        builder.build().unwrap(),
        redux::SystemTime::UNIX_EPOCH,
        state,
    );
    let inject = |store: &mut Store<NodeService>, event: P2pEvent| {
        assert!(store.dispatch(EventSourceAction::NewEvent {
            event: Event::P2p(event)
        }));
    };
    let failure = P2pEvent::Channel(P2pChannelEvent::Opened(
        peer_id,
        ChannelId::Rpc,
        Err("closed during open".into()),
    ));
    inject(&mut store, failure.clone());
    assert!(matches!(
        store.state().p2p.ready().unwrap().peers[&peer_id].status,
        P2pPeerStatus::Disconnecting { .. }
    ));
    assert_eq!(CLOSES.with(|v| *v.borrow()), 1);
    ACTIONS.with(|v| {
        assert!(matches!(&v.borrow()[0], P2pDisconnectionAction::Init {
        reason: P2pDisconnectionReason::P2pChannelOpenFailed(ChannelId::Rpc, error), ..
    } if error == "closed during open"))
    });
    inject(&mut store, failure.clone());
    inject(
        &mut store,
        P2pEvent::Channel(P2pChannelEvent::Opened(peer_id, ChannelId::Rpc, Ok(()))),
    );
    assert_eq!(CLOSES.with(|v| *v.borrow()), 1);
    assert!(!P2pChannelsRpcAction::Ready { peer_id }
        .is_enabled(store.state().p2p.ready().unwrap(), Timestamp::ZERO));
    inject(
        &mut store,
        P2pEvent::Connection(P2pConnectionEvent::Closed(peer_id)),
    );
    assert!(matches!(
        store.state().p2p.ready().unwrap().peers[&peer_id].status,
        P2pPeerStatus::Disconnected { .. }
    ));
    inject(
        &mut store,
        P2pEvent::Connection(P2pConnectionEvent::Closed(peer_id)),
    );
    inject(&mut store, failure);
    ACTIONS.with(|v| {
        assert_eq!(
            v.borrow()
                .iter()
                .filter(|a| matches!(a, P2pDisconnectionAction::Finish { .. }))
                .count(),
            1
        )
    });
}
