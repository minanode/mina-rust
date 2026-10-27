//! Node builder for configuring and constructing native Mina nodes.
//!
//! See [`NodeBuilder`] for the main entry point.

use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    net::IpAddr,
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::Context;
use ledger::proofs::provers::BlockProver;
use mina_core::{consensus::ConsensusConstants, constants::constraint_constants};
use mina_node::{
    account::{AccountPublicKey, AccountSecretKey},
    daemon_json::Daemon,
    p2p::{
        channels::ChannelId, connection::outgoing::P2pConnectionOutgoingInitOpts,
        identity::SecretKey as P2pSecretKey, P2pLimits, P2pMeshsubConfig, P2pTimeouts,
    },
    service::Recorder,
    snark::{get_srs, BlockVerifier, TransactionVerifier, VerifierSRS},
    transition_frontier::{
        archive::archive_config::ArchiveConfig, genesis::GenesisConfig,
        persist::TransitionFrontierPersistConfig,
    },
    BlockProducerConfig, GlobalConfig, LedgerConfig, P2pConfig, SnarkConfig, SnarkerConfig,
    SnarkerSetting, SnarkerStrategy, TransitionFrontierConfig,
};
use mina_node_common::{archive::config::ArchiveStorageOptions, p2p::TaskSpawner};
use mina_p2p_messages::v2::NonZeroCurvePoint;
use rand::Rng;

use crate::NodeServiceBuilder;

use super::Node;

/// Builder for constructing a native Mina node with fluent API.
pub struct NodeBuilder {
    /// Seed for RNG, used in `Node::new()` for replay. If `None` in constructor,
    /// a random seed is generated.
    rng_seed: [u8; 32],
    /// If `Some`, overrides system time (for testing). If `None`, uses real time.
    custom_initial_time: Option<redux::Timestamp>,
    /// Genesis block configuration.
    genesis_config: Arc<GenesisConfig>,
    /// P2P networking configuration.
    p2p: P2pConfig,
    /// If `Some`, uses provided key. If `None`, `P2pSecretKey::rand()` in `build()`.
    p2p_sec_key: Option<P2pSecretKey>,
    /// If `true`, node acts as seed (no initial peers needed).
    p2p_is_seed: bool,
    /// If `true`, P2P service already spawned via custom task spawner.
    p2p_is_started: bool,
    /// If `Some`, enables block production. If `None`, node is non-producing.
    block_producer: Option<BlockProducerConfig>,
    /// If `Some`, enables archive mode. If `None`, no archiving.
    archive: Option<ArchiveConfig>,
    /// If `Some`, enables SNARK worker. If `None`, no SNARK work.
    snarker: Option<SnarkerSetting>,
    /// How the SNARK worker selects jobs, also one started later (P5.4).
    snarker_strategy: SnarkerStrategy,
    block_first_lead: Option<Duration>,
    /// Service builder for I/O components.
    service: NodeServiceBuilder,
    /// If `Some`, uses provided SRS. If `None`, `get_srs()` in `build()`.
    verifier_srs: Option<Arc<VerifierSRS>>,
    /// If `Some`, uses provided index. If `None`, `BlockVerifier::make()` in `build()`.
    block_verifier_index: Option<BlockVerifier>,
    /// If `Some`, uses provided. If `None`, `TransactionVerifier::make()` in `build()`.
    work_verifier_index: Option<TransactionVerifier>,
    /// If `Some`, starts HTTP RPC server on port. If `None`, no RPC server.
    http_port: Option<u16>,
    /// Daemon JSON configuration.
    daemon_conf: Daemon,
    /// If `Some`, overrides computed chain ID (for interop testing).
    chain_id_override: Option<mina_core::ChainId>,
    /// If `true`, skips SNARK proof verification (for interop testing).
    skip_proof_verification: bool,
    /// The frontier store (R20), off by default.
    persist: TransitionFrontierPersistConfig,
    /// The full genesis config whose ledgers are loaded only if the node
    /// bootstraps (I5d2).
    genesis_ledgers_deferred: Option<Arc<GenesisConfig>>,
    /// The operator may start, stop and re-price the SNARK worker while the
    /// node runs (P5.4).
    worker_control: bool,
    /// The included-work log the last run kept (P5.4).
    included_work: Option<mina_node::external_snark_worker::IncludedWorkLog>,
    /// Why the saved worker setting was not used (P5.4).
    worker_setting_warning: Option<String>,
    /// The block production history the last run kept (F-producer).
    block_production: Option<mina_node::stats::block_producer::BlockProductionHistory>,
}

