use mina_node::{
    core::{channels::mpsc, thread},
    ledger::write::BlockApplyResult,
};
use mina_p2p_messages::v2::{self};
use std::{env, io::Write};

use mina_core::NetworkConfig;
use mina_p2p_messages::v2::PrecomputedBlockData;
use std::net::SocketAddr;

use super::NodeService;

#[cfg(not(target_arch = "wasm32"))]
pub mod aws;
#[cfg(not(target_arch = "wasm32"))]
pub mod delivery;
#[cfg(not(target_arch = "wasm32"))]
pub mod gcp;
#[cfg(not(target_arch = "wasm32"))]
pub mod outbox;
#[cfg(not(target_arch = "wasm32"))]
pub mod rpc;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

pub mod config;

use config::ArchiveStorageOptions;

const MAX_EVENT_COUNT: u64 = 100;
/// How long one delivery may wait for the archive's answer. OCaml's archive
/// answers when its processor takes the diff, after the blocks before it are
/// written, so a busy archive can take a while.
const DEFAULT_SEND_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Environment variable {0} is not set")]
    EnvironmentVariableNotSet(String),
    #[error("Failed to upload block to AWS: {0}")]
    UploadError(String),
    #[error("Archive outbox: {0}")]
    Outbox(String),
}

pub struct ArchiveService {
    archive_sender: mpsc::UnboundedSender<BlockApplyResult>,
}

