use std::{sync::Arc, time::Duration};

use ledger::proofs::provers::BlockProver;
use mina_node::{
    account::{AccountPublicKey, AccountSecretKey},
    core::{consensus::ConsensusConstants, constants::constraint_constants},
    p2p::{
        channels::ChannelId, connection::outgoing::P2pConnectionOutgoingInitOpts,
        identity::SecretKey as P2pSecretKey, P2pLimits, P2pMeshsubConfig, P2pTimeouts,
    },
    snark::{get_srs, BlockVerifier, TransactionVerifier, VerifierSRS},
    transition_frontier::{genesis::GenesisConfig, persist::TransitionFrontierPersistConfig},
    BlockProducerConfig, GlobalConfig, LedgerConfig, P2pConfig, SnarkConfig, SnarkerConfig,
    SnarkerStrategy, TransitionFrontierConfig,
};
use mina_node_common::{p2p::TaskSpawner, NodeServiceCommonBuilder};
use mina_p2p_messages::v2::{self, NonZeroCurvePoint};
use rand::Rng;

use super::{Node, P2pTaskSpawner};

pub struct NodeBuilder {
    rng_seed: [u8; 32],
    custom_initial_time: Option<redux::Timestamp>,
    genesis_config: Arc<GenesisConfig>,
    p2p_sec_key: Option<P2pSecretKey>,
    p2p_is_seed: bool,
    p2p_no_discovery: bool,
    p2p_is_started: bool,
    initial_peers: Vec<P2pConnectionOutgoingInitOpts>,
    block_producer: Option<BlockProducerConfig>,
    snarker: Option<SnarkerConfig>,
    service: NodeServiceCommonBuilder,
    verifier_srs: Option<Arc<VerifierSRS>>,
    block_verifier_index: Option<BlockVerifier>,
    work_verifier_index: Option<TransactionVerifier>,
    persist: TransitionFrontierPersistConfig,
}

impl NodeBuilder {
    pub fn new(custom_rng_seed: Option<[u8; 32]>, genesis_config: Arc<GenesisConfig>) -> Self {
        let rng_seed = custom_rng_seed.unwrap_or_else(|| rand::thread_rng().gen());
        Self {
            rng_seed,
            custom_initial_time: None,
            genesis_config,
            p2p_sec_key: None,
            p2p_is_seed: false,
            p2p_no_discovery: false,
            p2p_is_started: false,
            initial_peers: Vec::new(),
            block_producer: None,
            snarker: None,
            service: NodeServiceCommonBuilder::new(rng_seed),
            verifier_srs: None,
            block_verifier_index: None,
            work_verifier_index: None,
            persist: Default::default(),
        }
    }

    /// Set custom initial time. Used for testing.
    pub fn custom_initial_time(&mut self, time: redux::Timestamp) -> &mut Self {
        self.custom_initial_time = Some(time);
        self
    }

    /// If not called, random one will be generated and used instead.
    pub fn p2p_sec_key(&mut self, key: P2pSecretKey) -> &mut Self {
        assert!(self.p2p_sec_key.is_none());
        self.p2p_sec_key = Some(key);
        self
    }

    /// Set up node as a seed node.
    pub fn p2p_seed_node(&mut self) -> &mut Self {
        self.p2p_is_seed = true;
        self
    }

    pub fn p2p_no_discovery(&mut self) -> &mut Self {
        self.p2p_no_discovery = true;
        self
    }

    /// Extend p2p initial peers from an iterable.
    pub fn initial_peers(
        &mut self,
        peers: impl IntoIterator<Item = P2pConnectionOutgoingInitOpts>,
    ) -> &mut Self {
        self.initial_peers.extend(peers);
        self
    }

    // /// Extend p2p initial peers by opening the url.
    // pub async fn initial_peers_from_url(
    //     &mut self,
    //     url: impl reqwest::IntoUrl,
    // ) -> anyhow::Result<&mut Self> {
    //     let url = url.into_url().context("failed to parse peers url")?;
    //     peers_from_reader(
    //         &mut self.initial_peers,
    //         reqwest::get(url.clone()).await
    //             .context(anyhow::anyhow!("reading peer list url {url}"))?,
    //     )
    //     .context(anyhow::anyhow!("reading peer list url {url}"))?;
    //     Ok(self)
    // }

    /// ICE servers of the node's WebRTC connections (W1). Call it before
    /// `p2p_custom_task_spawner`.
    pub fn p2p_webrtc_config(&mut self, config: mina_node::p2p::webrtc::WebrtcConfig) -> &mut Self {
        assert!(
            !self.p2p_is_started,
            "p2p_webrtc_config must come before the p2p service starts"
        );
        self.service.p2p_webrtc_config(config);
        self
    }

