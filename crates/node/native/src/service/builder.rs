use ledger::proofs::provers::BlockProver;
use mina_node::{
    account::AccountSecretKey, core::thread, p2p::identity::SecretKey as P2pSecretKey,
    service::Recorder,
};
pub use mina_node_common::NodeServiceCommonBuildError;
use mina_node_common::{
    archive::config::ArchiveStorageOptions, p2p::TaskSpawner, rpc::RpcSender, EventSender,
    NodeServiceCommonBuilder,
};

use crate::{http_server, NodeService, P2pTaskSpawner};

pub struct NodeServiceBuilder {
    common: NodeServiceCommonBuilder,
    pub(super) recorder: Recorder,
    http_server_port: Option<u16>,
}

#[derive(thiserror::Error, derive_more::From, Debug, Clone)]
pub enum NodeServiceBuildError {
    #[error("error when building common parts of the service: {0}")]
    Common(NodeServiceCommonBuildError),
}

impl NodeServiceBuilder {
    pub fn new(rng_seed: [u8; 32]) -> Self {
        Self {
            common: NodeServiceCommonBuilder::new(rng_seed),
            recorder: Default::default(),
            http_server_port: None,
        }
    }

    pub fn event_sender(&self) -> &EventSender {
        self.common.event_sender()
    }

    pub fn rpc_sender(&self) -> RpcSender {
        self.common.rpc_sender()
    }

    pub fn ledger_init(&mut self) -> &mut Self {
        self.common.ledger_init();
        self
    }

    /// Starts the frontier store thread; see
    /// [`NodeServiceCommonBuilder::frontier_store_init`].
    pub fn frontier_store_init(
        &mut self,
        base_dir: std::path::PathBuf,
        reset: bool,
        node_commit: String,
    ) -> &mut Self {
        let restore_guard = mina_node::persist::load::RestoreGuard::default();
        crate::shutdown::take_back_restore_on_shutdown(restore_guard.clone());
        self.common
            .frontier_store_init(base_dir, reset, node_commit, restore_guard);
        self
    }

    /// See [`NodeServiceCommonBuilder::genesis_cache_init`].
    pub fn genesis_cache_init(
        &mut self,
        cache: mina_node::transition_frontier::genesis::GenesisCache,
    ) -> &mut Self {
        self.common.genesis_cache_init(cache);
        self
    }

    pub fn block_producer_init(
        &mut self,
        keypair: AccountSecretKey,
        provers: Option<BlockProver>,
    ) -> &mut Self {
        self.common.block_producer_init(keypair, provers);
        self
    }

    pub fn archive_init(&mut self, options: ArchiveStorageOptions, work_dir: String) -> &mut Self {
        self.common.archive_init(options, work_dir);
        self
    }

    /// WebRTC settings for the p2p service: call it before `p2p_init`.
    pub fn p2p_webrtc_config(&mut self, config: mina_node::p2p::webrtc::WebrtcConfig) -> &mut Self {
        self.common.p2p_webrtc_config(config);
        self
    }

    pub fn p2p_init(&mut self, secret_key: P2pSecretKey) -> &mut Self {
        self.common.p2p_init(secret_key, P2pTaskSpawner {});
        self
    }

    pub fn p2p_init_with_custom_task_spawner(
        &mut self,
        secret_key: P2pSecretKey,
        task_spawner: impl TaskSpawner,
    ) -> &mut Self {
        self.common.p2p_init(secret_key, task_spawner);
        self
    }

    pub fn gather_stats(&mut self) -> &mut Self {
        self.common.gather_stats();
        self
    }

    pub fn skip_proof_verification(&mut self, skip: bool) -> &mut Self {
        self.common.skip_proof_verification(skip);
        self
    }

    /// Keeps the SNARK worker's setting and included-work log in these
    /// files (P5.4).
    pub fn snark_worker_files_init(
        &mut self,
        files: mina_node_common::snark_worker_files::SnarkWorkerFiles,
    ) -> &mut Self {
        self.common.snark_worker_files_init(files);
        self
    }

    /// Keeps the block producer's history in this file (F-producer).
    pub fn block_production_file_init(
        &mut self,
        file: mina_node_common::block_production_file::BlockProductionFile,
    ) -> &mut Self {
        self.common.block_production_file_init(file);
        self
    }

    pub fn record(&mut self, recorder: Recorder) -> &mut Self {
        self.recorder = recorder;
        self
    }

    pub fn http_server_init(
        &mut self,
        listener: std::net::TcpListener,
        metrics_listener: Option<std::net::TcpListener>,
        metrics_store_root: Option<std::path::PathBuf>,
        options: http_server::HttpServerOptions,
    ) -> std::io::Result<&mut Self> {
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        if let Some(cur_port) = self.http_server_port {
            panic!("trying to start http server on port `{port}`, when it's already running on port `{cur_port}`");
        }
        self.http_server_port = Some(port);
        let rpc_sender = self.rpc_sender();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        thread::Builder::new()
            .name("mina_http_server".to_owned())
            .spawn(move || {
                let result = runtime.block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener)?;
                    http_server::run_on_listener(listener, rpc_sender, options).await
                });
                if let Err(e) = result {
                    tracing::error!("http server error: {e}");
                }
            })?;
        if let Some(metrics_listener) = metrics_listener {
            let rpc_sender = self.rpc_sender();
            thread::Builder::new()
                .name("mina_metrics_server".to_owned())
                .spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    if let Err(error) = runtime.block_on(http_server::metrics::run(
                        metrics_listener,
                        rpc_sender,
                        metrics_store_root,
                    )) {
                        tracing::error!("metrics server error: {error}");
                    }
                })
                .unwrap();
        }
        Ok(self)
    }

    #[cfg(feature = "p2p-webrtc")]
    pub fn webrtc_signal_server_init(&mut self, listener: std::net::TcpListener) -> &mut Self {
        let rpc_sender = self.rpc_sender();
        thread::Builder::new()
            .name("mina_webrtc_signal".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let result = runtime.block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener)?;
                    http_server::signaling::run(listener, rpc_sender).await
                });
                if let Err(error) = result {
                    tracing::error!("WebRTC signaling server error: {error}");
                }
            })
            .unwrap();
        self
    }

    pub fn build(self) -> Result<NodeService, NodeServiceBuildError> {
        let mut service = self.common.build()?;
        service.recorder = self.recorder;
        Ok(service)
    }
}