impl NodeBuilder {
    pub fn new(
        custom_rng_seed: Option<[u8; 32]>,
        daemon_conf: Daemon,
        genesis_config: Arc<GenesisConfig>,
    ) -> Self {
        let rng_seed = custom_rng_seed.unwrap_or_else(|| {
            let mut seed = [0; 32];
            getrandom::getrandom(&mut seed).unwrap_or_else(|_| {
                seed = rand::thread_rng().gen();
            });
            seed
        });
        Self {
            rng_seed,
            custom_initial_time: None,
            genesis_config,
            p2p: P2pConfig {
                catchup_rpc: Default::default(),
                libp2p_port: None,
                listen_port: None,
                // Must be replaced with builder api.
                identity_pub_key: P2pSecretKey::deterministic(0).public_key(),
                initial_peers: Vec::new(),
                external_addrs: Vec::new(),
                enabled_channels: ChannelId::iter_all().collect(),
                peer_discovery: true,
                meshsub: P2pMeshsubConfig {
                    initial_time: Duration::ZERO,
                    ..Default::default()
                },
                timeouts: P2pTimeouts::default(),
                limits: P2pLimits::default().with_max_peers(Some(100)),
            },
            p2p_sec_key: None,
            p2p_is_seed: false,
            p2p_is_started: false,
            block_producer: None,
            archive: None,
            snarker: None,
            snarker_strategy: SnarkerStrategy::default(),
            block_first_lead: Some(Duration::from_secs(420)),
            service: NodeServiceBuilder::new(rng_seed),
            verifier_srs: None,
            block_verifier_index: None,
            work_verifier_index: None,
            http_port: None,
            daemon_conf,
            chain_id_override: None,
            skip_proof_verification: false,
            persist: Default::default(),
            genesis_ledgers_deferred: None,
            worker_control: false,
            included_work: None,
            worker_setting_warning: None,
            block_production: None,
        }
    }

    pub fn catchup_rpc(
        &mut self,
        enabled: bool,
        peers: impl IntoIterator<Item = mina_node::p2p::PeerId>,
    ) -> &mut Self {
        self.p2p.catchup_rpc = mina_node::p2p::P2pCatchupRpcConfig {
            enabled,
            peers: peers.into_iter().collect(),
        };
        self
    }

    /// Override chain ID for P2P networking (interop testing).
    pub fn chain_id_override(&mut self, chain_id: mina_core::ChainId) -> &mut Self {
        self.chain_id_override = Some(chain_id);
        self
    }

    /// Skip SNARK proof verification (interop testing).
    pub fn skip_proof_verification(&mut self, skip: bool) -> &mut Self {
        self.skip_proof_verification = skip;
        self
    }

    /// Set custom initial time. Used for testing.
    pub fn custom_initial_time(&mut self, time: redux::Timestamp) -> &mut Self {
        self.custom_initial_time = Some(time);
        self
    }

    /// If not called, random one will be generated and used instead.
    pub fn p2p_sec_key(&mut self, key: P2pSecretKey) -> &mut Self {
        self.p2p.identity_pub_key = key.public_key();
        self.p2p_sec_key = Some(key);
        self
    }

    /// Keep the default identity in the work directory. Call after explicit
    /// key options: they take precedence and do not read or write a key file.
    pub fn p2p_identity(&mut self, work_dir: &Path) -> anyhow::Result<&mut Self> {
        let key = crate::identity::resolve(work_dir, self.p2p_sec_key.clone())?;
        tracing::info!(peer_id = %key.public_key().peer_id().to_libp2p_string(), "P2P identity");
        Ok(self.p2p_sec_key(key))
    }

    pub fn p2p_libp2p_port(&mut self, port: u16) -> &mut Self {
        self.p2p.libp2p_port = Some(port);
        self
    }

    /// Set up node as a seed node.
    pub fn p2p_seed_node(&mut self) -> &mut Self {
        self.p2p_is_seed = true;
        self
    }

