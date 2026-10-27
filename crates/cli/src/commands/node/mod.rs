pub mod status;

#[derive(Debug, clap::Subcommand)]
pub enum NodeCommand {
    /// Inspect a running node using read-only HTTP requests.
    Status(status::Status),
    // The about text is the struct's, which has more than one paragraph.
    FrontierStoreCheck(crate::commands::keys::FrontierStoreCheck),
}

use anyhow::Context;
use clap::{builder::TypedValueParser, error::ErrorKind};
use ledger::proofs::provers::BlockProver;
use mina_node::{
    account::{parse_snarker_fee, parse_snarker_public_key, DEFAULT_SNARKER_FEE},
    core::log::inner::Level,
    p2p::{
        connection::outgoing::P2pConnectionOutgoingInitOpts,
        identity::SecretKey,
        webrtc::{IceServer, UdpPortRange, WebrtcConfig},
    },
    service::Recorder,
    snark::{BlockVerifier, TransactionVerifier},
    transition_frontier::genesis::GenesisConfig,
    SnarkerSetting, SnarkerStrategy,
};
use mina_node_account::AccountPublicKey;
use mina_node_native::{
    archive::config::ArchiveStorageOptions,
    snark_worker_files::{SnarkWorkerFiles, SETTING_FILE},
    tracing::{self, LogFileOptions},
    NodeBuilder,
};
use reqwest::Url;
use std::{
    ffi::OsStr,
    fs::File,
    io::BufReader,
    net::{IpAddr, TcpListener},
    path::PathBuf,
    sync::Arc,
};

fn producer_prover_preload<T>(result: anyhow::Result<T>) -> anyhow::Result<T> {
    result.map_err(|error| {
        let message = format!("block producer provers failed: {error:#}");
        mina_node::core::error!(mina_node::core::log::system_time(); summary = message.clone());
        anyhow::anyhow!(message)
    })
}

fn read_genesis_config(
    path: Option<PathBuf>,
    network: &mina_core::network::NetworkId,
) -> anyhow::Result<Arc<GenesisConfig>> {
    match path {
        Some(path) => {
            let bytes = std::fs::read(&path).with_context(|| format!("{path:?}"))?;
            Ok(Arc::new(
                GenesisConfig::from_bytes(bytes).with_context(|| format!("{path:?}"))?,
            ))
        }
        None => Ok(mina_node::config::default_genesis_config(network)),
    }
}

/// A `--record` mode, surrounding whitespace allowed as before clap checked
/// it.
fn parse_record(value: &str) -> Result<String, String> {
    match value.trim() {
        mode @ ("none" | "state-with-input-actions") => Ok(mode.to_owned()),
        other => Err(format!(
            "unknown --record mode {other:?}: expected none or state-with-input-actions"
        )),
    }
}

fn parse_frontier_store_load_timeout(value: &str) -> Result<u64, String> {
    match value.parse::<u64>() {
        Ok(seconds @ 1..=300) => Ok(seconds),
        _ => Err("--frontier-store-load-timeout must be between 1 and 300 seconds".into()),
    }
}

/// Run a Mina node, or inspect a running one with `mina node status`.
///
/// Without options, the node joins the network given by `--network` as a full
/// node: it finds peers, syncs, then follows the tip of the chain. The HTTP
/// server serves the dashboard, the API and GraphQL on 127.0.0.1:3000.
/// Options add block production, SNARK work and browser peers.
#[derive(Debug, clap::Args)]
pub struct Node {
    #[command(subcommand)]
    pub command: Option<NodeCommand>,

