use super::{
    archive::ArchiveService,
    block_producer::BlockProducerService,
    p2p::webrtc_with_libp2p::P2pServiceCtx,
    replay::ReplayerState,
    rpc::{RpcSender, RpcService},
    snark_worker::SnarkWorker,
    snarks::SnarkBlockVerifyArgs,
    EventReceiver, EventSender,
};
use crate::rpc::RpcReceiver;
use mina_node::{
    core::{channels::mpsc, invariants::InvariantsState},
    event_source::Event,
    ledger::LedgerManager,
    p2p::identity::SecretKey as P2pSecretKey,
    service::Recorder,
    stats::Stats,
    transition_frontier::genesis::{same_genesis, GenesisCache, GenesisConfig},
};
use rand::{rngs::StdRng, SeedableRng};
use sha3::{
    digest::{core_api::XofReaderCoreWrapper, ExtendableOutput, Update},
    Shake256, Shake256ReaderCore,
};
use std::sync::Arc;

pub struct NodeService {
    /// Master seed for deterministic random number generation.
    pub rng_seed: [u8; 32],
    /// XOF-based RNG for ephemeral keys (derived from seed + "ephemeral").
    pub rng_ephemeral: XofReaderCoreWrapper<Shake256ReaderCore>,
    /// XOF-based RNG for static operations (derived from seed + "static").
    pub rng_static: XofReaderCoreWrapper<Shake256ReaderCore>,
    /// Standard RNG for general-purpose randomness.
    pub rng: StdRng,

    /// Events sent on this channel are retrieved and processed in the
    /// `event_source` state machine defined in the `mina-node` crate.
    pub event_sender: EventSender,
    /// Channel for consuming events in the event source state machine.
    pub event_receiver: EventReceiver,

    /// Channel for asynchronous block proof verification requests.
    pub snark_block_proof_verify: mpsc::TrackedUnboundedSender<SnarkBlockVerifyArgs>,

    /// Manages ledger operations, database access, and staged ledger state.
    pub ledger_manager: LedgerManager,
    /// SNARK proof worker for generating transaction proofs (enabled when node
    /// acts as SNARK worker).
    pub snark_worker: Option<SnarkWorker>,
    /// The web node's isolated prover: when set, the SNARK worker proves
    /// there (P5.3) instead of in a thread of this memory.
    pub isolated_prover: Option<crate::block_producer::IsolatedBlockProver>,
    /// Block production service including VRF evaluation and block proving
    /// (enabled when node acts as block producer).
    pub block_producer: Option<BlockProducerService>,
    /// Archive service for storing full blockchain history (enabled when node
    /// acts as archive node).
    pub archive: Option<ArchiveService>,
    /// P2P networking context (WebRTC and optionally libp2p transports).
    pub p2p: P2pServiceCtx,

    /// Runtime statistics and metrics collection.
    pub stats: Option<Stats>,
    /// RPC service for external API queries.
    pub rpc: RpcService,
    /// Records node state and actions for debugging and replay.
    pub recorder: Recorder,
    /// Replayer state for deterministic action replay (only set in replay
    /// mode).
    pub replayer: Option<ReplayerState>,
    /// State for runtime invariant checking and validation.
    pub invariants_state: InvariantsState,
    /// The frontier store thread (R20 D3), if persistence is on.
    pub persist: Option<mina_node::persist::writer::PersistWriter>,
    /// Set to give up the load for a restore under way (I5d1).
    pub persist_load_cancel: Option<Arc<mina_node::persist::load_control::LoadControl>>,
    /// Where the genesis of a daemon JSON config is kept after a full load
    /// (I5d2), for the node's next start.
    pub genesis_cache: Option<GenesisCache>,
    /// The SNARK worker's setting and included-work log in the work
    /// directory (P5.4); none in the web node.
    pub snark_worker_files: Option<crate::snark_worker_files::SnarkWorkerFiles>,
    /// Where the block producer's history is kept across restarts.
    pub block_production_file: Option<crate::block_production_file::BlockProductionFile>,
}

impl NodeService {
    pub fn event_sender(&self) -> &EventSender {
        &self.event_sender
    }

    pub fn rpc_sender(&self) -> RpcSender {
        self.rpc.req_sender()
    }

    pub fn event_receiver_with_rpc_receiver(&mut self) -> (&mut EventReceiver, &mut RpcReceiver) {
        (&mut self.event_receiver, self.rpc.req_receiver())
    }

    pub fn event_receiver(&mut self) -> &mut EventReceiver {
        &mut self.event_receiver
    }

    pub fn rpc_receiver(&mut self) -> &mut RpcReceiver {
        self.rpc.req_receiver()
    }

    pub fn ledger_manager(&self) -> &LedgerManager {
        &self.ledger_manager
    }

    pub fn block_producer(&self) -> Option<&BlockProducerService> {
        self.block_producer.as_ref()
    }

    pub fn archive(&self) -> Option<&ArchiveService> {
        self.archive.as_ref()
    }

    pub fn stats(&mut self) -> Option<&mut Stats> {
        self.stats.as_mut()
    }