    pub fn p2p_no_discovery(&mut self) -> &mut Self {
        self.p2p.peer_discovery = false;
        self
    }

    /// Extend p2p initial peers from an iterable.
    pub fn initial_peers(
        &mut self,
        peers: impl IntoIterator<Item = P2pConnectionOutgoingInitOpts>,
    ) -> &mut Self {
        self.p2p.initial_peers.extend(peers);
        self
    }

    pub fn external_addrs(&mut self, v: impl Iterator<Item = IpAddr>) -> &mut Self {
        self.p2p.external_addrs.extend(v);
        self
    }

    /// ICE servers, UDP ports and NAT addresses of the node's WebRTC
    /// connections (W1). Call it before `p2p_custom_task_spawner`.
    pub fn p2p_webrtc_config(&mut self, config: mina_node::p2p::webrtc::WebrtcConfig) -> &mut Self {
        assert!(
            !self.p2p_is_started,
            "p2p_webrtc_config must come before the p2p service starts"
        );
        self.service.p2p_webrtc_config(config);
        self
    }

    /// Extend p2p initial peers from file.
    pub fn initial_peers_from_file(&mut self, path: impl AsRef<Path>) -> anyhow::Result<&mut Self> {
        peers_from_reader(
            &mut self.p2p.initial_peers,
            File::open(&path).context(anyhow::anyhow!(
                "opening peer list file {:?}",
                path.as_ref()
            ))?,
        )
        .context(anyhow::anyhow!(
            "reading peer list file {:?}",
            path.as_ref()
        ))?;

        Ok(self)
    }

    /// Extend p2p initial peers by opening the url.
    pub fn initial_peers_from_url(
        &mut self,
        url: impl reqwest::IntoUrl,
    ) -> anyhow::Result<&mut Self> {
        let url = url.into_url().context("failed to parse peers url")?;
        peers_from_reader(
            &mut self.p2p.initial_peers,
            reqwest::blocking::get(url.clone())
                .context(anyhow::anyhow!("reading peer list url {url}"))?,
        )
        .context(anyhow::anyhow!("reading peer list url {url}"))?;
        Ok(self)
    }

    pub fn p2p_max_peers(&mut self, limit: usize) -> &mut Self {
        self.p2p.limits = self.p2p.limits.with_max_peers(Some(limit));
        self
    }

    /// Override default p2p task spawner.
    pub fn p2p_custom_task_spawner(
        &mut self,
        spawner: impl TaskSpawner,
    ) -> anyhow::Result<&mut Self> {
        let sec_key: P2pSecretKey = self.p2p_sec_key.clone().ok_or_else(|| anyhow::anyhow!("before calling `with_p2p_custom_task_spawner` method, p2p secret key needs to be set with `with_p2p_sec_key`."))?;
        self.service
            .p2p_init_with_custom_task_spawner(sec_key, spawner);
        self.p2p_is_started = true;
        Ok(self)
    }

    /// Set up block producer.
    pub fn block_producer(
        &mut self,
        key: AccountSecretKey,
        provers: Option<BlockProver>,
    ) -> &mut Self {
        let config = BlockProducerConfig {
            pub_key: key.public_key().into(),
            custom_coinbase_receiver: None,
            proposed_protocol_version: None,
            block_inject_delay_ms: mina_node::block_producer::default_block_inject_delay_ms(),
        };
        self.block_producer = Some(config);
        self.service.block_producer_init(key, provers);
        self
    }

    /// Set up block producer using keys from file.
    pub fn block_producer_from_file(
        &mut self,
        path: impl AsRef<Path>,
        password: &str,
        provers: Option<BlockProver>,
    ) -> anyhow::Result<&mut Self> {
        let key = AccountSecretKey::from_encrypted_file(&path, password).with_context(|| {
            format!(
                "Failed to decrypt secret key file: {}",
                path.as_ref().display()
            )
        })?;
        Ok(self.block_producer(key, provers))
    }