    /// Override default p2p task spawner.
    pub fn p2p_custom_task_spawner(
        &mut self,
        spawner: impl TaskSpawner,
    ) -> anyhow::Result<&mut Self> {
        let sec_key = self.p2p_sec_key.get_or_insert_with(P2pSecretKey::rand);
        self.service.p2p_init(sec_key.clone(), spawner);
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

    /// Set up the isolated prover, an instance of this module with its own
    /// memory (I34c), so that nothing is ever proved in the node's memory:
    /// for a block producer with `producer`'s key, whose proofs it checks
    /// with `producer`'s verifier, or for the SNARK worker if `work` (P5.3;
    /// see [`Self::snarker`]). Not both: a block producer's prover refuses the
    /// SNARK worker (`isolated::ProverRole`), so that no block proof waits
    /// behind SNARK work. The first instance starts loading its circuits at
    /// once.
    pub fn isolated_prover(
        &mut self,
        producer: Option<(AccountSecretKey, BlockVerifier)>,
        work: bool,
        network: String,
    ) -> &mut Self {
        // The node's own verifier SRS, the one `build` uses.
        let srs = self.verifier_srs.clone().unwrap_or_else(get_srs);
        let producer = producer.map(|(key, verifier)| {
            self.block_producer = Some(BlockProducerConfig {
                pub_key: key.public_key().into(),
                custom_coinbase_receiver: None,
                proposed_protocol_version: None,
                block_inject_delay_ms: mina_node::block_producer::default_block_inject_delay_ms(),
            });
            let checker = mina_node_common::block_producer::isolated::BlockProofChecker {
                verifier,
                srs: srs.clone(),
            };
            (key, checker)
        });
        crate::block_prover::start(&mut self.service, producer, work, network);
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

    pub fn custom_block_producer_config(
        &mut self,
        config: BlockProducerConfig,
    ) -> anyhow::Result<&mut Self> {
        *self.block_producer.as_mut().ok_or_else(|| {
            anyhow::anyhow!("block producer not initialized! Call `block_producer` function first.")
        })? = config;
        Ok(self)
    }

    /// Run the SNARK worker. `public_key` receives the fees, `fee` is in
    /// nanomina. As with OCaml's `--run-snark-worker`, no private key is needed.
    /// The web node's worker proves in its isolated prover
    /// ([`Self::isolated_prover`] with `work`), which proves no blocks.
    /// `block_first_lead`: the longest a job may take, for a worker whose
    /// prover competes with the block producer's, so that it takes none
    /// when one of our blocks is due within that time
    /// (`SnarkerConfig::block_first_lead`); `None` in the web node.
    /// The isolated prover has no zkApp step circuits, so the worker takes
    /// no zkApp command job (`SnarkerConfig::skip_zkapp_jobs`).
    pub fn snarker(
        &mut self,
        public_key: AccountPublicKey,
        fee: u64,
        strategy: SnarkerStrategy,
        block_first_lead: Option<Duration>,
    ) -> &mut Self {
        let config = SnarkerConfig {
            public_key,
            fee: v2::CurrencyFeeStableV1(v2::UnsignedExtendedUInt64Int64ForVersionTagsStableV1(
                fee.into(),
            )),
            strategy,
            auto_commit: true,
            block_first_lead,
            // The isolated prover loads no zkApp step circuits (P5.3).
            skip_zkapp_jobs: true,
        };
        self.snarker = Some(config);
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

    /// Turns the frontier store on (I5e): the node keeps its verified
    /// ledgers and blocks in this browser's Origin Private File System and
    /// restores from them at its next start, with the checks a native node
    /// with `--frontier-store` runs. `reset` deletes the store when it is
    /// opened. Without OPFS the node runs without a store.
    pub fn frontier_store(
        &mut self,
        config: TransitionFrontierPersistConfig,
        reset: bool,
    ) -> &mut Self {
        let build = mina_node::BuildEnv::get();
        let node_commit = format!("{} {}", build.version, build.git.commit_hash);
        let client = crate::frontier_store::start_file_server();
        self.service.frontier_store_init_with(
            move |chain_id| crate::frontier_store::backend(&client, chain_id),
            reset,
            node_commit,
            // No signals in the browser: a closed tab counts as a crash.
            Default::default(),
        );
        self.persist = TransitionFrontierPersistConfig {
            enabled: true,
            ..config
        };
        self
    }

    pub fn gather_stats(&mut self) -> &mut Self {
        self.service.gather_stats();
        self
    }

    pub fn build(self) -> anyhow::Result<Node> {
        self.genesis_config.initialize_runtime()?;
        let daemon = self
            .genesis_config
            .runtime_config()?
            .daemon
            .unwrap_or(mina_node::daemon_json::Daemon::DEFAULT);
        let p2p_sec_key = self.p2p_sec_key.unwrap_or_else(P2pSecretKey::rand);
        let initial_peers = if self.initial_peers.is_empty() && !self.p2p_is_seed {
            default_peers()
        } else {
            self.initial_peers
        };
        let initial_peers = initial_peers
            .into_iter()
            .filter(|opts| *opts.peer_id() != p2p_sec_key.public_key().peer_id())
            .collect();

        let srs = self.verifier_srs.unwrap_or_else(get_srs);
        let block_verifier_index = self
            .block_verifier_index
            .ok_or_else(|| anyhow::anyhow!("block verifier index not set on the node builder!"))?;
        let work_verifier_index = self.work_verifier_index.ok_or_else(|| {
            anyhow::anyhow!("transaction verifier index not set on the node builder!")
        })?;

        let mut transition_frontier = TransitionFrontierConfig::new(self.genesis_config);
        transition_frontier.persist = self.persist;

        let protocol_constants = transition_frontier.genesis.protocol_constants()?;
        let consensus_consts =
            ConsensusConstants::create(constraint_constants(), &protocol_constants);

        // build config
        let snarker_strategy = self
            .snarker
            .as_ref()
            .map(|snarker| snarker.strategy)
            .unwrap_or_default();
        let node_config = mina_node::Config {
            global: GlobalConfig {
                build: mina_node::BuildEnv::get().into(),
                snarker: self.snarker,
                consensus_constants: consensus_consts.clone(),
                testing_run: false,
                client_port: None,
                chain_id_override: None,
                skip_proof_verification: false,
                worker_control: false,
                snarker_strategy,
                block_first_lead: None,
            },
            p2p: P2pConfig {
                catchup_rpc: Default::default(),
                libp2p_port: None,
                listen_port: None,
                identity_pub_key: p2p_sec_key.public_key(),
                initial_peers,
                external_addrs: vec![],
                enabled_channels: ChannelId::iter_all().collect(),
                peer_discovery: !self.p2p_no_discovery,
                meshsub: P2pMeshsubConfig {
                    initial_time: Duration::ZERO,
                    ..Default::default()
                },
                timeouts: P2pTimeouts::default(),
                limits: P2pLimits::default().with_max_peers(Some(100)),
            },
            ledger: LedgerConfig {},
            snark: SnarkConfig {
                block_verifier_index,
                block_verifier_srs: srs.clone(),
                work_verifier_index,
                work_verifier_srs: srs,
            },
            transition_frontier,
            block_producer: self.block_producer,
            tx_pool: ledger::transaction_pool::Config {
                trust_system: (),
                pool_max_size: daemon.tx_pool_max_size(),
                slot_tx_end: daemon.slot_tx_end(),
            },
            archive: None,
        };

        // build service
        let mut service = self.service;
        service.ledger_init();

        if !self.p2p_is_started {
            service.p2p_init(p2p_sec_key, P2pTaskSpawner {});
        }

        let service = service.build()?;

        let initial_time = self
            .custom_initial_time
            .unwrap_or_else(redux::Timestamp::global_now);
        let state = mina_node::State::new(node_config, &consensus_consts, initial_time);

        Ok(Node::new(self.rng_seed, state, service, None))
    }
}

fn default_peers() -> Vec<P2pConnectionOutgoingInitOpts> {
    ["/2bjYBqn45MmtismsAYP9rZ6Xns9snCcNsN1eDgQZB5s6AzY2CR2/https/webrtc3.webnode.openmina.com/443"]
        .into_iter()
        .map(|s| s.parse().unwrap())
        .collect()
}

// fn peers_from_reader(
//     peers: &mut Vec<P2pConnectionOutgoingInitOpts>,
//     read: impl Read,
// ) -> anyhow::Result<()> {
//     let read = BufReader::new(read);
//     for line in read.lines() {
//         let line = line.context("reading line")?;
//         let l = line.trim();
//         if !l.is_empty() {
//             peers.push(l.parse().context(anyhow::anyhow!("parsing entry"))?);
//         }
//     }
//     Ok(())
// }