    // Network.
    /// A peer to connect to at startup, as a multiaddr. Repeat it for more
    /// peers.
    ///
    /// The format is `/ip4/<address>/tcp/<port>/p2p/<peer id>`, with `/ip6/`
    /// or `/dns4/` in place of `/ip4/` for other addresses. Peers given here,
    /// or by `--peer-list-file` or `--peer-list-url`, replace the network's
    /// default peers.
    #[arg(
        long,
        short = 'P',
        visible_alias = "peer",
        value_name = "MULTIADDR",
        help_heading = "Network"
    )]
    pub peers: Vec<P2pConnectionOutgoingInitOpts>,

    /// A file of peers to connect to at startup, one multiaddr per line.
    ///
    /// Empty lines are skipped. Lines that don't parse are logged and
    /// skipped.
    #[arg(
        long,
        env = "MINA_PEER_LIST_FILE",
        value_name = "FILE",
        help_heading = "Network"
    )]
    pub peer_list_file: Option<PathBuf>,

    /// A URL that returns peers to connect to at startup, in the
    /// `--peer-list-file` format.
    #[arg(
        long,
        env = "MINA_PEER_LIST_URL",
        value_name = "URL",
        help_heading = "Network"
    )]
    pub peer_list_url: Option<Url>,

    /// Run as a seed node: without peers given, the node waits for peers to
    /// connect instead of dialling the network's default peers.
    #[arg(long, env = "MINA_SEED", help_heading = "Network")]
    pub seed: bool,

    /// The most peer connections the node keeps.
    #[arg(
        long,
        default_value = "100",
        value_name = "N",
        help_heading = "Network"
    )]
    pub max_peers: usize,

    /// The TCP port for libp2p peer connections.
    #[arg(
        long,
        env = "MINA_LIBP2P_PORT",
        visible_alias = "external-port",
        default_value = "8302",
        value_name = "PORT",
        help_heading = "Network"
    )]
    pub libp2p_port: u16,

    /// A public IP address at which peers may reach this node. Repeat it for
    /// more addresses.
    #[arg(long, value_name = "IP", help_heading = "Network")]
    pub libp2p_external_ip: Vec<IpAddr>,

    /// An encrypted libp2p key file, as OCaml's `mina libp2p
    /// generate-keypair` writes it, for the node's peer identity.
    ///
    /// The password is read from the `MINA_LIBP2P_PASS` environment
    /// variable. The file takes precedence over `--p2p-secret-key`.
    #[arg(long, value_name = "FILE", help_heading = "Network")]
    pub libp2p_keypair: Option<String>,

    /// Refused: a password on the command line would be visible to other
    /// processes; it is read from the MINA_LIBP2P_PASS environment
    /// variable instead.
    #[arg(hide = true, value_parser = crate::commands::secret_value::refused_node_password())]
    pub libp2p_password: Option<String>,

    /// The node's libp2p secret key, for its peer identity.
    ///
    /// By default the node reads `p2p-secret-key` in the work directory, and
    /// creates it on the first start.
    #[arg(
        long,
        short = 's',
        env = "MINA_P2P_SEC_KEY",
        hide_env_values = true,
        value_name = "KEY",
        help_heading = "Network",
        value_parser = crate::commands::secret_value::p2p_key()
    )]
    pub p2p_secret_key: Option<SecretKey>,

    // HTTP and GraphQL.
    /// The TCP port of the HTTP server: the dashboard, the API and GraphQL.
    #[arg(
        long,
        short,
        env = "MINA_PORT",
        default_value = "3000",
        value_name = "PORT",
        help_heading = "HTTP and GraphQL"
    )]
    pub port: u16,

    /// The address the HTTP server listens on. `0.0.0.0` listens on all IPv4
    /// interfaces.
    #[arg(
        long,
        env = "MINA_HTTP_ADDR",
        default_value = "127.0.0.1",
        value_name = "ADDR",
        help_heading = "HTTP and GraphQL"
    )]
    pub http_addr: IpAddr,

    /// A page origin, like `https://dash.example.org`, whose pages may read
    /// this node's HTTP API besides pages of this machine.
    ///
    /// Pages of this machine are `localhost`, `127.0.0.1` and `[::1]` on any
    /// port. Repeat the option, or give a comma-separated list in the
    /// environment; `*` lets every page read the API. Opening the node's
    /// pages at a host name rather than an IP address or `localhost` also
    /// needs that name listed, as `http://name:port`.
    #[arg(
        long = "http-cors-origin",
        env = "MINA_HTTP_CORS_ORIGINS",
        value_delimiter = ',',
        value_name = "ORIGIN",
        help_heading = "HTTP and GraphQL"
    )]
    pub http_cors_origins: Vec<String>,

    /// Accept transactions over HTTP: `POST /send-payment` and the GraphQL
    /// mutations `sendPayment`, `sendDelegation` and `sendZkapp`.
    ///
    /// Off by default, so that a public node does not take submissions from
    /// anyone who can reach its port. When off, they get HTTP 403 or a
    /// GraphQL error. Turn it on only for a node whose HTTP port only you can
    /// reach.
    #[arg(
        long,
        env = "MINA_ENABLE_TX_SUBMISSION",
        default_value_t = false,
        help_heading = "HTTP and GraphQL"
    )]
    pub enable_tx_submission: bool,

    /// Answer only a read-only GraphQL subset, for a node whose HTTP port
    /// anyone can reach.
    ///
    /// The subset takes one query per request (no mutations, batches or
    /// introspection), with the fields block explorers, o1js and wallets
    /// read, `bestChain` with `maxLength` up to 20, and a cost ceiling. It
    /// leaves out the peers, SNARK pool, operator and telemetry fields.
    /// Refused requests get HTTP 400 before any work is done. The dashboard
    /// and GraphiQL are not served.
    #[arg(
        long,
        env = "MINA_PUBLIC_GRAPHQL",
        default_value_t = false,
        help_heading = "HTTP and GraphQL"
    )]
    pub public_graphql: bool,

    /// Serve whole-ledger exports over HTTP, for `mina ledger export` and
    /// `mina fork export`. Off by default: an export is expensive, so
    /// restrict access to the HTTP port first.
    #[arg(
        long,
        env = "MINA_ENABLE_LEDGER_EXPORT",
        default_value_t = false,
        help_heading = "HTTP and GraphQL"
    )]
    pub enable_ledger_export: bool,

    /// Don't serve the dashboard at `/`. The API routes keep their paths.
    #[arg(
        long,
        env = "MINA_NO_DASHBOARD",
        default_value_t = false,
        help_heading = "HTTP and GraphQL"
    )]
    pub no_dashboard: bool,

    /// Serve Prometheus metrics on this separate TCP port. Off by default.
    #[arg(
        long,
        env = "MINA_METRICS_PORT",
        value_name = "PORT",
        help_heading = "HTTP and GraphQL"
    )]
    pub metrics_port: Option<u16>,

    /// The address the metrics listener listens on.
    #[arg(
        long,
        env = "MINA_METRICS_ADDR",
        default_value = "127.0.0.1",
        value_name = "ADDR",
        help_heading = "HTTP and GraphQL"
    )]
    pub metrics_addr: IpAddr,

    // Storage.
    /// The work directory: the node's chain state, caches, logs and settings.
    #[arg(
        long,
        short = 'd',
        default_value = "~/.mina",
        env = "MINA_HOME",
        value_name = "DIR",
        help_heading = "Storage"
    )]
    pub work_dir: String,

    /// Don't keep a frontier store. The node keeps one by default, from
    /// which a restart resumes instead of syncing from the start.
    ///
    /// The store holds the best chain's blocks as they come, and a checkpoint
    /// of the root's ledgers and staged ledger every `--checkpoint-interval`
    /// seconds. A restart restores from it when it passes its checks and
    /// isn't too old; otherwise the node bootstraps and logs why.
    #[arg(long, env = "MINA_NO_FRONTIER_STORE", help_heading = "Storage")]
    pub no_frontier_store: bool,

    /// Hidden older form of `--no-frontier-store`: `--frontier-store=false`
    /// or `MINA_FRONTIER_STORE=false` turns the store off.
    #[arg(
        long,
        env = "MINA_FRONTIER_STORE",
        default_value_t = true,
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "true",
        action = clap::ArgAction::Set,
        value_parser = clap::builder::BoolishValueParser::new(),
        hide = true
    )]
    pub frontier_store: bool,

    /// Write the frontier store, but don't restore from it at startup.
    #[arg(long, help_heading = "Storage")]
    pub frontier_store_no_restore: bool,

    /// The directory for frontier stores, one per chain, at
    /// `<DIR>/<chain id>/frontier`. Defaults to the work directory.
    #[arg(
        long,
        env = "MINA_FRONTIER_STORE_DIR",
        value_name = "DIR",
        help_heading = "Storage"
    )]
    pub frontier_store_dir: Option<String>,

    /// Delete the frontier store when the node opens it, so that the node
    /// bootstraps.
    #[arg(long, help_heading = "Storage")]
    pub frontier_store_reset: bool,

    /// Seconds a frontier store restore may go without progress before the
    /// node gives it up and bootstraps (1 to 300).
    #[arg(
        long,
        default_value_t = 60,
        value_parser = parse_frontier_store_load_timeout,
        value_name = "SECONDS",
        help_heading = "Storage"
    )]
    pub frontier_store_load_timeout: u64,

    /// Seconds between checkpoints of the frontier store's root.
    #[arg(
        long,
        env = "MINA_CHECKPOINT_INTERVAL",
        default_value_t = 1800,
        value_name = "SECONDS",
        help_heading = "Storage"
    )]
    pub checkpoint_interval: u64,

    /// Keep retained proofs in memory rather than in files under
    /// `<work dir>/proof-cache`. Takes precedence over `--proof-cache`.
    ///
    /// Proofs go to the files by default, except on a block producer
    /// (`--producer-key`), which keeps the proofs it may read to build and
    /// prove a block in memory unless `--proof-cache` is given.
    #[arg(long, env = "MINA_NO_PROOF_CACHE", help_heading = "Storage")]
    pub no_proof_cache: bool,

    /// Keep retained proofs in files under `<work dir>/proof-cache` on a
    /// block producer too. A failed read can then cost it a block.
    #[arg(long, help_heading = "Storage")]
    pub proof_cache: bool,

    /// Keep the staking, next epoch and root snarked ledgers' accounts in
    /// memory rather than in files under `<work dir>/ledger-accounts`.
    #[arg(long, env = "MINA_NO_LEDGER_DISK", help_heading = "Storage")]
    pub no_ledger_disk: bool,

    /// Keep the root snarked ledger's accounts in memory, while the epoch
    /// ledgers' go to files. No effect with `--no-ledger-disk`.
    #[arg(long, env = "MINA_NO_ROOT_LEDGER_DISK", help_heading = "Storage")]
    pub no_root_ledger_disk: bool,

    /// Write a ledger on disk again once this many of its accounts changed,
    /// instead of a sixteenth of them (at least 4,096). For tests: 1 writes
    /// the root snarked ledger again at every root move that changes it, 0 at
    /// every commit.
    #[arg(long, env = "MINA_LEDGER_REFREEZE_SLOTS", hide = true)]
    pub ledger_refreeze_slots: Option<u64>,

    // Block production.
    /// Produce blocks with the account key in this encrypted key file.
    ///
    /// The password is read from the `MINA_PRIVKEY_PASS` environment
    /// variable; unset means an empty password.
    #[arg(
        long,
        env = "MINA_PRODUCER_KEY",
        visible_alias = "block-producer-key",
        group = "producer",
        value_name = "FILE",
        help_heading = "Block production"
    )]
    pub producer_key: Option<PathBuf>,

    /// Refused: the producer key's password is read from the
    /// MINA_PRIVKEY_PASS environment variable; on the command line it
    /// would be visible to other processes for as long as the node runs.
    #[arg(hide = true, value_parser = crate::commands::secret_value::refused_node_password())]
    pub producer_key_password: Option<String>,

    /// Refused as the positionals are: `--password` and the other names an
    /// operator may try (RC7-FIXES).
    #[arg(
        long = "password",
        hide = true,
        aliases = [
            "pass",
            "privkey-pass",
            "privkey-password",
            "libp2p-pass",
            "libp2p-password",
        ],
        num_args = 0..=1,
        default_missing_value = "",
        value_parser = crate::commands::secret_value::refused_node_password()
    )]
    pub password_flag: Option<String>,

    /// The public key that receives the coinbase of the blocks this node
    /// produces. Defaults to the producer's own key.
    ///
    /// If the key is a zkApp account's, its receive permission must be None.
    #[arg(
        long,
        requires = "producer",
        value_name = "PUBLIC_KEY",
        help_heading = "Block production"
    )]
    pub coinbase_receiver: Option<AccountPublicKey>,

    /// The least milliseconds after the start of its slot before the node
    /// publishes a block it produced (at least 1000).
    #[arg(
        long,
        default_value_t = 1_000,
        requires = "producer",
        value_parser = clap::value_parser!(u64).range(1_000..),
        value_name = "MILLISECONDS",
        help_heading = "Block production"
    )]
    pub block_inject_delay_ms: u64,

    // SNARK work.
    /// Run the SNARK worker, with fees paid to this public key (B62...).
    ///
    /// As with OCaml's `--run-snark-worker`, the worker needs no private key:
    /// the public key only goes into the message its proofs commit to. If the
    /// key is a zkApp account's, its receive permission must be None, or
    /// OCaml nodes reject the work.
    #[arg(
        long,
        env = "MINA_RUN_SNARKER",
        visible_alias = "run-snark-worker",
        group = "snarker",
        hide_env_values = true,
        value_name = "PUBLIC_KEY",
        help_heading = "SNARK work",
        value_parser = SnarkerValueParser(parse_snarker_public_key)
    )]
    pub run_snarker: Option<AccountPublicKey>,

    /// The fee asked for each SNARK work bundle, in MINA (for example
    /// `0.1`), as OCaml's `--snark-worker-fee`.
    ///
    /// If the worker's key has no account yet, OCaml nodes only accept work
    /// whose fee is at least the account creation fee (1 MINA).
    #[arg(
        long,
        env = "MINA_SNARKER_FEE",
        visible_alias = "snark-worker-fee",
        hide_env_values = true,
        default_value = DEFAULT_SNARKER_FEE,
        requires = "snarker",
        value_name = "MINA",
        help_heading = "SNARK work",
        value_parser = SnarkerValueParser(parse_snarker_fee)
    )]
    pub snarker_fee: u64,

    /// How the SNARK worker picks its next job: `rand` picks any of the jobs
    /// it may take at random, as OCaml's default `--work-selection`, and
    /// `seq` the first in scan-state order.
    ///
    /// It applies to a worker started with `--run-snarker`, from the
    /// dashboard, or from the setting the dashboard saved.
    #[arg(
        long,
        env = "MINA_SNARKER_STRATEGY",
        default_value = "rand",
        value_name = "STRATEGY",
        help_heading = "SNARK work"
    )]
    pub snarker_strategy: SnarkerStrategy,

    /// Seconds before a won slot in which a node that also produces blocks
    /// starts no new SNARK job (0 to 4200; 0 turns it off). No effect
    /// without `--producer-key`.
    #[arg(long, env = "MINA_SNARKER_BLOCK_FIRST_LEAD", default_value_t = 420,
        value_parser = clap::value_parser!(u64).range(0..=4_200),
        value_name = "SECONDS", help_heading = "SNARK work")]
    pub snarker_block_first_lead: u64,

    /// Let the dashboard start, stop and re-price the SNARK worker
    /// (`POST /snarker/control`). Off by default.
    ///
    /// The worker's settings decide where fees go, so allow this only on a
    /// node whose HTTP port only you can reach. It can't be combined with
    /// `--public-graphql`. A worker started from the dashboard is saved in
    /// `<work dir>/snark-worker.json` and comes back after a restart, with
    /// or without this option; `--run-snarker` takes precedence over it.
    /// Stopping the worker from the dashboard deletes the file, as does
    /// deleting it while the node is stopped.
    #[arg(
        long,
        env = "MINA_ALLOW_WORKER_CONTROL",
        default_value_t = false,
        conflicts_with = "public_graphql",
        help_heading = "SNARK work"
    )]
    pub allow_worker_control: bool,

    /// A page origin, like `https://dash.example.org`, that may change the
    /// SNARK worker besides pages of this machine.
    ///
    /// Pages of this machine are `localhost`, `127.0.0.1` and `[::1]` on any
    /// port. Repeat the option, or give a comma-separated list in the
    /// environment. Requests from any other page are refused.
    #[arg(
        long = "worker-control-origin",
        env = "MINA_WORKER_CONTROL_ORIGINS",
        value_delimiter = ',',
        requires = "allow_worker_control",
        value_name = "ORIGIN",
        help_heading = "SNARK work"
    )]
    pub worker_control_origins: Vec<String>,

    // WebRTC.
    /// Serve WebRTC signaling for browser peers on this separate TCP port.
    /// Off by default.
    #[arg(
        long,
        env = "MINA_WEBRTC_SIGNAL_PORT",
        value_name = "PORT",
        help_heading = "WebRTC"
    )]
    pub webrtc_signal_port: Option<u16>,

    /// The address the signaling listener listens on. Pages served over
    /// HTTPS need a TLS proxy in front of it.
    #[arg(
        long,
        env = "MINA_WEBRTC_SIGNAL_ADDR",
        default_value = "127.0.0.1",
        value_name = "ADDR",
        help_heading = "WebRTC"
    )]
    pub webrtc_signal_addr: IpAddr,

    /// A JSON file of ICE servers for WebRTC connections, in the browser's
    /// `RTCIceServer` list format. Defaults to the built-in STUN servers.
    ///
    /// For example `[{"urls": "stun:stun.example.org:3478"}]`, or `[]` for
    /// none. A node with a public address needs none. A file keeps TURN
    /// credentials off the command line.
    #[arg(
        long,
        env = "MINA_WEBRTC_ICE_SERVERS",
        value_name = "FILE",
        help_heading = "WebRTC"
    )]
    pub webrtc_ice_servers: Option<PathBuf>,

    /// The UDP ports for WebRTC candidates, as `MIN-MAX`, so that a firewall
    /// need open only those. Defaults to any port.
    ///
    /// Each connection takes a port per local address: allow several per
    /// `--max-peers`.
    #[arg(
        long,
        env = "MINA_WEBRTC_UDP_PORTS",
        value_name = "MIN-MAX",
        help_heading = "WebRTC"
    )]
    pub webrtc_udp_ports: Option<UdpPortRange>,

    /// The public address of a 1:1 NAT in front of this node, used in its
    /// WebRTC host candidates in place of its interface addresses.
    ///
    /// For a cloud host whose interface has a private address. Repeat it, or
    /// separate addresses with commas, for IPv4 and IPv6.
    #[arg(
        long,
        env = "MINA_WEBRTC_NAT_IP",
        value_delimiter = ',',
        value_name = "IP",
        help_heading = "WebRTC"
    )]
    pub webrtc_nat_ip: Vec<IpAddr>,

    // Logging.
    /// The log level of the console and the log file: `error`, `warn`,
    /// `info`, `debug` or `trace`.
    ///
    /// `info` logs what an operator follows: the settings at startup, sync
    /// progress, each new best tip, a status line with the peer count, the
    /// blocks produced, the SNARK work sold, warnings and errors. `debug` adds
    /// detail for diagnosis, and `trace` adds every action of the state
    /// machine.
    #[arg(
        long,
        short,
        env = "MINA_VERBOSITY",
        visible_alias = "log-level",
        default_value = "info",
        value_name = "LEVEL",
        help_heading = "Logging"
    )]
    pub verbosity: Level,

    /// Log to the console only, without `mina.log`.
    #[arg(
        long,
        env = "MINA_DISABLE_FILESYSTEM_LOGGING",
        default_value_t = false,
        help_heading = "Logging"
    )]
    pub disable_filesystem_logging: bool,

    /// The directory for the log file, `mina.log`. Defaults to the work
    /// directory.
    #[arg(
        long,
        env = "MINA_LOG_PATH",
        default_value = "$MINA_HOME",
        hide_default_value = true,
        value_name = "DIR",
        help_heading = "Logging"
    )]
    pub log_path: String,

    /// The size in MiB at which `mina.log` is rotated to `mina.log.1`, older
    /// files shifting up one.
    #[arg(
        long,
        env = "MINA_LOG_FILE_SIZE_MB",
        default_value_t = LogFileOptions::DEFAULT_MAX_FILE_MB,
        value_parser = clap::value_parser!(u64).range(1..=1 << 20),
        value_name = "MIB",
        help_heading = "Logging"
    )]
    pub log_file_size_mb: u64,

    /// The number of log files kept, `mina.log` included; the oldest is
    /// deleted. The logs take at most `--log-file-size-mb` times this.
    #[arg(
        long,
        env = "MINA_LOG_FILE_COUNT",
        default_value_t = LogFileOptions::DEFAULT_MAX_FILES,
        value_parser = clap::value_parser!(u64).range(1..=1000).map(|v| v as usize),
        value_name = "N",
        help_heading = "Logging"
    )]
    pub log_file_count: usize,

    // Advanced.
    /// A daemon config JSON file to start from instead of the network's
    /// embedded genesis, as OCaml's `--config-file`.
    #[arg(
        short = 'c',
        long,
        env = "MINA_CONFIG",
        value_name = "FILE",
        help_heading = "Advanced"
    )]
    pub config: Option<PathBuf>,

    /// A genesis config to start from instead of `--config`'s.
    ///
    /// It is either a prebuilt config as `ledger-tool` writes it (bin_prot),
    /// or one written with `--without-ledgers` (JSON). The latter only suits
    /// a network that has moved past its genesis, as the ledgers are synced
    /// from peers.
    #[arg(
        long,
        env = "MINA_GENESIS_CONFIG",
        conflicts_with = "config",
        value_name = "FILE",
        help_heading = "Advanced"
    )]
    pub genesis_config: Option<PathBuf>,

    /// Turn off peer discovery: no Kademlia, and no asking peers for more
    /// peers.
    ///
    /// The node still dials its initial peers. Those are the network's
    /// default peers unless peers are given with `--peers`,
    /// `--peer-list-file` or `--peer-list-url`, or `--seed` is set.
    #[arg(long, help_heading = "Advanced")]
    pub no_peers_discovery: bool,

    // Development only: hidden from the help, still accepted.
    /// Record the node's initial state and input actions to
    /// `<work dir>/recorder`, for `mina replay`. For development.
    #[arg(
        long,
        default_value = "none",
        env = "MINA_RECORD",
        hide = true,
        value_parser = parse_record
    )]
    pub record: String,

    /// A fixed seed for the node's random number generator, as 64 hex
    /// digits. For tests.
    #[arg(long, env = "MINA_RNG_SEED", hide = true)]
    pub rng_seed: Option<String>,

    /// Override the chain ID used for P2P networking. For interop tests.
    #[arg(long, env = "MINA_CHAIN_ID", hide = true)]
    pub chain_id: Option<String>,

    /// Skip SNARK proof verification. For interop tests with OCaml nodes run
    /// with `--proof-level none`, whose blocks carry dummy proofs. Never on a
    /// public network.
    #[arg(long, hide = true)]
    pub skip_proof_verification: bool,

    /// Write each block the node applies as a precomputed block JSON file in
    /// `MINA_LOCAL_PRECOMPUTED_STORAGE_PATH`, by default
    /// `<work dir>/archive-precomputed`.
    #[arg(long, env = "MINA_ARCHIVE_LOCAL_STORAGE", hide = true)]
    pub archive_local_storage: bool,

    /// Send block diffs to an OCaml archive process at
    /// `MINA_ARCHIVE_ADDRESS`.
    ///
    /// Diffs wait in a durable outbox until the archive acknowledges them,
    /// retried with back-off; `GET /archive/status` reports it. Optional:
    /// `MINA_ARCHIVE_OUTBOX_DIR` (default `<work dir>/archive-outbox`),
    /// `MINA_ARCHIVE_OUTBOX_MAX_ENTRIES` (default 20000) and
    /// `MINA_ARCHIVE_OUTBOX_MAX_BYTES` (default 4 GiB), beyond which the
    /// oldest entries are dropped and recorded as gaps,
    /// `MINA_ARCHIVE_RETAIN_BLOCKS` (default 870), the acknowledged diffs
    /// kept for replay at startup and `POST /archive/resend`, and
    /// `MINA_ARCHIVE_SEND_TIMEOUT_SECS` (default 120).
    #[arg(long, env = "MINA_ARCHIVE_ARCHIVER_PROCESS", hide = true)]
    pub archive_archiver_process: bool,

    /// Upload precomputed blocks to Google Cloud Storage, with
    /// `GCP_CREDENTIALS_JSON` and `GCP_BUCKET_NAME`.
    #[arg(long, env = "MINA_ARCHIVE_GCP_STORAGE", hide = true)]
    pub archive_gcp_storage: bool,

    /// Upload precomputed blocks to AWS S3, with `AWS_ACCESS_KEY_ID`,
    /// `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_DEFAULT_REGION` and
    /// `MINA_AWS_BUCKET_NAME`.
    #[arg(long, env = "MINA_ARCHIVE_AWS_STORAGE", hide = true)]
    pub archive_aws_storage: bool,
}