    /// Turns the frontier store on (R20): the node writes each chain's store
    /// to `<base_dir>/<chain id>/frontier`, checkpointing its root every
    /// `checkpoint_interval_secs`. `reset` deletes the store when it is
    /// opened. A restart doesn't load it yet (I5d1 and I5d2).
    pub fn frontier_store(
        &mut self,
        base_dir: std::path::PathBuf,
        reset: bool,
        checkpoint_interval_secs: u64,
        restore: bool,
    ) -> &mut Self {
        self.persist = TransitionFrontierPersistConfig {
            enabled: true,
            checkpoint_interval_secs,
            restore,
            ..Default::default()
        };
        let build = mina_node::BuildEnv::get();
        let node_commit = format!("{} {}", build.version, build.git.commit_hash);
        self.service
            .frontier_store_init(base_dir, reset, node_commit);
        self
    }

    pub fn frontier_store_load_timeout(&mut self, seconds: u64) -> &mut Self {
        self.persist.load_timeout_secs = seconds;
        self
    }

    /// Keeps the genesis of the daemon JSON config in `cache` after its
    /// full load, for the next start (I5d2).
    pub fn genesis_cache(
        &mut self,
        cache: mina_node::transition_frontier::genesis::GenesisCache,
    ) -> &mut Self {
        self.service.genesis_cache_init(cache);
        self
    }

    /// The genesis config was kept by the genesis cache (I5d2): `full` is the
    /// config it was kept of, whose ledgers are loaded only if the node
    /// bootstraps (the restore from the frontier store replaces them).
    pub fn genesis_ledgers_deferred(&mut self, full: Arc<GenesisConfig>) -> &mut Self {
        self.genesis_ledgers_deferred = Some(full);
        self
    }

    pub fn archive(&mut self, options: ArchiveStorageOptions, work_dir: String) -> &mut Self {
        self.archive = Some(ArchiveConfig::new(work_dir.clone()));
        self.service.archive_init(options, work_dir.clone());
        self
    }

    /// Receive block producer's coinbase reward to another account.
    pub fn custom_coinbase_receiver(
        &mut self,
        addr: NonZeroCurvePoint,
    ) -> anyhow::Result<&mut Self> {
        let bp = self.block_producer.as_mut().ok_or_else(|| {
            anyhow::anyhow!(
                "can't set custom_coinbase_receiver when block producer is not initialized."
            )
        })?;
        bp.custom_coinbase_receiver = Some(addr);
        Ok(self)
    }

    pub fn block_inject_delay_ms(&mut self, delay_ms: u64) -> anyhow::Result<&mut Self> {
        let bp = self.block_producer.as_mut().ok_or_else(|| {
            anyhow::anyhow!(
                "can't set block_inject_delay_ms when block producer is not initialized."
            )
        })?;
        bp.block_inject_delay_ms =
            mina_node::block_producer::effective_block_inject_delay_ms(delay_ms);
        Ok(self)
    }

    pub fn custom_block_producer_config(
        &mut self,
        config: BlockProducerConfig,
    ) -> anyhow::Result<&mut Self> {
        let mut config = config;
        config.block_inject_delay_ms = mina_node::block_producer::effective_block_inject_delay_ms(
            config.block_inject_delay_ms,
        );
        *self.block_producer.as_mut().ok_or_else(|| {
            anyhow::anyhow!("block producer not initialized! Call `block_producer` function first.")
        })? = config;
        Ok(self)
    }

    /// Run the SNARK worker. `public_key` receives the fees, `fee` is in
    /// nanomina. As with OCaml's `--run-snark-worker`, no private key is needed.
    pub fn snarker(&mut self, public_key: AccountPublicKey, fee: u64) -> &mut Self {
        self.snarker = Some(SnarkerSetting { public_key, fee });
        self
    }

    /// How the SNARK worker selects jobs (`--snarker-strategy`): one started
    /// with the node, and one the operator starts later (P5.4). OCaml's
    /// default, random, if not set.
    pub fn snarker_strategy(&mut self, strategy: SnarkerStrategy) -> &mut Self {
        self.snarker_strategy = strategy;
        self
    }

    /// Lead before a known won slot for the native worker. `None` disables it.
    pub fn block_first_lead(&mut self, lead: Option<Duration>) -> &mut Self {
        self.block_first_lead = lead;
        self
    }

    /// Lets the operator start, stop and re-price the SNARK worker while the
    /// node runs (P5.4, `--allow-worker-control`). It changes where fees go:
    /// never with a public HTTP API.
    pub fn worker_control(&mut self, allowed: bool) -> &mut Self {
        self.worker_control = allowed;
        self
    }

