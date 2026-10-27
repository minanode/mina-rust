use ledger::proofs::provers::BlockProver;
use mina_node::{
    account::AccountSecretKey,
    core::channels::mpsc,
    ledger::{LedgerCtx, LedgerManager},
    p2p::{
        identity::SecretKey as P2pSecretKey,
        service_impl::{
            webrtc_with_libp2p::{P2pServiceCtx, P2pServiceWebrtcWithLibp2p},
            TaskSpawner,
        },
        webrtc::WebrtcConfig,
    },
    stats::Stats,
};
use rand::{rngs::StdRng, SeedableRng};
use sha3::{
    digest::{ExtendableOutput, Update},
    Shake256,
};

use crate::{
    rpc::{RpcSender, RpcService},
    EventReceiver, EventSender, NodeService,
};

use super::{
    archive::{config::ArchiveStorageOptions, ArchiveService},
    block_producer::{isolated, BlockProducerService, IsolatedBlockProver},
};

pub struct NodeServiceCommonBuilder {
    rng_seed: [u8; 32],
    rng: StdRng,
    /// Events sent on this channel are retrieved and processed in the
    /// `event_source` state machine defined in the `mina-node` crate.
    event_sender: EventSender,
    event_receiver: EventReceiver,
    ledger_manager: Option<LedgerManager>,
    block_producer: Option<BlockProducerService>,
    /// The web node's isolated prover (I34c, P5.3), for its block producer
    /// and its SNARK worker.
    isolated_prover: Option<IsolatedBlockProver>,
    archive: Option<ArchiveService>,
    p2p: Option<P2pServiceCtx>,
    /// ICE servers, UDP ports and NAT addresses for WebRTC (W1), read by
    /// [`Self::p2p_init`].
    p2p_webrtc: WebrtcConfig,
    gather_stats: bool,
    rpc: RpcService,
    skip_proof_verification: bool,
    /// The frontier store thread, if persistence is on.
    persist: Option<mina_node::persist::writer::PersistWriter>,
    genesis_cache: Option<mina_node::transition_frontier::genesis::GenesisCache>,
    snark_worker_files: Option<super::snark_worker_files::SnarkWorkerFiles>,
    block_production_file: Option<super::block_production_file::BlockProductionFile>,
}

#[derive(thiserror::Error, Debug, Clone)]
pub enum NodeServiceCommonBuildError {
    #[error("ledger was never initialized! Please call: NodeServiceBuilder::ledger_init")]
    LedgerNotInit,
    #[error("p2p was never initialized! Please call: NodeServiceBuilder::p2p_init")]
    P2pNotInit,
}

impl NodeServiceCommonBuilder {
    pub fn new(rng_seed: [u8; 32]) -> Self {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        Self {
            rng_seed,
            rng: StdRng::from_seed(rng_seed),
            event_sender,
            event_receiver: event_receiver.into(),
            ledger_manager: None,
            block_producer: None,
            isolated_prover: None,
            archive: None,
            p2p: None,
            p2p_webrtc: WebrtcConfig::default(),
            rpc: RpcService::new(),
            gather_stats: false,
            skip_proof_verification: false,
            persist: None,
            genesis_cache: None,
            snark_worker_files: None,
            block_production_file: None,
        }
    }

    /// Keeps the SNARK worker's setting and included-work log in these
    /// files (P5.4).
    /// Keeps the block producer's history in this file (F-producer).
    pub fn block_production_file_init(
        &mut self,
        file: super::block_production_file::BlockProductionFile,
    ) -> &mut Self {
        self.block_production_file = Some(file);
        self
    }

    pub fn snark_worker_files_init(
        &mut self,
        files: super::snark_worker_files::SnarkWorkerFiles,
    ) -> &mut Self {
        self.snark_worker_files = Some(files);
        self
    }

    /// Keeps the genesis of a daemon JSON config in `cache` after its full
    /// load, for the node's next start (I5d2).
    pub fn genesis_cache_init(
        &mut self,
        cache: mina_node::transition_frontier::genesis::GenesisCache,
    ) -> &mut Self {
        self.genesis_cache = Some(cache);
        self
    }

    pub fn event_sender(&self) -> &EventSender {
        &self.event_sender
    }

    pub fn rpc_sender(&self) -> RpcSender {
        self.rpc.req_sender()
    }

    /// Starts the frontier store thread (R20 D3). Each chain's store is
    /// `<base_dir>/<chain id>/frontier`, opened when the node knows its chain
    /// id. Call before [`Self::ledger_init`], which hands the ledger thread
    /// the writer.
    #[cfg(not(target_family = "wasm"))]
    pub fn frontier_store_init(
        &mut self,
        base_dir: std::path::PathBuf,
        reset: bool,
        node_commit: String,
        restore_guard: mina_node::persist::load::RestoreGuard,
    ) -> &mut Self {
        use mina_node::persist::{fs_backend::store_dir, FsBackend};
        self.frontier_store_init_with(
            move |chain_id| {
                let dir = store_dir(&base_dir, chain_id);
                let backend = FsBackend::new(&dir)?;
                Ok((backend, dir.display().to_string()))
            },
            reset,
            node_commit,
            restore_guard,
        )
    }