impl Node {
    /// What the HTTP server allows and serves, from the flags.
    fn http_server_options(
        &self,
    ) -> anyhow::Result<mina_node_native::http_server::HttpServerOptions> {
        let worker_control_origins =
            mina_node_native::http_server::control_origin::ControlOrigins::new(
                self.worker_control_origins.clone(),
            )
            .map_err(|error| anyhow::anyhow!("--worker-control-origin: {error}"))?;
        let read_origins = mina_node_native::http_server::control_origin::ReadOrigins::new(
            self.http_cors_origins.clone(),
        )
        .map_err(|error| anyhow::anyhow!("--http-cors-origin: {error}"))?;
        Ok(mina_node_native::http_server::HttpServerOptions {
            tx_submission: self.enable_tx_submission,
            public_graphql: self.public_graphql,
            ledger_export: self.enable_ledger_export,
            worker_control: self.allow_worker_control && !self.public_graphql,
            worker_control_origins,
            read_origins,
            dashboard: !self.no_dashboard,
            // For the dashboard's bug report (`POST /report`).
            work_dir: shellexpand::full(&self.work_dir)
                .ok()
                .map(|dir| std::path::PathBuf::from(dir.into_owned())),
        })
    }

    /// Runs the Mina node with the configured options.
    ///
    /// Starts the node and blocks until shutdown. Builds [`BlockVerifier`] and
    /// [`TransactionVerifier`] at startup for SNARK proof verification. The
    /// node behavior depends on the struct fields: a basic node syncs the
    /// chain, while optional features like block production
    /// ([`producer_key`](Self::producer_key)), SNARK worker
    /// ([`run_snarker`](Self::run_snarker)), or archiving
    /// ([`archive_local_storage`](Self::archive_local_storage)) are enabled
    /// via their respective fields.
    ///
    /// # Panics
    ///
    /// Panics if the Tokio runtime cannot be built.
    ///
    /// # Notes
    ///
    /// Building SNARK verification keys at startup may take several seconds.
    /// Optional services are initialized based on their respective CLI flags.
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails (config loading, key
    /// decryption, node building, etc.).
    pub fn run(self) -> anyhow::Result<()> {
        match self.command {
            Some(NodeCommand::Status(status)) => return status.run(),
            Some(NodeCommand::FrontierStoreCheck(check)) => return check.run(),
            None => {}
        }
        // Passwords come from the environment: the command-line positionals
        // that used to take them are refused during parsing. Read them before
        // binding ports or opening log files, so a bad value fails cleanly.
        // An unset MINA_PRIVKEY_PASS means empty, as the old default was.
        let producer_key_password =
            crate::commands::secret_value::password_from_env("MINA_PRIVKEY_PASS")?;
        let libp2p_password =
            crate::commands::secret_value::optional_password_from_env("MINA_LIBP2P_PASS")?;
        debug_assert!(
            self.producer_key_password.is_none()
                && self.libp2p_password.is_none()
                && self.password_flag.is_none()
        );
        // Before any flag is moved out of `self`.
        let http_server_options = self.http_server_options()?;
        let (http_listener, webrtc_signal_listener) = bind_http_listeners(
            self.http_addr,
            self.port,
            self.webrtc_signal_addr,
            self.webrtc_signal_port,
        )?;
        // Initialize logging and thread pool
        let work_dir = shellexpand::full(&self.work_dir).unwrap().into_owned();
        let block_producer_enabled = self.producer_key.is_some();
        // Own the accounts directory before touching any existing node files.
        // Memory mode also removes stale accounts and holds the lock, so a
        // second node cannot empty a live node's directory in either mode.
        let ledger_accounts_dir = std::path::Path::new(&work_dir).join("ledger-accounts");
        let ledger_disk_error = match ledger::database::disk::configure_global_dir(
            &ledger_accounts_dir,
            !self.no_ledger_disk,
        ) {
            Ok(()) => None,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(error).context("the work directory is already used by another node");
            }
            Err(error) => Some(error),
        };

        let log_file = (!self.disable_filesystem_logging).then(|| LogFileOptions {
            dir: if self.log_path == "$MINA_HOME" {
                work_dir.clone()
            } else {
                shellexpand::full(&self.log_path)
                    .map(|path| path.into_owned())
                    .unwrap_or_else(|_| self.log_path.clone())
            }
            .into(),
            max_file_bytes: self.log_file_size_mb << 20,
            max_files: self.log_file_count,
        });
        // The file logger's guard: dropping it drains the logger's thread.
        // It is dropped at the end of this scope, and by an unreadable ledger
        // record's exit, which would otherwise end the process first.
        let log_guard = DropAtEnd(std::sync::Arc::new(std::sync::Mutex::new(
            match &log_file {
                Some(options) => {
                    tracing::initialize_with_filesystem_output(self.verbosity, options.clone())
                }
                None => {
                    tracing::initialize(self.verbosity);
                    None
                }
            },
        )));
        let file_logging = log_guard.0.lock().is_ok_and(|guard| guard.is_some());
        if let Some(error) = ledger_disk_error {
            mina_node::core::warn!(
                mina_node::core::log::system_time();
                kind = "LedgerDiskUnavailable",
                summary = "the ledger accounts directory could not be prepared; ledgers stay in memory",
                dir = ledger_accounts_dir.display().to_string(),
                error = error.to_string(),
            );
        }

        // Resolve the saved worker before recording the effective startup
        // settings. Keep the crash recorder ahead of node initialization.
        let worker_files = SnarkWorkerFiles::new(&work_dir);
        let saved_worker = worker_files.load_setting();
        let flags = self.run_snarker.clone().map(|public_key| SnarkerSetting {
            public_key,
            fee: self.snarker_fee,
        });
        let (worker, origin) = startup_worker(flags, saved_worker.as_ref().ok().cloned().flatten());
        let frontier_store = self.frontier_store_enabled()?;
        let proof_cache_setting = self.proof_cache_setting()?;

        // For `mina report`: how the node was started, and a crash file for
        // each panic from now on.
        {
            let p2p_secret_key = self.p2p_secret_key.as_ref().map(|key| key.to_string());
            let producer_key = self.producer_key.as_ref().map(|p| p.display().to_string());
            let frontier_store_dir = frontier_store.then(|| {
                self.frontier_store_dir
                    .as_deref()
                    .and_then(|dir| shellexpand::full(dir).ok())
                    .map_or_else(|| work_dir.clone(), |dir| dir.into_owned())
            });
            crate::commands::report::on_node_start(crate::commands::report::StartInfo {
                work_dir: std::path::Path::new(&work_dir),
                http_port: self.port,
                log_dir: log_file.as_ref().map(|file| file.dir.to_str()).flatten(),
                frontier_store_dir: frontier_store_dir.as_deref(),
                snark_worker: worker.as_ref().map(|setting| {
                    crate::commands::report::crash::StartWorker {
                        public_key: setting.public_key.to_string(),
                        fee: setting.fee,
                        source: origin.summary().to_owned(),
                    }
                }),
                secrets: [
                    Some(producer_key_password.as_str()),
                    libp2p_password.as_deref(),
                    self.libp2p_keypair.as_deref(),
                    p2p_secret_key.as_deref(),
                    producer_key.as_deref(),
                ]
                .into_iter()
                .flatten()
                .collect(),
            });
        }

        rayon::ThreadPoolBuilder::new()
            .num_threads(num_cpus::get().max(2) - 1)
            .thread_name(|i| format!("mina_rayon_{i}"))
            .build_global()
            .context("failed to initialize threadpool")?;

        // Before any field moves out of `self`. A bad file stops the node
        // here, before it starts anything.
        let webrtc_config = webrtc_config(
            self.webrtc_ice_servers.as_deref(),
            self.webrtc_udp_ports,
            &self.webrtc_nat_ip,
        )?;

        // A node started with a daemon JSON and the frontier store, which
        // it restores from, keeps that config's genesis without the ledgers
        // in its work directory (I5d2). The next start with the same file and
        // build takes it from there and loads the genesis ledgers only if it
        // bootstraps: the restore replaces them.
        let genesis_cache = match &self.config {
            Some(config) if frontier_store && !self.frontier_store_no_restore => {
                let started = std::time::Instant::now();
                let build = mina_node::BuildEnv::get();
                let build = format!("{} {} {}", build.version, build.git.commit_hash, build.time);
                let cache = mina_node::transition_frontier::genesis::GenesisCache::for_config(
                    &std::path::Path::new(&work_dir).join("genesis"),
                    config,
                    &build,
                )
                .with_context(|| format!("config file {config:?}"))?;
                mina_node::core::debug!(
                    mina_node::core::log::system_time();
                    kind = "GenesisCache",
                    summary = "hashed the config file",
                    path = cache.path().display().to_string(),
                    elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                );
                Some(cache)
            }
            _ => None,
        };
        let cached_genesis = genesis_cache.as_ref().and_then(|cache| match cache.read() {
            Ok(loaded) => loaded,
            Err(error) => {
                mina_node::core::warn!(
                    mina_node::core::log::system_time();
                    kind = "GenesisCache",
                    summary = "the kept genesis is unusable: dropped, the config is loaded",
                    path = cache.path().display().to_string(),
                    error = error,
                );
                let _ = cache.remove();
                None
            }
        });
        let mut genesis_ledgers_deferred = None;

        // Load configuration
        let (daemon_conf, genesis_conf) = match (self.config, self.genesis_config) {
            (_, Some(path)) => (
                mina_node::daemon_json::Daemon::DEFAULT,
                read_genesis_config(Some(path), &mina_core::NetworkConfig::global().network_id)?,
            ),
            (Some(config), None) if cached_genesis.is_some() => {
                let loaded = cached_genesis.expect("checked");
                mina_node::core::info!(
                    mina_node::core::log::system_time();
                    kind = "GenesisCache",
                    summary = "the genesis is the one kept for this config: its ledgers are loaded only if the node bootstraps",
                    genesis_ledger_hash = loaded.genesis_ledger_hash.to_string(),
                );
                genesis_ledgers_deferred = Some(Arc::new(GenesisConfig::DaemonJsonFile(config)));
                (
                    loaded
                        .runtime_config
                        .daemon
                        .clone()
                        .unwrap_or(mina_node::daemon_json::Daemon::DEFAULT),
                    Arc::new(GenesisConfig::WithoutLedgers(Box::new(loaded))),
                )
            }
            (Some(config), None) => {
                // Buffered: serde_json reads a raw `File` about a byte per
                // syscall (696 s for the 736 MB mainnet config, R31).
                let reader = BufReader::with_capacity(
                    1 << 20,
                    File::open(config).context("config file {config:?}")?,
                );
                let config: mina_node::daemon_json::DaemonJson =
                    serde_json::from_reader(reader).context("config file {config:?}")?;
                (
                    config
                        .daemon
                        .clone()
                        .unwrap_or(mina_node::daemon_json::Daemon::DEFAULT),
                    Arc::new(GenesisConfig::DaemonJson(Box::new(config))),
                )
            }
            (None, None) => (
                mina_node::daemon_json::Daemon::DEFAULT,
                read_genesis_config(None, &mina_core::NetworkConfig::global().network_id)?,
            ),
        };

        // Apply runtime constants before any builder API can start a service
        // or construct a prover using the compiled network defaults.
        genesis_conf.initialize_runtime()?;
        // A fork other than the release's has its own blockchain circuits,
        // verification key and chain-id digest: load them, or refuse to run.
        if let Some(fork) = genesis_conf.prepare_runtime_fork()? {
            mina_node::core::info!(mina_node::core::log::system_time();
                kind = "RuntimeForkCircuits",
                summary = "using the runtime fork's blockchain circuits",
                dir = fork.dir.display().to_string(),
                step = fork.step_blockchain_gates.clone(),
                wrap = fork.wrap_blockchain_gates.clone(),
                blockchain_step_digest = hex::encode(fork.blockchain_step_digest),
            );
        }

        // Parse chain ID override (applied to builder below).
        let chain_id_override = self
            .chain_id
            .as_deref()
            .map(mina_node::core::ChainId::from_hex)
            .transpose()
            .context("invalid --chain-id hex string")?;

        let custom_rng_seed = match self.rng_seed {
            None => None,
            Some(v) => match hex::decode(v)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| {
                    <[u8; 32]>::try_from(bytes.as_slice()).map_err(anyhow::Error::from)
                }) {
                Ok(v) => Some(v),
                Err(err) => {
                    mina_node::core::error!(
                        mina_node::core::log::system_time();
                        summary = "bad rng seed",
                        err = err.to_string(),
                    );
                    return Err(err);
                }
            },
        };