    /// The SNARK worker setting saved in the work directory could not be
    /// used: the dashboard shows `warning` (P5.4).
    pub fn snark_worker_setting_warning(&mut self, warning: String) -> &mut Self {
        self.worker_setting_warning = Some(warning);
        self
    }

    /// Keeps the SNARK worker's setting and the log of our included work in
    /// `dir`, the work directory (P5.4), and takes up the log the last run
    /// kept. An unreadable log is started afresh, with a warning.
    pub fn snark_worker_files(&mut self, dir: std::path::PathBuf) -> &mut Self {
        let mut files = mina_node_common::snark_worker_files::SnarkWorkerFiles::new(dir);
        match files.load_included_work() {
            Ok(log) => self.included_work = log,
            Err(error) => mina_core::warn!(
                mina_core::log::system_time();
                kind = "SnarkWorkerIncludedWorkUnreadable",
                summary = "the included-work log could not be read: a new one is started",
                error = error.to_string(),
            ),
        }
        self.service.snark_worker_files_init(files);
        self
    }

    /// Keeps the block producer's history in `dir`, the work directory, and
    /// takes up what the last run kept (F-producer). An unreadable file is
    /// started afresh, with a warning.
    pub fn block_production_history(&mut self, dir: std::path::PathBuf) -> &mut Self {
        let file = mina_node_common::block_production_file::BlockProductionFile::new(dir);
        match file.load() {
            Ok(history) => self.block_production = history,
            Err(error) => mina_core::warn!(
                mina_core::log::system_time();
                kind = "BlockProductionHistoryUnreadable",
                summary = "the block production history could not be read: a new one is started",
                error = error.to_string(),
            ),
        }
        self.service.block_production_file_init(file);
        self
    }

    /// Set verifier srs. If not set, default will be used.
    pub fn verifier_srs(&mut self, srs: Arc<VerifierSRS>) -> &mut Self {
        self.verifier_srs = Some(srs);
        self
    }

    pub fn block_verifier_index(&mut self, index: BlockVerifier) -> &mut Self {
        self.block_verifier_index = Some(index);
        self
    }

    pub fn work_verifier_index(&mut self, index: TransactionVerifier) -> &mut Self {
        self.work_verifier_index = Some(index);
        self
    }

    pub fn gather_stats(&mut self) -> &mut Self {
        self.service.gather_stats();
        self
    }

    pub fn record(&mut self, recorder: Recorder) -> &mut Self {
        self.service.record(recorder);
        self
    }

    /// Starts the HTTP server on an already-bound operator listener.
    /// `tx_submission` allows `POST /send-payment` and the GraphQL mutations
    /// that submit transactions; it should be off on public relays.
    pub fn http_server(
        &mut self,
        listener: std::net::TcpListener,
        metrics_listener: Option<std::net::TcpListener>,
        metrics_store_root: Option<std::path::PathBuf>,
        options: crate::http_server::HttpServerOptions,
    ) -> std::io::Result<&mut Self> {
        self.http_port = Some(listener.local_addr()?.port());
        self.service
            .http_server_init(listener, metrics_listener, metrics_store_root, options)?;
        Ok(self)
    }

    /// Starts a separate signaling-only listener, already bound by the CLI.
    #[cfg(feature = "p2p-webrtc")]
    pub fn webrtc_signal_server(&mut self, listener: std::net::TcpListener) -> &mut Self {
        self.service.webrtc_signal_server_init(listener);
        self
    }