    pub fn replayer(&mut self) -> Option<&mut ReplayerState> {
        self.replayer.as_mut()
    }
}

impl NodeService {
    pub fn for_replay(
        rng_seed: [u8; 32],
        initial_time: redux::Timestamp,
        p2p_sec_key: P2pSecretKey,
        dynamic_effects_lib: Option<String>,
    ) -> Self {
        Self {
            rng_seed,
            rng_ephemeral: Shake256::default()
                .chain(rng_seed)
                .chain(b"ephemeral")
                .finalize_xof(),
            rng_static: Shake256::default()
                .chain(rng_seed)
                .chain(b"static")
                .finalize_xof(),
            rng: StdRng::from_seed(rng_seed),
            event_sender: mpsc::unbounded_channel().0,
            event_receiver: mpsc::unbounded_channel().1.into(),
            snark_block_proof_verify: mpsc::unbounded_channel().0,
            ledger_manager: LedgerManager::spawn(Default::default()),
            snark_worker: None,
            isolated_prover: None,
            block_producer: None,
            archive: None,
            p2p: P2pServiceCtx::mocked(p2p_sec_key),
            stats: Some(Stats::new()),
            rpc: RpcService::new(),
            recorder: Recorder::None,
            replayer: Some(ReplayerState {
                initial_monotonic: redux::Instant::now(),
                initial_time,
                expected_actions: Default::default(),
                replay_dynamic_effects_lib: dynamic_effects_lib.unwrap_or_default(),
            }),
            invariants_state: Default::default(),
            persist: None,
            persist_load_cancel: None,
            genesis_cache: None,
            snark_worker_files: None,
            block_production_file: None,
        }
    }
}

impl AsMut<NodeService> for NodeService {
    fn as_mut(&mut self) -> &mut NodeService {
        self
    }
}

impl redux::Service for NodeService {}

impl mina_node::Service for NodeService {
    fn queues(&mut self) -> mina_node::service::Queues {
        mina_node::service::Queues {
            events: self.event_receiver.len(),
            snark_block_verify: self.snark_block_proof_verify.len(),
            ledger: self.ledger_manager.pending_calls(),
            vrf_evaluator: self
                .block_producer
                .as_ref()
                .map(|v| v.vrf_pending_requests()),
            block_prover: self
                .block_producer
                .as_ref()
                .map(|v| v.prove_pending_requests()),
            p2p_webrtc: self.p2p.webrtc.pending_cmds(),
            #[cfg(feature = "p2p-libp2p")]
            p2p_libp2p: self.p2p.mio.pending_cmds(),
            rpc: self.rpc.req_receiver().len(),
        }
    }

    fn stats(&mut self) -> Option<&mut Stats> {
        self.stats()
    }

    fn recorder(&mut self) -> &mut Recorder {
        &mut self.recorder
    }

    fn is_replay(&self) -> bool {
        self.replayer.is_some()
    }
}

impl redux::TimeService for NodeService {
    fn monotonic_time(&mut self) -> redux::Instant {
        self.replayer
            .as_ref()
            .map(|v| v.next_monotonic_time())
            .unwrap_or_else(redux::Instant::now)
    }
}

impl mina_node::service::EventSourceService for NodeService {
    fn next_event(&mut self) -> Option<Event> {
        // RPC requests (from the page or the HTTP server) are taken between
        // events too, not only in the run loop's `select!`, which it doesn't
        // reach while the event queue never drains.
        let now = redux::TimeService::monotonic_time(self);
        self.rpc.next_event(now, &mut self.event_receiver)
    }
}

impl mina_node::service::LedgerService for NodeService {
    fn ledger_manager(&self) -> &LedgerManager {
        &self.ledger_manager
    }

    fn force_sync_calls(&self) -> bool {
        self.replayer.is_some()
    }
}

impl mina_node::service::TransitionFrontierGenesisService for NodeService {
    fn load_genesis(&mut self, config: Arc<GenesisConfig>) {
        let res = match config.load() {
            Err(err) => Err(err.to_string()),
            Ok((masks, data)) => {
                let is_archive = self.archive().is_some();
                masks.into_iter().for_each(|mut mask| {
                    if !is_archive {
                        // Optimization: We don't need token owners if the node is not an archive
                        mask.unset_token_owners();
                    }
                    self.ledger_manager.insert_genesis_ledger(mask);
                });
                Ok(data)
            }
        };
        if let (Ok(data), Some(cache)) = (&res, &self.genesis_cache) {
            if matches!(
                *config,
                GenesisConfig::DaemonJson(_) | GenesisConfig::DaemonJsonFile(_)
            ) {
                // For the next start (I5d2): its genesis without the ledgers.
                match cache.write(data) {
                    Ok(()) => mina_node::core::info!(
                        mina_node::core::log::system_time();
                        kind = "GenesisCache",
                        summary = "kept the genesis for the next start",
                        path = cache.path().display().to_string(),
                    ),
                    Err(error) => mina_node::core::warn!(
                        mina_node::core::log::system_time();
                        kind = "GenesisCache",
                        summary = "could not keep the genesis for the next start",
                        path = cache.path().display().to_string(),
                        error = error.to_string(),
                    ),
                }
            }
        }
        let _ = self.event_sender.send(Event::GenesisLoad(res));
    }