        let mut node_builder: NodeBuilder =
            NodeBuilder::new(custom_rng_seed, daemon_conf, genesis_conf);
        if let Some(cache) = genesis_cache {
            node_builder.genesis_cache(cache);
        }
        if let Some(full) = genesis_ledgers_deferred {
            node_builder.genesis_ledgers_deferred(full);
        }

        if let Some(chain_id) = chain_id_override {
            node_builder.chain_id_override(chain_id);
        }
        if self.skip_proof_verification {
            node_builder.skip_proof_verification(true);
        }

        // Configure P2P identity
        if let Some(sec_key) = self.p2p_secret_key {
            node_builder.p2p_sec_key(sec_key);
        }

        // Load libp2p keypair from file (overrides MINA_P2P_SEC_KEY)
        if let (Some(key_file), Some(password)) = (&self.libp2p_keypair, &libp2p_password) {
            match SecretKey::from_encrypted_file(key_file, password) {
                Ok(sk) => {
                    node_builder.p2p_sec_key(sk.clone());
                    mina_node::core::info!(
                        mina_node::core::log::system_time();
                        summary = "read sercret key from file",
                        file_name = key_file,
                        pk = sk.public_key().to_string(),
                    )
                }
                Err(err) => {
                    mina_node::core::error!(
                        mina_node::core::log::system_time();
                        summary = "failed to read secret key",
                        file_name = key_file,
                        err = err.to_string(),
                    );
                    return Err(err.into());
                }
            }
        } else if self.libp2p_keypair.is_some() && libp2p_password.is_none() {
            let error = "keyfile is specified, but `MINA_LIBP2P_PASS` is not set";
            mina_node::core::error!(
                mina_node::core::log::system_time();
                summary = error,
            );
            return Err(anyhow::anyhow!(error));
        }

        node_builder.p2p_identity(std::path::Path::new(&work_dir))?;

        // Configure P2P networking
        node_builder.p2p_libp2p_port(self.libp2p_port);
        node_builder.catchup_rpc(true, []);

        // Parsed by clap: a value that isn't an IP address stops the node
        // instead of being dropped.
        node_builder.external_addrs(self.libp2p_external_ip.into_iter());

        node_builder.p2p_max_peers(self.max_peers);
        node_builder.p2p_webrtc_config(webrtc_config);
        self.seed.then(|| node_builder.p2p_seed_node());
        self.no_peers_discovery
            .then(|| node_builder.p2p_no_discovery());

        node_builder.initial_peers(self.peers);
        if let Some(path) = self.peer_list_file {
            node_builder.initial_peers_from_file(path)?;
        }
        if let Some(url) = self.peer_list_url {
            node_builder.initial_peers_from_url(url)?;
        }

        // Build SNARK verifiers
        let block_verifier_index = BlockVerifier::make();
        let work_verifier_index = TransactionVerifier::make();
        node_builder
            .block_verifier_index(block_verifier_index.clone())
            .work_verifier_index(work_verifier_index.clone());

        // Initialize block producer (optional)
        if let Some(producer_key_path) = self.producer_key {
            let password = &producer_key_password;
            mina_node::core::info!(mina_node::core::log::system_time(); summary = "loading block producer provers");
            producer_prover_preload(BlockProver::make(
                Some(block_verifier_index),
                Some(work_verifier_index),
            ))?;
            mina_node::core::info!(mina_node::core::log::system_time(); summary = "loaded block producer provers");
            node_builder.block_producer_from_file(producer_key_path, password, None)?;
            node_builder.block_inject_delay_ms(self.block_inject_delay_ms)?;

            if let Some(pub_key) = self.coinbase_receiver {
                node_builder
                    .custom_coinbase_receiver(pub_key.into())
                    .unwrap();
            }
        }