    pub fn build(mut self) -> anyhow::Result<Node> {
        self.genesis_config.initialize_runtime()?;
        if let Some(daemon) = self.genesis_config.runtime_config()?.daemon {
            self.daemon_conf = daemon;
        }
        let p2p_sec_key = self.p2p_sec_key.clone().unwrap_or_else(P2pSecretKey::rand);
        self.p2p_sec_key(p2p_sec_key.clone());
        if self.p2p.initial_peers.is_empty() && !self.p2p_is_seed {
            self.p2p.initial_peers = default_peers();
        }

        self.p2p.initial_peers = self
            .p2p
            .initial_peers
            .into_iter()
            .filter(|opts| *opts.peer_id() != p2p_sec_key.public_key().peer_id())
            .filter_map(|opts| match opts {
                P2pConnectionOutgoingInitOpts::LibP2P(mut opts) => {
                    opts.host = opts.host.resolve()?;
                    Some(P2pConnectionOutgoingInitOpts::LibP2P(opts))
                }
                x => Some(x),
            })
            .collect();

        let srs = self.verifier_srs.unwrap_or_else(get_srs);
        let block_verifier_index = self
            .block_verifier_index
            .unwrap_or_else(BlockVerifier::make);
        let work_verifier_index = self
            .work_verifier_index
            .unwrap_or_else(TransactionVerifier::make);

        let initial_time = self
            .custom_initial_time
            .unwrap_or_else(redux::Timestamp::global_now);
        self.p2p.meshsub.initial_time = initial_time
            .checked_sub(redux::Timestamp::ZERO)
            .unwrap_or_default();

        let protocol_constants = self.genesis_config.protocol_constants()?;
        let consensus_consts =
            ConsensusConstants::create(constraint_constants(), &protocol_constants);

        // build config
        let node_config = mina_node::Config {
            global: GlobalConfig {
                build: mina_node::BuildEnv::get().into(),
                snarker: self.snarker.map(|setting| {
                    let mut config =
                        SnarkerConfig::with_setting(&setting, None, self.snarker_strategy);
                    config.block_first_lead =
                        self.block_producer.as_ref().and(self.block_first_lead);
                    config
                }),
                consensus_constants: consensus_consts.clone(),
                testing_run: false,
                client_port: self.http_port,
                chain_id_override: self.chain_id_override,
                skip_proof_verification: self.skip_proof_verification,
                worker_control: self.worker_control,
                snarker_strategy: self.snarker_strategy,
                block_first_lead: self.block_producer.as_ref().and(self.block_first_lead),
            },
            p2p: self.p2p,
            ledger: LedgerConfig {},
            snark: SnarkConfig {
                block_verifier_index,
                block_verifier_srs: srs.clone(),
                work_verifier_index,
                work_verifier_srs: srs,
            },
            transition_frontier: TransitionFrontierConfig {
                persist: self.persist,
                genesis_ledgers_deferred: self.genesis_ledgers_deferred,
                ..TransitionFrontierConfig::new(self.genesis_config)
            },
            block_producer: self.block_producer,
            archive: self.archive,
            tx_pool: ledger::transaction_pool::Config {
                trust_system: (),
                pool_max_size: self.daemon_conf.tx_pool_max_size(),
                slot_tx_end: self.daemon_conf.slot_tx_end(),
            },
        };

        // build service
        let mut service = self.service;
        service.ledger_init();
        service.skip_proof_verification(self.skip_proof_verification);

        if !self.p2p_is_started {
            service.p2p_init(p2p_sec_key);
        }

        let mut service = service.build()?;
        if let (Some(history), Some(stats)) = (self.block_production, service.stats()) {
            stats.block_producer().restore(history);
            stats
                .block_producer()
                .resolve_restored_schedules(initial_time);
        }
        let mut state = mina_node::State::new(node_config, &consensus_consts, initial_time);
        if let Some(log) = self.included_work {
            state.external_snark_worker.restore_included_work(log);
        }
        if let Some(warning) = self.worker_setting_warning {
            state.external_snark_worker.set_setting_warning(warning);
        }

        Ok(Node::new(self.rng_seed, state, service, None))
    }
}

fn default_peers() -> Vec<P2pConnectionOutgoingInitOpts> {
    mina_core::NetworkConfig::global()
        .default_peers
        .iter()
        .map(|s| s.parse().unwrap())
        .collect()
}

fn peers_from_reader(
    peers: &mut Vec<P2pConnectionOutgoingInitOpts>,
    read: impl Read,
) -> anyhow::Result<()> {
    let reader = BufReader::new(read);
    for line in reader.lines() {
        let line = line.context("reading line")?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed.parse::<P2pConnectionOutgoingInitOpts>() {
            Ok(opts) => {
                if let Some(opts) = opts.with_host_resolved() {
                    peers.push(opts);
                } else {
                    mina_core::warn!(
                        "Peer address name resolution failed, skipping: {:?}",
                        trimmed
                    );
                }
            }
            Err(e) => mina_core::warn!("Peer address parse error: {:?}", e),
        }
    }
    Ok(())
}