    fn load_genesis_ledgers(&mut self, config: Arc<GenesisConfig>, expected: Arc<GenesisConfig>) {
        let is_archive = self.archive().is_some();
        let insert = self.ledger_manager.genesis_ledger_inserter();
        let cache = self.genesis_cache.clone();
        let event_sender = self.event_sender.clone();
        let load = move || {
            let started = std::time::Instant::now();
            let res = config
                .load()
                .map_err(|error| error.to_string())
                .and_then(|(masks, loaded)| {
                    let GenesisConfig::WithoutLedgers(expected) = &*expected else {
                        return Err("the node's genesis is not one without ledgers".to_owned());
                    };
                    if !same_genesis(&loaded, expected) {
                        // The kept genesis isn't this config's, so neither is
                        // the node's genesis block: drop the entry and stop,
                        // and the next start loads the config in full.
                        if let Some(cache) = &cache {
                            let _ = cache.remove();
                        }
                        mina_node::core::error!(
                            mina_node::core::log::system_time();
                            kind = "GenesisCache",
                            summary = "the kept genesis is not the config's: the entry is dropped and the node stops",
                        );
                        std::process::exit(1);
                    }
                    let count = masks.len();
                    for mut mask in masks {
                        if !is_archive {
                            mask.unset_token_owners();
                        }
                        insert(mask);
                    }
                    Ok(count)
                });
            match &res {
                Ok(count) => mina_node::core::info!(
                    mina_node::core::log::system_time();
                    kind = "GenesisCache",
                    summary = format!("loaded the {count} deferred genesis ledgers"),
                    elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                ),
                Err(error) => mina_node::core::warn!(
                    mina_node::core::log::system_time();
                    kind = "GenesisCache",
                    summary = "could not load the deferred genesis ledgers",
                    error = error.clone(),
                ),
            }
            let _ = event_sender.send(Event::GenesisLedgersLoad(res.map(|_| ())));
        };
        #[cfg(not(target_family = "wasm"))]
        {
            let spawned = std::thread::Builder::new()
                .name("genesis-ledgers".into())
                .spawn(load);
            if let Err(error) = spawned {
                let _ = self
                    .event_sender
                    .send(Event::GenesisLedgersLoad(Err(error.to_string())));
            }
        }
        #[cfg(target_family = "wasm")]
        load();
    }
}

impl mina_node::service::TransitionFrontierPersistService for NodeService {
    fn persist_open(
        &mut self,
        chain_id: mina_node::core::ChainId,
        genesis_state_hash: mina_p2p_messages::v2::StateHash,
    ) -> bool {
        use mina_node::persist::writer::PersistJob;
        self.persist.as_ref().is_some_and(|persist| {
            persist.send(PersistJob::Open {
                chain_id,
                genesis_state_hash,
            })
        })
    }

    fn persist_chain_put(
        &mut self,
        chain: Vec<mina_node::persist::ChainEntry>,
        blocks: Vec<mina_node::core::block::ArcBlockWithHash>,
    ) -> bool {
        use mina_node::persist::writer::PersistJob;
        self.persist
            .as_ref()
            .is_some_and(|persist| persist.send(PersistJob::ChainPut { chain, blocks }))
    }

    fn persist_load(
        &mut self,
        request: mina_node::persist::load::PersistLoadRequest,
        verifier: Option<mina_node::persist::load::BlockProofVerifier>,
    ) -> bool {
        use mina_node::persist::writer::{LoadJob, PersistJob};
        let Some(persist) = self.persist.as_ref() else {
            return false;
        };
        let control = Arc::new(mina_node::persist::load_control::LoadControl::default());
        self.persist_load_cancel = Some(control.clone());
        persist.send(PersistJob::Load(Box::new(LoadJob {
            request,
            verifier,
            insert: self.ledger_manager.persist_inserter(),
            control,
        })))
    }

    fn persist_load_progress(&self) -> u64 {
        self.persist_load_cancel
            .as_ref()
            .map_or(0, |control| control.progress())
    }

    fn persist_load_cancel(&mut self) -> bool {
        self.persist_load_cancel
            .as_ref()
            .is_some_and(|control| control.cancel())
    }

    fn persist_restore_done(&mut self) -> bool {
        use mina_node::persist::writer::PersistJob;
        self.persist_load_cancel = None;
        self.persist
            .as_ref()
            .is_some_and(|persist| persist.send(PersistJob::RestoreDone))
    }

    fn persist_discard(&mut self, reason: String) -> bool {
        use mina_node::persist::writer::PersistJob;
        self.persist
            .as_ref()
            .is_some_and(|persist| persist.send(PersistJob::Discard { reason }))
    }
}

impl mina_node::core::invariants::InvariantService for NodeService {
    type ClusterInvariantsState<'a> = std::cell::RefMut<'a, InvariantsState>;

    fn invariants_state(&mut self) -> &mut InvariantsState {
        &mut self.invariants_state
    }
}