        // Initialize archive service (optional)
        let archive_storage_options = ArchiveStorageOptions::from_iter(
            [
                (
                    self.archive_local_storage,
                    ArchiveStorageOptions::LOCAL_PRECOMPUTED_STORAGE,
                ),
                (
                    self.archive_archiver_process,
                    ArchiveStorageOptions::ARCHIVER_PROCESS,
                ),
                (
                    self.archive_gcp_storage,
                    ArchiveStorageOptions::GCP_PRECOMPUTED_STORAGE,
                ),
                (
                    self.archive_aws_storage,
                    ArchiveStorageOptions::AWS_PRECOMPUTED_STORAGE,
                ),
            ]
            .iter()
            .filter(|(enabled, _)| *enabled)
            .map(|(_, option)| option.clone()),
        );

        if archive_storage_options.is_enabled() {
            mina_node::core::info!(
                summary = "Archive mode enabled",
                local_storage = archive_storage_options.uses_local_precomputed_storage(),
                archiver_process = archive_storage_options.uses_archiver_process(),
                gcp_storage = archive_storage_options.uses_gcp_precomputed_storage(),
                aws_storage = archive_storage_options.uses_aws_precomputed_storage(),
            );

            archive_storage_options
                .validate_env_vars()
                .map_err(|e| anyhow::anyhow!(e))?;

            node_builder.archive(archive_storage_options, work_dir.clone());
        }

        // Frontier store (on by default, R20)
        if frontier_store {
            let base_dir = match &self.frontier_store_dir {
                Some(dir) => shellexpand::full(dir)
                    .context("--frontier-store-dir")?
                    .into_owned(),
                None => work_dir.clone(),
            };
            anyhow::ensure!(
                self.checkpoint_interval > 0,
                "--checkpoint-interval must be at least 1 second"
            );
            anyhow::ensure!(
                self.frontier_store_load_timeout > 0,
                "--frontier-store-load-timeout must be at least 1 second"
            );
            mina_node::core::debug!(
                mina_node::core::log::system_time();
                summary = "frontier store enabled",
                base_dir = base_dir.clone(),
                reset = self.frontier_store_reset,
                restore = !self.frontier_store_no_restore,
                checkpoint_interval_secs = self.checkpoint_interval,
                load_timeout_secs = self.frontier_store_load_timeout,
            );
            node_builder.frontier_store(
                base_dir.into(),
                self.frontier_store_reset,
                self.checkpoint_interval,
                !self.frontier_store_no_restore,
            );
            node_builder.frontier_store_load_timeout(self.frontier_store_load_timeout);
        }

        // SNARK worker (optional): the flags, else the setting the dashboard
        // saved (P5.4). Its files, and the log of our included work, are in
        // the work directory.
        match saved_worker {
            Ok(_) => {}
            Err(error) => {
                mina_node::core::warn!(
                    mina_node::core::log::system_time();
                    kind = "SnarkWorkerSettingUnreadable",
                    summary = "the SNARK worker setting saved by the dashboard is unusable: the worker stays off",
                    error = error.to_string(),
                );
                // The dashboard says so too, unless the flags start the
                // worker anyway (P5.4).
                if self.run_snarker.is_none() {
                    node_builder.snark_worker_setting_warning(dashboard_setting_warning(
                        &worker_files.setting_path().display().to_string(),
                        &error.to_string(),
                    ));
                }
            }
        }
        // What an operator checks first, in one line (O1). Report the
        // effective settings after P5.4 has resolved the saved worker.
        let on_off = |on: bool| if on { "on" } else { "off" };
        let build = mina_node::BuildEnv::get();
        mina_node::core::info!(
            mina_node::core::log::system_time();
            kind = "NodeStart",
            summary = "starting the node",
            network = mina_core::NetworkConfig::global().name,
            version = build.version.as_str(),
            commit = build.git.commit_hash.get(..12).unwrap_or(&build.git.commit_hash),
            http_port = self.port,
            libp2p_port = self.libp2p_port,
            work_dir = work_dir.as_str(),
            frontier_store = on_off(frontier_store),
            block_producer = on_off(block_producer_enabled),
            snark_worker = on_off(worker.is_some()),
            log_level = self.verbosity.as_str(),
            log_file = log_file.as_ref().filter(|_| file_logging).map_or_else(
                || "off".to_owned(),
                |options| format!(
                    "{} (rotated at {} MB, {} files kept)",
                    options.dir.join(LogFileOptions::FILE_NAME).display(),
                    self.log_file_size_mb,
                    options.max_files,
                ),
            ),
        );
        if let Some(setting) = worker {
            mina_node::core::info!(
                mina_node::core::log::system_time();
                kind = "SnarkWorkerStartup",
                summary = origin.summary(),
                public_key = setting.public_key.to_string(),
                fee = setting.fee,
                strategy = debug(self.snarker_strategy),
                saved_setting = worker_files.setting_path().display().to_string(),
            );
            node_builder.snarker(setting.public_key, setting.fee);
        }
        node_builder
            .snarker_strategy(self.snarker_strategy)
            .block_first_lead(
                (self.snarker_block_first_lead > 0)
                    .then(|| std::time::Duration::from_secs(self.snarker_block_first_lead)),
            )
            .worker_control(http_server_options.worker_control)
            .snark_worker_files(work_dir.clone().into())
            .block_production_history(work_dir.clone().into());

        // Build and run the node
        mina_core::set_work_dir(work_dir.clone().into());
        // The snark pool keeps its works' proofs out of the heap, in files
        // this process owns, as OCaml's daemon keeps a proof cache in its
        // config directory (PC1).
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L1755-L1757>
        // A block producer keeps them in memory unless told otherwise
        // (BP-SAFE): a cached proof can become unreadable, and the producer
        // reads its block's proofs to hash, apply and prove it.
        let proof_cache = ProofCacheChoice::new(
            self.no_proof_cache,
            proof_cache_setting,
            block_producer_enabled,
        );
        let proof_cache_dir = std::path::Path::new(&work_dir).join("proof-cache");
        mina_node::core::info!(
            mina_node::core::log::system_time();
            kind = "ProofCacheMode",
            summary = proof_cache.summary(),
            proof_cache = on_off(proof_cache.enabled()),
            block_producer = on_off(block_producer_enabled),
            dir = proof_cache_dir.display().to_string(),
        );
        proof_cache.open(&proof_cache_dir);
        // The epoch ledgers' and the root snarked ledger's accounts go to
        // files this process owns, as OCaml's daemon keeps them in RocksDB in
        // its config directory (PC3a, PC3b). They are not persistence: a
        // restart removes them and restores from the frontier store.
        if !self.no_ledger_disk {
            // A record that stays unreadable stops the node with one logged
            // line, for its supervisor to restart it: the restart rebuilds
            // the ledgers, as the files are this process's.
            ledger::database::disk::set_exit_on_unreadable(true);
            // Drain the file logger before that exit, so the log file keeps
            // the line saying why. The logger's thread only writes the file,
            // and the guard's drop waits for it at most about a second;
            // `try_lock` skips it if the end of this scope is dropping it.
            let guard = log_guard.0.clone();
            ledger::database::disk::set_before_unreadable_exit(move || {
                if let Ok(mut guard) = guard.try_lock() {
                    drop(guard.take());
                }
            });
            ledger::database::disk::set_root_on_disk(!self.no_root_ledger_disk);
            ledger::database::disk::set_refreeze_slots(self.ledger_refreeze_slots);
        }

        anyhow::ensure!(
            self.metrics_port != Some(self.port),
            "--metrics-port must differ from --port"
        );
        let metrics_listener = self
            .metrics_port
            .map(|port| bind_metrics_listener(self.metrics_addr, port))
            .transpose()?;
        let metrics_store_root = if frontier_store {
            Some(match &self.frontier_store_dir {
                Some(dir) => PathBuf::from(
                    shellexpand::full(dir)
                        .context("--frontier-store-dir")?
                        .into_owned(),
                ),
                None => PathBuf::from(&work_dir),
            })
        } else {
            None
        };

        if let Some(listener) = webrtc_signal_listener {
            #[cfg(feature = "p2p-webrtc")]
            {
                node_builder.webrtc_signal_server(listener);
            }
            #[cfg(not(feature = "p2p-webrtc"))]
            {
                drop(listener);
                anyhow::bail!("--webrtc-signal-port requires a build with p2p-webrtc");
            }
        }

        node_builder
            .http_server(
                http_listener,
                metrics_listener,
                metrics_store_root,
                http_server_options,
            )
            .context("start operator HTTP server")?
            .gather_stats()
            .record(match self.record.as_str() {
                "none" => Recorder::None,
                "state-with-input-actions" => Recorder::only_input_actions(work_dir),
                other => unreachable!("--record {other}: clap accepts only the strategies above"),
            });

        let mut node = node_builder.build().context("node build failed!")?;

        // Start event loop
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .thread_stack_size(64 * 1024 * 1024)
            .build()
            .unwrap();

        runtime.block_on(node.run_forever());

        Ok(())
    }
}

/// The WebRTC settings of the `--webrtc-*` flags (W1). Logs what it read,
/// without credentials.
fn webrtc_config(
    ice_servers: Option<&std::path::Path>,
    udp_ports: Option<UdpPortRange>,
    nat_ips: &[IpAddr],
) -> anyhow::Result<WebrtcConfig> {
    let ice_servers = match ice_servers {
        None => None,
        Some(path) => {
            let json = std::fs::read_to_string(path)
                .with_context(|| format!("reading --webrtc-ice-servers {}", path.display()))?;
            let servers = IceServer::parse_list(&json)
                .with_context(|| format!("--webrtc-ice-servers {}", path.display()))?;
            Some(servers)
        }
    };
    let config = WebrtcConfig {
        ice_servers,
        udp_ports,
        nat_ips: nat_ips.to_vec(),
    };
    let urls: Vec<String> = config
        .ice_servers()
        .into_iter()
        .flat_map(|server| server.urls)
        .collect();
    mina_node::core::info!(
        mina_node::core::log::system_time();
        summary = "WebRTC settings",
        ice_servers = urls.join(" "),
        udp_ports = config.udp_ports.map_or("any".to_owned(), |p| p.to_string()),
        nat_ips = format!("{:?}", config.nat_ips),
    );
    Ok(config)
}

impl Node {
    /// `--proof-cache`, or the hidden older form `MINA_PROOF_CACHE=<bool>`,
    /// whose `false` turns the cache off as `--no-proof-cache` does. `None`
    /// when neither is given.
    fn proof_cache_setting(&self) -> anyhow::Result<Option<bool>> {
        if self.proof_cache {
            return Ok(Some(true));
        }
        let Some(value) = std::env::var_os("MINA_PROOF_CACHE") else {
            return Ok(None);
        };
        let value = value.to_str().map(str::to_ascii_lowercase);
        match value.as_deref() {
            Some("1" | "true" | "t" | "yes" | "y" | "on") => Ok(Some(true)),
            Some("0" | "false" | "f" | "no" | "n" | "off") => Ok(Some(false)),
            _ => anyhow::bail!("MINA_PROOF_CACHE: expected true or false"),
        }
    }