    /// Starts the frontier store thread over the backend `make_backend`
    /// gives for a chain id, with a name for it in the logs: natively a
    /// directory, in the browser the Origin Private File System (I5e). Call
    /// before [`Self::ledger_init`].
    pub fn frontier_store_init_with<B, F>(
        &mut self,
        make_backend: F,
        reset: bool,
        node_commit: String,
        restore_guard: mina_node::persist::load::RestoreGuard,
    ) -> &mut Self
    where
        B: mina_node::persist::StoreBackend,
        F: Fn(&mina_node::core::ChainId) -> std::io::Result<(B, String)> + Send + 'static,
    {
        use mina_node::{
            event_source::Event,
            persist::writer::{PersistWriter, WriterOptions},
        };
        let event_sender = self.event_sender.clone();
        self.persist = Some(PersistWriter::spawn(
            make_backend,
            WriterOptions {
                reset,
                node_commit,
                restore_guard,
            },
            move |event| {
                let _ = event_sender.send(Event::Persist(event));
            },
        ));
        self
    }

    pub fn ledger_init(&mut self) -> &mut Self {
        let mut ctx = LedgerCtx::default();
        ctx.set_event_sender(self.event_sender.clone());
        if let Some(persist) = &self.persist {
            ctx.set_persist_writer(persist.clone());
        }
        if self.archive.is_some() {
            ctx.set_archive_mode();
        };
        self.ledger_manager = Some(LedgerManager::spawn(ctx));
        self
    }

    pub fn block_producer_init(
        &mut self,
        keypair: AccountSecretKey,
        provers: Option<BlockProver>,
    ) -> &mut Self {
        self.block_producer = Some(BlockProducerService::start(
            self.event_sender.clone(),
            keypair,
            provers,
        ));
        self
    }

    /// Starts the isolated prover's bridge (the web node), which starts its
    /// first prover instance at once. `checker` verifies its block proofs,
    /// if the node produces blocks. `transport` carries the bridge's
    /// commands to the prover instances; their answers go to the returned
    /// sender. The block producer ([`Self::block_producer_init_isolated`])
    /// and the SNARK worker, if the node runs one, then prove there.
    pub fn isolated_prover_init(
        &mut self,
        checker: Option<isolated::BlockProofChecker>,
        transport: impl FnMut(isolated::Command) + Send + 'static,
    ) -> isolated::BridgeInputSender {
        let (prover, input) =
            IsolatedBlockProver::start(self.event_sender.clone(), checker, transport);
        self.isolated_prover = Some(prover);
        input
    }

    /// A block producer that proves in the isolated prover started by
    /// [`Self::isolated_prover_init`].
    pub fn block_producer_init_isolated(&mut self, keypair: AccountSecretKey) -> &mut Self {
        let prover = self
            .isolated_prover
            .clone()
            .expect("isolated_prover_init must be called first");
        self.block_producer = Some(BlockProducerService::start_isolated(
            self.event_sender.clone(),
            keypair,
            prover,
        ));
        self
    }

    pub fn archive_init(&mut self, options: ArchiveStorageOptions, work_dir: String) -> &mut Self {
        self.archive = Some(ArchiveService::start(options, work_dir));
        self
    }

    /// WebRTC settings for [`Self::p2p_init`]: call it before.
    pub fn p2p_webrtc_config(&mut self, config: WebrtcConfig) -> &mut Self {
        self.p2p_webrtc = config;
        self
    }

    pub fn p2p_init<S: TaskSpawner>(
        &mut self,
        secret_key: P2pSecretKey,
        task_spawner: S,
    ) -> &mut Self {
        self.p2p = Some(<NodeService as P2pServiceWebrtcWithLibp2p>::init(
            secret_key.clone(),
            task_spawner,
            self.rng_seed,
            self.p2p_webrtc.clone(),
        ));
        self
    }

    pub fn gather_stats(&mut self) -> &mut Self {
        self.gather_stats = true;
        self
    }

    pub fn skip_proof_verification(&mut self, skip: bool) -> &mut Self {
        self.skip_proof_verification = skip;
        self
    }

    pub fn build(self) -> Result<NodeService, NodeServiceCommonBuildError> {
        let ledger_manager = self
            .ledger_manager
            .ok_or(NodeServiceCommonBuildError::LedgerNotInit)?;
        let p2p = self.p2p.ok_or(NodeServiceCommonBuildError::P2pNotInit)?;
        let mut block_producer = self.block_producer;
        if let Some(block_producer) = &mut block_producer {
            block_producer.skip_proof_verification(self.skip_proof_verification);
        }

        Ok(NodeService {
            rng_seed: self.rng_seed,
            rng_ephemeral: Shake256::default()
                .chain(self.rng_seed)
                .chain(b"ephemeral")
                .finalize_xof(),
            rng_static: Shake256::default()
                .chain(self.rng_seed)
                .chain(b"static")
                .finalize_xof(),
            rng: self.rng,
            event_sender: self.event_sender.clone(),
            event_receiver: self.event_receiver,
            snark_block_proof_verify: NodeService::snark_block_proof_verifier_spawn(
                self.event_sender,
                self.skip_proof_verification,
            ),
            ledger_manager,
            block_producer,
            // initialized in state machine.
            snark_worker: None,
            isolated_prover: self.isolated_prover,
            archive: self.archive,
            p2p,
            stats: self.gather_stats.then(Stats::new),
            rpc: self.rpc,
            recorder: Default::default(),
            replayer: None,
            invariants_state: Default::default(),
            persist: self.persist,
            persist_load_cancel: None,
            genesis_cache: self.genesis_cache,
            snark_worker_files: self.snark_worker_files,
            block_production_file: self.block_production_file,
        })
    }
}