#[cfg(not(target_arch = "wasm32"))]
struct ArchiveServiceClients {
    archiver_outbox: Option<delivery::SharedOutbox>,
    aws_client: Option<aws::ArchiveAWSClient>,
    gcp_client: Option<gcp::ArchiveGCPClient>,
    local_path: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ArchiveServiceClients {
    async fn new(options: &ArchiveStorageOptions, work_dir: String) -> Result<Self, Error> {
        let aws_client = if options.uses_aws_precomputed_storage() {
            let client = aws::ArchiveAWSClient::new().await?;
            Some(client)
        } else {
            None
        };

        let gcp_client = if options.uses_gcp_precomputed_storage() {
            let client = gcp::ArchiveGCPClient::new().await?;
            Some(client)
        } else {
            None
        };

        let local_path = if options.uses_local_precomputed_storage() {
            let env_path = env::var("MINA_LOCAL_PRECOMPUTED_STORAGE_PATH");
            let default = format!("{}/archive-precomputed", work_dir);
            Some(env_path.unwrap_or(default))
        } else {
            None
        };

        let archiver_outbox = if options.uses_archiver_process() {
            let address =
                std::env::var("MINA_ARCHIVE_ADDRESS").expect("MINA_ARCHIVE_ADDRESS is not set");
            let address = reqwest::Url::parse(&address).expect("Invalid URL");

            // Convert URL to SocketAddr
            let socket_addrs = address.socket_addrs(|| None).expect("Invalid URL");

            let socket_addr = socket_addrs.first().expect("No socket address found");

            Some(start_archiver_outbox(*socket_addr, &work_dir)?)
        } else {
            None
        };

        Ok(Self {
            archiver_outbox,
            aws_client,
            gcp_client,
            local_path,
        })
    }

    pub async fn send_block(&self, breadcrumb: BlockApplyResult, options: &ArchiveStorageOptions) {
        if options.uses_archiver_process() {
            if let Some(outbox) = &self.archiver_outbox {
                Self::handle_archiver_process(&breadcrumb, outbox);
            } else {
                mina_node::core::warn!(summary = "Archiver outbox not set");
            }
        }

        if options.requires_precomputed_block() {
            let network_name = NetworkConfig::global().name;
            let height = breadcrumb.block.height();
            let state_hash = breadcrumb.block.hash();

            let key = format!("{network_name}-{height}-{state_hash}.json");

            mina_node::core::debug!(
                summary = "Uploading precomputed block to archive",
                key = key.clone()
            );

            let precomputed_block = match breadcrumb.precomputed_block(scheduled_time_now()) {
                Ok(block) => block,
                Err(e) => {
                    mina_node::core::warn!(
                        summary = "Failed to convert breadcrumb to precomputed block",
                        error = e
                    );
                    return;
                }
            };

            let data = match PrecomputedBlockData::from(precomputed_block)
                .to_ocaml_json(&mina_node::ledger::write::LedgerHashes)
            {
                Ok(data) => data,
                Err(e) => {
                    mina_node::core::warn!(
                        summary = "Failed to serialize precomputed block",
                        error = e.to_string()
                    );
                    return;
                }
            };

            if options.uses_local_precomputed_storage() {
                if let Some(path) = &self.local_path {
                    let key_clone = key.clone();
                    match write_to_local_storage(path, &key, &data) {
                        Ok(_) => mina_node::core::info!(
                            summary = "Successfully wrote precomputed block to local storage",
                            key = key_clone
                        ),
                        Err(e) => mina_node::core::warn!(
                            summary = "Failed to write precomputed block to local storage",
                            key = key_clone,
                            error = e.to_string()
                        ),
                    }
                } else {
                    mina_node::core::warn!(summary = "Local precomputed storage path not set");
                }
            }

            if options.uses_gcp_precomputed_storage() {
                if let Some(client) = &self.gcp_client {
                    if let Err(e) = client.upload_block(&key, &data).await {
                        mina_node::core::warn!(
                            summary = "Failed to upload precomputed block to GCP",
                            error = e.to_string()
                        );
                    }
                } else {
                    mina_node::core::warn!(summary = "GCP client not initialized");
                }
            }
            if options.uses_aws_precomputed_storage() {
                if let Some(client) = &self.aws_client {
                    if let Err(e) = client.upload_block(&key, &data).await {
                        mina_node::core::warn!(
                            summary = "Failed to upload precomputed block to AWS",
                            error = e.to_string()
                        );
                    }
                } else {
                    mina_node::core::warn!(summary = "AWS client not initialized");
                }
            }
        }
    }

    /// Puts the block's diff in the outbox; the delivery thread sends it.
    fn handle_archiver_process(breadcrumb: &BlockApplyResult, outbox: &delivery::SharedOutbox) {
        let height = breadcrumb.block.height();
        let state_hash = breadcrumb.block.hash().to_string();
        let payload = v2::ArchiveTransitionFrontierDiff::try_from(breadcrumb).and_then(|diff| {
            rpc::encode_payload(&v2::ArchiveRpc::SendDiff(diff)).map_err(|e| e.to_string())
        });
        let payload = match payload {
            Ok(payload) => payload,
            Err(error) => {
                mina_node::core::error!(
                    mina_node::core::log::system_time();
                    summary = "archive diff not built, block not archived",
                    height = height,
                    state_hash = state_hash,
                    error = error,
                );
                return;
            }
        };
        let block = outbox::BlockRef {
            height,
            state_hash: state_hash.clone(),
        };
        let parent_hash = breadcrumb.block.pred_hash().to_string();
        match outbox.append(block, &parent_hash, &payload) {
            Ok(outbox::AppendOutcome::Queued { .. }) => {
                mina_node::core::debug!(
                    mina_node::core::log::system_time();
                    summary = "archive diff queued",
                    height = height,
                    state_hash = state_hash,
                    pending = outbox.pending_len(),
                );
            }
            Ok(outbox::AppendOutcome::AlreadyPending { .. }) => {}
            Err(e) => {
                mina_node::core::error!(
                    mina_node::core::log::system_time();
                    summary = "archive outbox write failed, block not archived",
                    height = height,
                    state_hash = state_hash,
                    error = e.to_string(),
                );
            }
        }
    }
}

/// Sends outbox entries to an archive process over its `Send_archive_diff`
/// RPC.
#[cfg(not(target_arch = "wasm32"))]
pub struct RpcSink {
    pub address: SocketAddr,
    pub timeout: std::time::Duration,
}

#[cfg(not(target_arch = "wasm32"))]
impl delivery::ArchiveSink for RpcSink {
    fn deliver(&mut self, payload: &[u8]) -> Result<(), String> {
        match rpc::send_payload(self.address, payload, self.timeout) {
            Ok(rpc::HandleResult::MessageSent) => Ok(()),
            Ok(_) => Err("archive closed the connection before answering".to_owned()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn env_parse<T: std::str::FromStr>(name: &str) -> Result<Option<T>, Error> {
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map(Some)
            .map_err(|_| Error::Outbox(format!("{name}={value} is not a valid number"))),
        Err(_) => Ok(None),
    }
}

/// Opens the archive outbox (`MINA_ARCHIVE_OUTBOX_DIR`, by default
/// `<work dir>/archive-outbox`) and starts delivering it to `address`.
#[cfg(not(target_arch = "wasm32"))]
fn start_archiver_outbox(
    address: SocketAddr,
    work_dir: &str,
) -> Result<delivery::SharedOutbox, Error> {
    let dir = env::var("MINA_ARCHIVE_OUTBOX_DIR")
        .unwrap_or_else(|_| format!("{work_dir}/archive-outbox"));
    let mut config = outbox::OutboxConfig::new(dir);
    if let Some(max) = env_parse("MINA_ARCHIVE_OUTBOX_MAX_ENTRIES")? {
        config.max_entries = max;
    }
    if let Some(max) = env_parse("MINA_ARCHIVE_OUTBOX_MAX_BYTES")? {
        config.max_bytes = max;
    }
    if let Some(retain) = env_parse("MINA_ARCHIVE_RETAIN_BLOCKS")? {
        config.retain_blocks = retain;
    }
    let timeout = env_parse("MINA_ARCHIVE_SEND_TIMEOUT_SECS")?.unwrap_or(DEFAULT_SEND_TIMEOUT_SECS);

    let opened = outbox::Outbox::open(config.clone()).map_err(|e| Error::Outbox(e.to_string()))?;
    let stats = opened.stats().clone();
    mina_node::core::info!(
        mina_node::core::log::system_time();
        summary = "archive outbox opened",
        dir = config.dir.display().to_string(),
        archive_address = address.to_string(),
        pending = stats.pending_entries,
        pending_bytes = stats.pending_bytes,
        max_entries = config.max_entries,
        max_bytes = config.max_bytes,
        gaps_total = stats.gaps_total,
    );
    let shared = delivery::SharedOutbox::new(opened, Some(address.to_string()));
    shared.register_global();
    shared.spawn_delivery(
        Box::new(RpcSink {
            address,
            timeout: std::time::Duration::from_secs(timeout),
        }),
        delivery::Backoff::default(),
    );
    Ok(shared)
}

impl ArchiveService {
    fn new(archive_sender: mpsc::UnboundedSender<BlockApplyResult>) -> Self {
        Self { archive_sender }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn run(
        mut archive_receiver: mpsc::UnboundedReceiver<BlockApplyResult>,
        options: ArchiveStorageOptions,
        work_dir: String,
    ) {
        let clients = match ArchiveServiceClients::new(&options, work_dir).await {
            Ok(clients) => clients,
            Err(e) => {
                mina_node::core::error!(
                    summary = "Failed to initialize archive service clients",
                    error = e.to_string()
                );
                return;
            }
        };

        while let Some(breadcrumb) = archive_receiver.recv().await {
            clients.send_block(breadcrumb, &options).await;
        }
    }

    // Note: Placeholder for the wasm implementation, if we decide to include an archive mode in the future
    #[cfg(target_arch = "wasm32")]
    fn run(
        mut archive_receiver: mpsc::UnboundedReceiver<BlockApplyResult>,
        options: ArchiveStorageOptions,
        work_dir: String,
    ) {
        unimplemented!()
    }

    pub fn start(options: ArchiveStorageOptions, work_dir: String) -> Self {
        let (archive_sender, archive_receiver) = mpsc::unbounded_channel::<BlockApplyResult>();

        #[cfg(not(target_arch = "wasm32"))]
        Self::start_native(archive_receiver, options, work_dir);

        #[cfg(target_arch = "wasm32")]
        Self::start_wasm(archive_receiver, options, work_dir);

        Self::new(archive_sender)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start_native(
        archive_receiver: mpsc::UnboundedReceiver<BlockApplyResult>,
        options: ArchiveStorageOptions,
        work_dir: String,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        thread::Builder::new()
            .name("mina_archive".to_owned())
            .spawn(move || {
                runtime.block_on(Self::run(archive_receiver, options, work_dir));
            })
            .unwrap();
    }

    #[cfg(target_arch = "wasm32")]
    fn start_wasm(
        archive_receiver: mpsc::UnboundedReceiver<BlockApplyResult>,
        options: ArchiveStorageOptions,
        work_dir: String,
    ) {
        thread::Builder::new()
            .name("mina_archive".to_owned())
            .spawn(move || {
                Self::run(archive_receiver, options, work_dir);
            })
            .unwrap();
    }
}

impl mina_node::transition_frontier::archive::archive_service::ArchiveService for NodeService {
    fn send_to_archive(&mut self, data: BlockApplyResult) {
        if let Some(archive) = self.archive.as_mut() {
            if let Err(e) = archive.archive_sender.send(data) {
                mina_node::core::warn!(
                    summary = "Failed sending diff to archive service",
                    error = e.to_string()
                );
            }
        }
    }
}

// Note: Placeholder for the wasm implementation, if we decide to include an archive mode in the future
#[cfg(target_arch = "wasm32")]
mod rpc {}

/// The time OCaml writes as a precomputed block's `scheduled_time`: now.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_subscriptions.ml#L140-L154>
#[cfg(not(target_arch = "wasm32"))]
fn scheduled_time_now() -> v2::BlockTimeTimeStableV1 {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64);
    v2::BlockTimeTimeStableV1(ms.into())
}

fn write_to_local_storage(base_path: &str, key: &str, data: &[u8]) -> Result<(), Error> {
    use std::{
        fs::{create_dir_all, File},
        path::Path,
    };

    let path = Path::new(base_path).join(key);
    if let Some(parent) = path.parent() {
        create_dir_all(parent)
            .map_err(|e| Error::UploadError(format!("Directory creation failed: {}", e)))?;
    }

    let mut file = File::create(&path)
        .map_err(|e| Error::UploadError(format!("File creation failed: {}", e)))?;

    file.write_all(data)
        .map_err(|e| Error::UploadError(format!("File write failed: {}", e)))?;

    Ok(())
}