    /// Whether the node keeps a frontier store: on unless turned off. The
    /// store's own flags need it on.
    fn frontier_store_enabled(&self) -> anyhow::Result<bool> {
        let enabled = self.frontier_store && !self.no_frontier_store;
        anyhow::ensure!(
            enabled
                || !(self.frontier_store_no_restore
                    || self.frontier_store_reset
                    || self.frontier_store_dir.is_some()),
            "--frontier-store-no-restore, --frontier-store-reset and --frontier-store-dir \
             need the frontier store, which is turned off"
        );
        Ok(enabled)
    }
}

/// Whether retained proofs go to the proof cache's files, and why (BP-SAFE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProofCacheChoice {
    /// `--no-proof-cache`, or its variables.
    OffByFlag,
    /// A block producer without `--proof-cache`.
    OffForProducer,
    /// `--proof-cache` on a block producer.
    OnForProducer,
    /// A node that doesn't produce blocks.
    On,
}

impl ProofCacheChoice {
    fn new(no_proof_cache: bool, proof_cache: Option<bool>, block_producer: bool) -> Self {
        match (no_proof_cache, proof_cache, block_producer) {
            (true, _, _) | (false, Some(false), _) => Self::OffByFlag,
            (false, None, true) => Self::OffForProducer,
            (false, Some(true), true) => Self::OnForProducer,
            (false, _, false) => Self::On,
        }
    }

    fn enabled(self) -> bool {
        matches!(self, Self::OnForProducer | Self::On)
    }

    /// Selects the node's proof store. A store in `dir` empties it of a
    /// previous run's files; without the cache, nothing in `dir` is read,
    /// written or removed: in a shared work directory, the files there can be
    /// another running node's live cache.
    fn open(self, dir: &std::path::Path) {
        if !self.enabled() {
            if std::fs::symlink_metadata(dir).is_ok() {
                mina_node::core::info!(
                    mina_node::core::log::system_time();
                    kind = "ProofCacheIgnored",
                    summary = "this node keeps proofs in memory; files in the proof cache directory are ignored and left in place",
                    dir = dir.display().to_string(),
                );
            }
            return;
        }
        if let Err(error) = mina_core::snark::proof_cache::set_global_dir(dir) {
            mina_node::core::warn!(
                mina_node::core::log::system_time();
                kind = "ProofCacheUnavailable",
                summary = "the proof cache directory could not be opened; cached proofs stay in memory",
                dir = dir.display().to_string(),
                error = error.to_string(),
            );
        }
    }

    fn summary(self) -> &'static str {
        match self {
            Self::OffByFlag => "proofs are kept in memory: the proof cache is turned off",
            Self::OffForProducer => {
                "proofs are kept in memory, the default for a block producer, so no proof \
                 cache read can fail while it builds or proves a block; --proof-cache turns \
                 the cache on"
            }
            Self::OnForProducer => {
                "proofs are cached on disk for this block producer, as --proof-cache asks; \
                 a failed read can cost it a block"
            }
            Self::On => "proofs are cached on disk",
        }
    }
}

/// Where the SNARK worker's key and fee come from at startup (P5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerOrigin {
    /// `--run-snarker`, which wins over a saved setting.
    Flags,
    /// The setting the dashboard saved.
    Saved,
    None,
}

impl WorkerOrigin {
    fn summary(self) -> &'static str {
        match self {
            Self::Flags => {
                "SNARK worker from --run-snarker (any setting saved by the dashboard is not used)"
            }
            Self::Saved => "SNARK worker from the setting the dashboard saved",
            Self::None => "no SNARK worker",
        }
    }
}

/// The worker the node starts with: the flags' if given, else the saved one.
fn startup_worker(
    flags: Option<SnarkerSetting>,
    saved: Option<SnarkerSetting>,
) -> (Option<SnarkerSetting>, WorkerOrigin) {
    match (flags, saved) {
        (Some(flags), _) => (Some(flags), WorkerOrigin::Flags),
        (None, Some(saved)) => (Some(saved), WorkerOrigin::Saved),
        (None, None) => (None, WorkerOrigin::None),
    }
}

/// The dashboard's warning about an unusable saved worker setting. Any page
/// can read it (`GET /snarker/control`), so it names the file in the work
/// directory but not its path; the log keeps the full error.
fn dashboard_setting_warning(path: &str, error: &str) -> String {
    let reason = error
        .strip_prefix(path)
        .and_then(|rest| rest.strip_prefix(": "))
        .filter(|reason| !reason.contains(path))
        .unwrap_or("it could not be read");
    format!(
        "the worker setting saved by the dashboard ({SETTING_FILE} in the node's work \
         directory) could not be used, so the worker stayed off: {reason}. Start the worker \
         again here, or delete the file."
    )
}

/// Reserve both sockets before either server can start. On a failed second
/// bind, the first socket is dropped with this function's local variables.
fn bind_http_listeners(
    http_addr: IpAddr,
    http_port: u16,
    signal_addr: IpAddr,
    signal_port: Option<u16>,
) -> anyhow::Result<(TcpListener, Option<TcpListener>)> {
    if let Some(port) = signal_port {
        anyhow::ensure!(
            cfg!(feature = "p2p-webrtc"),
            "--webrtc-signal-port {port} requires a build with p2p-webrtc"
        );
        let overlaps = match (http_addr, signal_addr) {
            (IpAddr::V4(a), IpAddr::V4(b)) => a == b || a.is_unspecified() || b.is_unspecified(),
            (IpAddr::V6(a), IpAddr::V6(b)) => a == b || a.is_unspecified() || b.is_unspecified(),
            // Dual-stack and mapped-address overlaps depend on the OS and
            // are caught by the actual binds below, still before any thread.
            _ => false,
        };
        anyhow::ensure!(
            port == 0 || port != http_port || !overlaps,
            "operator HTTP (--http-addr {http_addr} --port {http_port}) overlaps \
             WebRTC signaling (--webrtc-signal-addr {signal_addr} --webrtc-signal-port {port}); \
             choose different ports or non-overlapping addresses"
        );
    }
    let http = bind_http_listener(http_addr, http_port, "--http-addr", "--port")?;
    let signal = signal_port
        .map(|port| {
            bind_http_listener(
                signal_addr,
                port,
                "--webrtc-signal-addr",
                "--webrtc-signal-port",
            )
        })
        .transpose()?;
    Ok((http, signal))
}

fn bind_http_listener(
    addr: IpAddr,
    port: u16,
    addr_flag: &str,
    port_flag: &str,
) -> anyhow::Result<TcpListener> {
    let listener = TcpListener::bind((addr, port))
        .with_context(|| format!("cannot bind {addr_flag} {addr} {port_flag} {port}"))?;
    listener
        .set_nonblocking(true)
        .with_context(|| format!("cannot configure {addr_flag} {addr} {port_flag} {port}"))?;
    Ok(listener)
}

fn bind_metrics_listener(addr: IpAddr, port: u16) -> anyhow::Result<TcpListener> {
    let listener = TcpListener::bind((addr, port))
        .with_context(|| format!("cannot bind metrics listener on {addr}:{port}"))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

#[derive(Clone)]
struct SnarkerValueParser<T>(fn(&str) -> Result<T, &'static str>);

impl<T: Clone + Send + Sync + 'static> TypedValueParser for SnarkerValueParser<T> {
    type Value = T;

    fn parse_ref(
        &self,
        _cmd: &clap::Command,
        _arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<T, clap::Error> {
        // Like R35's secret_value parser: a function parser lets clap echo
        // the rejected input. An operator may paste a secret into either
        // flag, so omit input, underlying errors and usage context entirely,
        // including when the value is not UTF-8.
        value
            .to_str()
            .ok_or("expected a UTF-8 public key or fee")
            .and_then(self.0)
            .map_err(|message| clap::Error::raw(ErrorKind::ValueValidation, message))
    }
}

/// Takes and drops the value in its mutex when it goes out of scope, though
/// other handles to the mutex remain.
struct DropAtEnd<T>(std::sync::Arc<std::sync::Mutex<Option<T>>>);

impl<T> Drop for DropAtEnd<T> {
    fn drop(&mut self) {
        if let Ok(mut value) = self.0.lock() {
            drop(value.take());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn default_genesis_and_explicit_override_for_both_networks() {
        use mina_core::network::NetworkId;
        use mina_node::config::{DEVNET_CONFIG, MAINNET_CONFIG};

        for (network, expected, override_bytes) in [
            (
                NetworkId::MAINNET,
                &*MAINNET_CONFIG,
                include_bytes!("../../../../../genesis_ledgers/mesa-devnet-without-ledgers.json")
                    .as_slice(),
            ),
            (
                NetworkId::TESTNET,
                &*DEVNET_CONFIG,
                include_bytes!("../../../../../genesis_ledgers/mesa-mainnet-without-ledgers.json")
                    .as_slice(),
            ),
        ] {
            assert!(Arc::ptr_eq(
                &read_genesis_config(None, &network).unwrap(),
                expected,
            ));
            let mut file = tempfile::NamedTempFile::new().unwrap();
            std::io::Write::write_all(&mut file, override_bytes).unwrap();
            let selected = read_genesis_config(Some(file.path().to_owned()), &network).unwrap();
            let GenesisConfig::WithoutLedgers(selected) = &*selected else {
                panic!("expected compact genesis override");
            };
            let fixture: serde_json::Value = serde_json::from_slice(override_bytes).unwrap();
            assert_eq!(
                selected.genesis_ledger_hash.to_string(),
                fixture["genesis_ledger_hash"].as_str().unwrap(),
            );
            assert_eq!(
                serde_json::to_value(&selected.constants).unwrap(),
                fixture["constants"],
            );
        }
    }

    #[test]
    fn producer_preload_returns_circuit_error_to_startup() {
        let error = producer_prover_preload::<()>(Err(anyhow::anyhow!(
            "cannot load circuit file 4.0.2devnet/step_blockchain_gates.json: test circuit blob missing"
        )))
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "block producer provers failed: cannot load circuit file 4.0.2devnet/step_blockchain_gates.json: test circuit blob missing"
        );
    }

    #[derive(Debug, Parser)]
    struct Cli {
        #[command(flatten)]
        node: Node,
    }

    /// The public genesis winner of Mina's ledgers, a well-known public key.
    const PUBLIC_KEY: &str = "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg";

    fn parse(args: &[&str]) -> Result<Node, clap::Error> {
        Cli::try_parse_from(std::iter::once("node").chain(args.iter().copied())).map(|c| c.node)
    }

    #[test]
    fn metrics_listener_is_opt_in() {
        assert_eq!(parse(&[]).unwrap().metrics_port, None);
        assert_eq!(
            parse(&[]).unwrap().metrics_addr,
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            parse(&["--metrics-port", "9630"]).unwrap().metrics_port,
            Some(9630)
        );
        assert_eq!(
            parse(&["--metrics-addr", "0.0.0.0"]).unwrap().metrics_addr,
            "0.0.0.0".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn signaling_listener_is_opt_in_and_independent_of_operator_http() {
        let defaults = parse(&[]).unwrap();
        assert_eq!(defaults.webrtc_signal_port, None);
        assert!(defaults.webrtc_signal_addr.is_loopback());
        let node = parse(&[
            "--webrtc-signal-port",
            "8303",
            "--webrtc-signal-addr",
            "0.0.0.0",
        ])
        .unwrap();
        assert_eq!(node.webrtc_signal_port, Some(8303));
        assert_eq!(
            node.webrtc_signal_addr,
            "0.0.0.0".parse::<IpAddr>().unwrap()
        );
        assert!(node.http_addr.is_loopback());
    }

    #[test]
    fn http_address_defaults_to_loopback_and_accepts_ipv4_and_ipv6() {
        assert_eq!(
            parse(&[]).unwrap().http_addr,
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            parse(&["--http-addr", "0.0.0.0"]).unwrap().http_addr,
            "0.0.0.0".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            parse(&["--http-addr", "::1"]).unwrap().http_addr,
            "::1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn occupied_metrics_port_fails_before_node_start() {
        let held = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        let error = bind_metrics_listener("127.0.0.1".parse().unwrap(), port).unwrap_err();
        assert!(format!("{error:#}").contains("cannot bind metrics listener"));
    }

    #[cfg(feature = "p2p-webrtc")]
    #[test]
    fn overlapping_operator_and_signaling_addresses_fail_before_binding() {
        for (http, signal) in [
            ("127.0.0.1", "127.0.0.1"),
            ("0.0.0.0", "127.0.0.1"),
            ("127.0.0.1", "0.0.0.0"),
            ("::", "::1"),
            ("::1", "::"),
        ] {
            let error = bind_http_listeners(
                http.parse().unwrap(),
                3888,
                signal.parse().unwrap(),
                Some(3888),
            )
            .unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("overlaps"), "{message}");
            assert!(message.contains("--port 3888"), "{message}");
            assert!(message.contains("--webrtc-signal-port 3888"), "{message}");
        }
    }

    #[cfg(feature = "p2p-webrtc")]
    #[test]
    fn occupied_operator_or_signaling_ports_fail_before_node_start() {
        let addr = "127.0.0.1".parse().unwrap();
        let held = TcpListener::bind((addr, 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        for (http_port, signal_port, flag) in [
            (port, None, "--port"),
            (0, Some(port), "--webrtc-signal-port"),
        ] {
            let error = bind_http_listeners(addr, http_port, addr, signal_port).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains(&format!("{flag} {port}")), "{message}");
            assert!(message.contains("cannot bind"), "{message}");
        }
    }

    #[cfg(feature = "p2p-webrtc")]
    #[test]
    fn failed_signaling_bind_releases_the_reserved_operator_socket() {
        let addr = "127.0.0.1".parse().unwrap();
        let operator = TcpListener::bind((addr, 0)).unwrap();
        let port = operator.local_addr().unwrap().port();
        let held = TcpListener::bind((addr, 0)).unwrap();
        drop(operator);
        assert!(
            bind_http_listeners(addr, port, addr, Some(held.local_addr().unwrap().port())).is_err()
        );
        assert!(TcpListener::bind((addr, port)).is_ok());
    }

    #[cfg(feature = "p2p-webrtc")]
    #[test]
    fn ephemeral_operator_and_signaling_ports_are_reserved_separately() {
        let addr = "127.0.0.1".parse().unwrap();
        let (operator, signal) = bind_http_listeners(addr, 0, addr, Some(0)).unwrap();
        assert_ne!(
            operator.local_addr().unwrap().port(),
            signal.unwrap().local_addr().unwrap().port()
        );
    }

    #[test]
    fn snarker_fee_is_read_as_ocaml_reads_mina() {
        // Expected values from `Currency.Fee.of_mina_string_exn`.
        for (input, nanomina) in [
            ("0.1", 100_000_000),
            ("1", 1_000_000_000),
            ("1.", 1_000_000_000),
            (".5", 500_000_000),
            ("0.000000001", 1),
            ("0.0000000019", 1),
            ("2.25", 2_250_000_000),
            ("18446744073.709551615", u64::MAX),
        ] {
            assert_eq!(parse_snarker_fee(input), Ok(nanomina), "{input}");
        }
        for input in [
            "",
            ".",
            "1.2.3",
            "-1",
            "0x10",
            "1e9",
            " 1",
            "0.12345678é",
            "18446744073.709551616",
        ] {
            assert!(parse_snarker_fee(input).is_err(), "{input:?} accepted");
        }
    }

    #[test]
    fn log_level_is_verbosity_and_log_files_are_capped() {
        let node = parse(&[]).unwrap();
        assert_eq!(node.verbosity, Level::INFO);
        assert_eq!((node.log_file_size_mb, node.log_file_count), (20, 10));

        let node = parse(&[
            "--log-level",
            "debug",
            "--log-file-size-mb",
            "5",
            "--log-file-count",
            "3",
        ])
        .unwrap();
        assert_eq!(node.verbosity, Level::DEBUG);
        assert_eq!((node.log_file_size_mb, node.log_file_count), (5, 3));

        for flag in ["--log-file-size-mb", "--log-file-count"] {
            assert!(parse(&[flag, "0"]).is_err(), "{flag} 0 accepted");
        }
    }

    #[test]
    fn run_snarker_takes_a_public_key_and_a_fee_in_mina() {
        let node = parse(&["--run-snarker", PUBLIC_KEY, "--snarker-fee", "0.25"]).unwrap();
        assert_eq!(node.run_snarker.unwrap().to_string(), PUBLIC_KEY);
        assert_eq!(node.snarker_fee, 250_000_000);

        // OCaml's default fee, 0.1 MINA, not the old 1,000,000 nanomina.
        let node = parse(&["--run-snarker", PUBLIC_KEY]).unwrap();
        assert_eq!(node.snarker_fee, 100_000_000);
    }

    #[test]
    fn native_block_first_lead_can_be_set_or_disabled() {
        assert_eq!(parse(&[]).unwrap().snarker_block_first_lead, 420);
        assert_eq!(
            parse(&["--snarker-block-first-lead", "600"])
                .unwrap()
                .snarker_block_first_lead,
            600
        );
        assert_eq!(
            parse(&["--snarker-block-first-lead", "0"])
                .unwrap()
                .snarker_block_first_lead,
            0
        );
        assert!(parse(&["--snarker-block-first-lead", "4201"]).is_err());
    }

    /// SW-rand: the worker picks jobs at random unless told `seq`, as
    /// OCaml's `--work-selection` defaults to `rand`. The flag needs no
    /// `--run-snarker`: it is also for a worker the dashboard starts.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/cli_entrypoint/mina_cli_entrypoint.ml#L946-L953>
    #[test]
    fn snarker_strategy_is_random_by_default_as_in_ocaml() {
        use SnarkerStrategy::{Random, Sequential};

        assert_eq!(parse(&[]).unwrap().snarker_strategy, Random);
        assert_eq!(
            parse(&["--run-snarker", PUBLIC_KEY])
                .unwrap()
                .snarker_strategy,
            Random
        );
        for (value, strategy) in [
            ("seq", Sequential),
            ("sequential", Sequential),
            ("rand", Random),
            ("random", Random),
        ] {
            let alone = parse(&["--snarker-strategy", value]).unwrap();
            assert_eq!(alone.snarker_strategy, strategy, "{value}");
            assert!(alone.run_snarker.is_none());
            let with_worker =
                parse(&["--run-snarker", PUBLIC_KEY, "--snarker-strategy", value]).unwrap();
            assert_eq!(with_worker.snarker_strategy, strategy, "{value}");
        }
        // OCaml's `roffset` is not implemented.
        assert!(parse(&["--snarker-strategy", "roffset"]).is_err());
    }

    #[test]
    fn run_snarker_refuses_a_private_key_and_bad_keys() {
        // A synthetic private-key-shaped marker, never a real key.
        let secret = format!("EK{}", "123456789ABCDEFGHJKLMNPQRSTUVWXYZ".repeat(2));
        let error = parse(&["--run-snarker", &secret]).unwrap_err();
        assert!(error.to_string().contains("looks like a private key"));
        for flag in ["--run-snarker", "--snarker-fee"] {
            let error = parse(&[flag, &secret]).unwrap_err();
            for diagnostic in [error.to_string(), format!("{error:?}")] {
                for fragment in secret.as_bytes().windows(7) {
                    assert!(
                        !diagnostic.as_bytes().windows(7).any(|s| s == fragment),
                        "validation error disclosed a marker fragment"
                    );
                }
            }
        }

        let mut corrupted = PUBLIC_KEY.to_owned();
        corrupted.replace_range(10..11, "x");
        assert!(parse(&["--run-snarker", &corrupted]).is_err());

        // A fee alone, or a nanomina-style fee without the worker, is refused.
        assert!(parse(&["--snarker-fee", "0.1"]).is_err());
    }

    /// P5.4: the frontier store is on unless turned off, and its own flags
    /// work without `--frontier-store`. CLI-REF: `--no-frontier-store` is the
    /// visible form; `--frontier-store=<BOOL>` stays accepted, hidden.
    #[test]
    fn the_frontier_store_is_on_by_default() {
        let enabled = |args: &[&str]| parse(args).unwrap().frontier_store_enabled().unwrap();
        assert!(enabled(&[]));
        assert!(enabled(&["--frontier-store"]));
        assert!(enabled(&["--frontier-store=true"]));
        assert!(!enabled(&["--frontier-store=false"]));
        assert!(!enabled(&["--frontier-store=off"]));
        assert!(!enabled(&["--no-frontier-store"]));
        assert!(!enabled(&["--frontier-store", "--no-frontier-store"]));

        let node = parse(&["--frontier-store-no-restore", "--frontier-store-reset"]).unwrap();
        assert!(node.frontier_store_enabled().unwrap());
        assert!(node.frontier_store_no_restore && node.frontier_store_reset);
        let node = parse(&["--frontier-store-dir", "/tmp/store"]).unwrap();
        assert!(node.frontier_store_enabled().unwrap());

        // The store's own flags need it.
        for flags in [
            &["--no-frontier-store", "--frontier-store-no-restore"][..],
            &["--frontier-store=false", "--frontier-store-reset"],
            &["--no-frontier-store", "--frontier-store-dir", "/tmp/store"],
        ] {
            assert!(parse(flags).unwrap().frontier_store_enabled().is_err());
        }
        assert!(parse(&["--frontier-store=maybe"]).is_err());
    }

    /// BP-SAFE: a block producer keeps proofs in memory unless it opts in;
    /// other nodes cache them unless they opt out.
    #[test]
    fn a_block_producer_keeps_proofs_in_memory_unless_it_opts_in() {
        let choice = |args: &[&str]| {
            let node = parse(args).unwrap();
            ProofCacheChoice::new(
                node.no_proof_cache,
                node.proof_cache_setting().unwrap(),
                node.producer_key.is_some(),
            )
        };
        let key = ["--producer-key", "/nonexistent/producer-key"];
        let with_key = |extra: &[&'static str]| {
            let mut args = key.to_vec();
            args.extend_from_slice(extra);
            choice(&args)
        };

        assert_eq!(with_key(&[]), ProofCacheChoice::OffForProducer);
        assert!(!with_key(&[]).enabled());
        assert_eq!(
            with_key(&["--proof-cache"]),
            ProofCacheChoice::OnForProducer
        );
        assert_eq!(
            with_key(&["--proof-cache", "--no-proof-cache"]),
            ProofCacheChoice::OffByFlag
        );

        assert_eq!(choice(&[]), ProofCacheChoice::On);
        assert!(choice(&[]).enabled());
        assert_eq!(choice(&["--proof-cache"]), ProofCacheChoice::On);
        assert_eq!(choice(&["--no-proof-cache"]), ProofCacheChoice::OffByFlag);
        let worker = choice(&["--run-snarker", PUBLIC_KEY, "--snarker-fee", "0.1"]);
        assert_eq!(worker, ProofCacheChoice::On);

        // CLI-REF: `--proof-cache` is a plain flag; `--no-proof-cache` turns
        // the cache off.
        for value in ["true", "false", "maybe"] {
            assert!(parse(&[&format!("--proof-cache={value}")]).is_err());
        }
    }

    /// BP-SAFE: a node without the cache leaves the proof cache directory
    /// alone, even when another running node's live store is in it, and
    /// keeps its own proofs in memory.
    #[test]
    fn a_node_without_the_cache_leaves_a_shared_cache_directory_alone() {
        use mina_core::snark::proof_cache::{global, ProofStore};
        let work = tempfile::tempdir().unwrap();
        let dir = work.path().join("proof-cache");
        let live = ProofStore::dir(&dir).unwrap();
        let blobs: Vec<_> = (0..3u8).map(|i| live.put(vec![i; 32])).collect();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let listing = || {
            let mut names: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            names.sort();
            names
        };
        let before = listing();
        assert_eq!(before.len(), 4);
        for choice in [
            ProofCacheChoice::OffForProducer,
            ProofCacheChoice::OffByFlag,
        ] {
            choice.open(&dir);
        }
        assert_eq!(listing(), before);
        for (i, blob) in blobs.iter().enumerate() {
            assert!(blob.is_file());
            assert_eq!(&*blob.bytes().unwrap(), &[i as u8; 32][..]);
        }
        assert!(global().is_memory());
    }

    /// Run by the test below in a child process, with the environment under
    /// test, so this process's environment is never changed.
    #[test]
    fn proof_cache_choice_in_a_child_environment() {
        let Ok(args) = std::env::var("BP_SAFE_CHILD_ARGS") else {
            return;
        };
        let args: Vec<&str> = args.split_whitespace().collect();
        let node = parse(&args).unwrap();
        let choice = match node.proof_cache_setting() {
            Ok(setting) => format!(
                "{:?}",
                ProofCacheChoice::new(node.no_proof_cache, setting, node.producer_key.is_some())
            ),
            Err(error) => format!("Error({error})"),
        };
        println!("BP_SAFE_CHOICE={choice}");
    }

    /// The variables act as their flags do: `MINA_NO_PROOF_CACHE` wins over
    /// the hidden `MINA_PROOF_CACHE`, `--proof-cache` wins over it, and
    /// `MINA_PRODUCER_KEY` makes a producer. The bare `PRODUCER_KEY` is not
    /// read.
    #[test]
    fn the_proof_cache_variables_follow_the_flags() {
        let choice = |env: &[(&str, &str)], args: &str| {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "commands::node::tests::proof_cache_choice_in_a_child_environment",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env_clear()
                .env("BP_SAFE_CHILD_ARGS", args);
            for (name, value) in env {
                command.env(name, value);
            }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{output:?}");
            let stdout = String::from_utf8(output.stdout).unwrap();
            stdout
                .lines()
                .find_map(|line| line.split_once("BP_SAFE_CHOICE=").map(|(_, c)| c.trim()))
                .unwrap_or_else(|| panic!("no choice in {stdout}"))
                .to_owned()
        };
        let key = ("MINA_PRODUCER_KEY", "/nonexistent/producer-key");
        assert_eq!(
            choice(&[("PRODUCER_KEY", "/nonexistent/producer-key")], ""),
            "On"
        );
        assert_eq!(choice(&[key], ""), "OffForProducer");
        assert_eq!(
            choice(&[key, ("MINA_PROOF_CACHE", "true")], ""),
            "OnForProducer"
        );
        assert_eq!(
            choice(&[key, ("MINA_PROOF_CACHE", "false")], ""),
            "OffByFlag"
        );
        assert_eq!(
            choice(
                &[
                    key,
                    ("MINA_PROOF_CACHE", "true"),
                    ("MINA_NO_PROOF_CACHE", "true")
                ],
                ""
            ),
            "OffByFlag"
        );
        assert_eq!(
            choice(&[key, ("MINA_PROOF_CACHE", "false")], "--proof-cache"),
            "OnForProducer"
        );
        assert_eq!(
            choice(&[key, ("MINA_PROOF_CACHE", "true")], "--no-proof-cache"),
            "OffByFlag"
        );
        assert_eq!(choice(&[], ""), "On");
        assert_eq!(choice(&[("MINA_NO_PROOF_CACHE", "true")], ""), "OffByFlag");
        assert_eq!(choice(&[("MINA_PROOF_CACHE", "off")], ""), "OffByFlag");
        assert_eq!(
            choice(&[("MINA_PROOF_CACHE", "maybe")], ""),
            "Error(MINA_PROOF_CACHE: expected true or false)"
        );
    }

    #[test]
    fn frontier_store_load_timeout_is_bounded_at_parse_time() {
        for seconds in ["1", "60", "300"] {
            assert_eq!(
                parse(&["--frontier-store-load-timeout", seconds])
                    .unwrap()
                    .frontier_store_load_timeout,
                seconds.parse::<u64>().unwrap()
            );
        }
        for seconds in ["0", "301", "18446744073709551615", "invalid"] {
            let error = parse(&["--frontier-store-load-timeout", seconds]).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("--frontier-store-load-timeout must be between 1 and 300 seconds"),
                "{error}"
            );
        }
    }

    /// F-console: the dashboard is served unless `--no-dashboard` (or
    /// `MINA_NO_DASHBOARD`) says otherwise.
    #[test]
    fn dashboard_is_on_unless_turned_off() {
        let options = |args: &[&str]| parse(args).unwrap().http_server_options().unwrap();
        assert!(options(&[]).dashboard);
        assert!(!options(&["--no-dashboard"]).dashboard);
    }

    /// P5.4: worker control is off unless asked for, and never with the
    /// public API.
    #[test]
    fn worker_control_is_opt_in_and_not_public() {
        assert!(!parse(&[]).unwrap().allow_worker_control);
        assert!(
            parse(&["--allow-worker-control"])
                .unwrap()
                .allow_worker_control
        );
        assert!(parse(&["--allow-worker-control", "--public-graphql"]).is_err());
        // More origins for the dashboard, only with control on.
        let node = parse(&[
            "--allow-worker-control",
            "--worker-control-origin",
            "https://dash.example.org",
            "--worker-control-origin=http://10.0.0.2:4200",
        ])
        .unwrap();
        assert_eq!(
            node.worker_control_origins,
            ["https://dash.example.org", "http://10.0.0.2:4200"]
        );
        assert!(parse(&["--worker-control-origin", "https://dash.example.org"]).is_err());
    }

    /// P5.4: `--run-snarker` wins over the setting the dashboard saved;
    /// without it the saved setting starts the worker.
    #[test]
    fn the_saved_worker_setting_comes_back_unless_flags_are_given() {
        let saved = SnarkerSetting {
            public_key: PUBLIC_KEY.parse().unwrap(),
            fee: 4_000_000,
        };
        let node = parse(&["--run-snarker", PUBLIC_KEY, "--snarker-fee", "0.25"]).unwrap();
        let flags = node.run_snarker.map(|public_key| SnarkerSetting {
            public_key,
            fee: node.snarker_fee,
        });
        let (worker, origin) = startup_worker(flags.clone(), Some(saved.clone()));
        assert_eq!((worker, origin), (flags, WorkerOrigin::Flags));
        assert_eq!(
            worker_fee(&startup_worker(None, Some(saved.clone()))),
            Some(4_000_000)
        );
        assert_eq!(
            startup_worker(None, Some(saved.clone())).1,
            WorkerOrigin::Saved
        );
        assert_eq!(startup_worker(None, None), (None, WorkerOrigin::None));

        // From the file the dashboard writes, as a restart reads it.
        let dir = tempfile::tempdir().unwrap();
        let files = SnarkWorkerFiles::new(dir.path());
        files.save_setting(Some(&saved)).unwrap();
        assert_eq!(
            startup_worker(None, files.load_setting().unwrap()),
            (Some(saved), WorkerOrigin::Saved)
        );
        // Stopped from the dashboard: nothing comes back.
        files.save_setting(None).unwrap();
        assert_eq!(
            startup_worker(None, files.load_setting().unwrap()),
            (None, WorkerOrigin::None)
        );
    }

    /// PR #41's review: the warning reaches any page, so no path in it.
    #[test]
    fn the_dashboard_warning_has_no_path() {
        let dir = tempfile::tempdir().unwrap();
        let files = SnarkWorkerFiles::new(dir.path());
        std::fs::write(files.setting_path(), b"not json").unwrap();
        let error = files.load_setting().unwrap_err().to_string();
        let path = files.setting_path().display().to_string();
        assert!(error.contains(&path), "the log keeps the path: {error}");
        let warning = dashboard_setting_warning(&path, &error);
        let dir_name = dir.path().display().to_string();
        assert!(!warning.contains(&dir_name), "{warning}");
        assert!(warning.contains("snark-worker.json"), "{warning}");
        assert!(warning.contains("expected"), "keeps the reason: {warning}");
        assert!(!dashboard_setting_warning(&path, &format!("other {path}")).contains(&dir_name));
    }

    fn worker_fee(worker: &(Option<SnarkerSetting>, WorkerOrigin)) -> Option<u64> {
        worker.0.as_ref().map(|setting| setting.fee)
    }

    // CLI-REF: an unknown mode was a panic after the ports were bound; it is
    // now a parse error. Surrounding whitespace is accepted, as `trim` did.
    #[test]
    fn record_modes_are_checked_at_parse_time() {
        assert_eq!(parse(&[]).unwrap().record, "none");
        for (value, mode) in [
            ("state-with-input-actions", "state-with-input-actions"),
            (" none ", "none"),
            ("state-with-input-actions\n", "state-with-input-actions"),
        ] {
            assert_eq!(parse(&["--record", value]).unwrap().record, mode);
        }
        let error = parse(&["--record", "everything"]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }

    // CLI-REF: a value that isn't an IP address was dropped without a word.
    #[test]
    fn external_ips_must_be_ip_addresses() {
        let node = parse(&[
            "--libp2p-external-ip",
            "203.0.113.7",
            "--libp2p-external-ip",
            "2001:db8::1",
        ])
        .unwrap();
        assert_eq!(
            node.libp2p_external_ip,
            [
                "203.0.113.7".parse::<IpAddr>().unwrap(),
                "2001:db8::1".parse().unwrap()
            ]
        );
        for value in ["node.example.com", "203.0.113.7:8302", ""] {
            let error = parse(&["--libp2p-external-ip", value]).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::ValueValidation, "{value:?}");
        }
    }

    // CLI-REF: the help named MINA_ARCHIVE_LOCAL_STORAGE_PATH, which nothing
    // reads; the archive service reads MINA_LOCAL_PRECOMPUTED_STORAGE_PATH.
    #[test]
    fn archive_local_storage_help_names_the_variable_read() {
        let command = <Cli as clap::CommandFactory>::command();
        let help = command
            .get_arguments()
            .find(|arg| arg.get_long() == Some("archive-local-storage"))
            .and_then(|arg| arg.get_long_help().or(arg.get_help()))
            .unwrap()
            .to_string();
        assert!(help.contains("MINA_LOCAL_PRECOMPUTED_STORAGE_PATH"));
        assert!(!help.contains("MINA_ARCHIVE_LOCAL_STORAGE_PATH"));
    }
}
