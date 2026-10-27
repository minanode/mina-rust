#![cfg(target_family = "wasm")]

pub use mina_node_common::*;

mod rayon;
pub use rayon::init_rayon;

mod block_prover;
pub use block_prover::{
    block_prover_realm_install_key, block_prover_realm_prove, block_prover_realm_prove_work,
    block_prover_realm_setup,
};

mod frontier_store;

mod memory;
pub use memory::memory_stats;

mod node;
pub use node::{Node, NodeBuilder};

use anyhow::Context;
use gloo_utils::format::JsValueSerdeExt;
use mina_node::{
    account::{parse_coinbase_receiver, parse_snarker, AccountPublicKey, AccountSecretKey},
    core::{log, thread},
    p2p::webrtc::{IceServer, WebrtcConfig},
    snark::{BlockVerifier, TransactionVerifier},
    transition_frontier::{
        genesis::GenesisConfig,
        persist::{
            TransitionFrontierPersistConfig, DEFAULT_CHECKPOINT_INTERVAL_SECS,
            WEB_LOAD_TIMEOUT_SECS,
        },
    },
    SnarkerStrategy,
};
use mina_node_common::rpc::RpcSender;
use wasm_bindgen::prelude::*;

use crate::node::P2pTaskRemoteSpawner;

/// Automatically run after wasm is loaded.
#[wasm_bindgen(start)]
fn main() {
    thread::main_thread_init();
    wasm_bindgen_futures::spawn_local(async {
        console_error_panic_hook::set_once();
        // Debug level logs every action to the console, which slows the node
        // down severalfold while the browser's console is being read.
        tracing::initialize(tracing::Level::INFO);

        // The block prover's iframe instantiates this module too, and builds
        // its own pool, of its own size, when it is set up.
        if !block_prover::in_prover_realm() {
            init_rayon().await.unwrap();
        }
    });
}

#[wasm_bindgen]
pub fn build_env() -> JsValue {
    JsValue::from_serde(&::mina_node::BuildEnv::get()).unwrap_or_default()
}

fn parse_bp_key(key: JsValue) -> Option<AccountSecretKey> {
    if key.is_falsy() {
        return None;
    }

    if key.is_string() {
        return Some(
            key.as_string()
                .unwrap()
                .parse()
                .expect("failed to parse passed block producer keys"),
        );
    }

    let (encrypted, password) = if key.is_array() {
        let arr: js_sys::Array = key.into();
        let password = arr
            .at(1)
            .as_string()
            .expect("invalid block_producer password");
        let encrypted = arr
            .at(0)
            .into_serde()
            .expect("block_producer encrypted key decode failed");
        (encrypted, password)
    } else {
        panic!("unsupported block_producer keys type: {key:?}");
    };

    Some(
        AccountSecretKey::from_encrypted(&encrypted, &password)
            .expect("block_producer secret key decrypt failed"),
    )
}

/// The options of [`run`] after its positional arguments: a JavaScript
/// object, every field optional.
#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
struct RunOptions {
    /// Keep a frontier store in this browser's Origin Private File System,
    /// and restore from it at the next start instead of downloading the
    /// ledgers again (I5e). Off unless set.
    frontier_store: bool,
    /// Delete the store when it is opened, then write a new one.
    frontier_store_reset: bool,
    /// Write the store but don't restore from it.
    frontier_store_no_restore: bool,
    /// Seconds between checkpoints of the root (default 30 minutes).
    checkpoint_interval_secs: Option<u64>,
    /// Seconds the node waits for its store to open, and then to load,
    /// before it gives up and bootstraps (default 180).
    load_timeout_secs: Option<u64>,
    /// The STUN and TURN servers of the node's WebRTC connections, as the
    /// browser's `RTCIceServer` list (W1); the built-in STUN servers when
    /// absent, none when empty.
    ice_servers: Option<Vec<IceServer>>,
}

impl RunOptions {
    fn parse(options: JsValue) -> Result<Self, JsError> {
        if options.is_undefined() || options.is_null() {
            return Ok(Self::default());
        }
        let options: Self = options
            .into_serde()
            .map_err(|error| JsError::new(&format!("invalid run options: {error}")))?;
        if let Some(servers) = &options.ice_servers {
            IceServer::validate_list(servers)
                .map_err(|error| JsError::new(&format!("invalid run options: {error}")))?;
        }
        Ok(options)
    }

    fn persist_config(&self) -> TransitionFrontierPersistConfig {
        TransitionFrontierPersistConfig {
            enabled: true,
            checkpoint_interval_secs: self
                .checkpoint_interval_secs
                .unwrap_or(DEFAULT_CHECKPOINT_INTERVAL_SECS),
            restore: !self.frontier_store_no_restore,
            load_timeout_secs: self.load_timeout_secs.unwrap_or(WEB_LOAD_TIMEOUT_SECS),
        }
    }
}

/// Starts a Mina node in a WASM environment and returns an RPC interface.
///
/// This is the main entry point for running a Mina node from JavaScript/WASM.
/// It spawns the node in a separate thread, sets up all necessary components,
/// and returns an RPC sender that can be used to communicate with the running
/// node.
///
/// # Arguments
///
/// * `block_producer` - Block producer configuration as a JavaScript value.
///   Can be one of:
///   - `null`/`undefined`: No block production
///   - `string`: Plain text secret key
///   - `[encrypted_key, password]`: Array with encrypted key and password
/// * `seed_nodes_urls` - Optional list of URLs to fetch peer lists from.
///   Each URL should return newline-separated peer addresses. This is similar
///   to the `--peer-list-url` flag in the native node, but allows multiple URLs
///   to be supplied.
/// * `seed_nodes_addresses` - Optional list of peer addresses to connect to
///   directly, in [WebRTC multiaddr format](https://o1-labs.github.io/mina-rust/docs/developers/webrtc#address-formats).
///   This is directly comparable to the `--peers` flag in the native node.
/// * `genesis_config_url` - Optional URL to fetch genesis configuration from:
///   a prebuilt config (bin_prot) or one without ledgers (JSON), as written by
///   `ledger-tool`. If not provided, uses the embedded Mesa genesis for the
///   selected network.
/// * `network` - Optional network name, `"devnet"` (the default) or
///   `"mainnet"`. It selects the signature prefixes, constants and verifier
///   keys, like the `--network` flag of the native node.
/// * `coinbase_receiver` - Optional public key to receive this producer's
///   coinbase. When absent, the producer receives its own coinbase.
///   Requires a block producer key when supplied.
/// * `snarker` - Optional public key (B62...) to run the SNARK worker for,
///   as OCaml's `--run-snark-worker`: the key receives the fees of the work
///   the node sells. Only a public key: the worker needs no private key. Its
///   proofs are made in the node's isolated prover (P5.3). Not with a block
///   producer key: a SNARK proof in progress cannot be stopped, so a block
///   proof would have to wait behind it and could miss its slot. Run the
///   worker in a node (a tab) that does not produce blocks.
/// * `snarker_fee` - The fee asked for each work bundle, in MINA as OCaml's
///   `--snark-worker-fee` reads it (default 0.1). Requires `snarker`. A key
///   without an account must ask at least 1 MINA, or OCaml refuses the work.
/// * `options` - Optional object of further options:
///   - `frontierStore` (boolean): keep the node's verified ledgers and blocks
///     in this browser's Origin Private File System and, at the next start,
///     restore from them with every check instead of downloading them again.
///     Off by default. A browser without it runs the node without a store.
///   - `frontierStoreReset` (boolean): delete the store first.
///   - `frontierStoreNoRestore` (boolean): write the store, don't restore.
///   - `checkpointIntervalSecs` (number): seconds between checkpoints of the
///     root, 1800 by default.
///   - `loadTimeoutSecs` (number): seconds to wait for the store to open and
///     load before bootstrapping instead, 180 by default.
///   - `iceServers` (array): the STUN and TURN servers of the node's WebRTC
///     connections, in the browser's `RTCIceServer` format, such as
///     `[{ urls: "turns:turn.example.org:5349", username, credential }]`;
///     the built-in STUN servers when absent, none when empty.
///
/// # Returns
///
/// An `RpcSender` that can be used to send RPC commands to the running node.
/// Rejects the JavaScript promise if the receiver is malformed or supplied
/// without a block producer key, or if configuring the receiver fails; if
/// the SNARK worker's key or fee is not valid, without repeating the value,
/// which may be a private key given by mistake; if both a block producer
/// key and a SNARK worker are given; or if `options` is not an object of the
/// fields above. A non-release fork in the genesis config also rejects the
/// promise before either verifier is created.
///
/// # Panics
///
/// Panics if:
/// - Block producer key parsing fails
/// - Genesis configuration cannot be fetched
///
/// # Example
///
/// ```javascript
/// const rpc = await run(
///   null,  // No block production
///   ["https://bootnodes.minaprotocol.com/networks/devnet-webrtc.txt"],
///   ["/dns4/webrtc-peer-signaling.example.com/tcp/443/webrtc/https/p2p/PEER_ID"],
///   null, // Use the default devnet configuration
///   null, // Devnet
///   null, // No coinbase receiver
///   { frontierStore: true },
/// );
/// ```
#[wasm_bindgen]
pub async fn run(
    block_producer: JsValue,
    seed_nodes_urls: Option<Vec<String>>,
    seed_nodes_addresses: Option<Vec<String>>,
    genesis_config_url: Option<String>,
    network: Option<String>,
    coinbase_receiver: Option<String>,
    snarker: Option<String>,
    snarker_fee: Option<String>,
    options: JsValue,
) -> Result<RpcSender, JsError> {
    // Reject invalid receiver or worker input before parsing producer keys or
    // starting any node worker or isolated prover.
    let coinbase_receiver =
        parse_coinbase_receiver(coinbase_receiver.as_deref(), !block_producer.is_falsy())
            .map_err(JsError::new)?;
    let snarker = parse_snarker(snarker.as_deref(), snarker_fee.as_deref())
        .map_err(|error| JsError::new(&error))?;
    if snarker.is_some() && !block_producer.is_falsy() {
        return Err(JsError::new(SNARKER_WITH_PRODUCER));
    }
    let options = RunOptions::parse(options)?;
    let block_producer = parse_bp_key(block_producer);
    let network = network.unwrap_or_else(|| "devnet".to_owned());
    mina_node::core::NetworkConfig::init(&network).expect("failed to set the network");

    let (rpc_sender_tx, rpc_sender_rx) = ::mina_node::core::channels::oneshot::channel();
    let _ = thread::spawn(move || {
        wasm_bindgen_futures::spawn_local(async move {
            let node = setup_node(
                block_producer,
                seed_nodes_urls,
                seed_nodes_addresses,
                genesis_config_url,
                network,
                coinbase_receiver,
                snarker,
                options,
            )
            .await;
            match node {
                Ok(mut node) => {
                    let _ = rpc_sender_tx.send(Ok(node.rpc()));
                    node.run_forever().await;
                }
                Err(error) => {
                    let _ = rpc_sender_tx.send(Err(error.to_string()));
                }
            }
        });

        keep_worker_alive_cursed_hack();
    });

    rpc_sender_rx
        .await
        .map_err(|_| JsError::new("node worker stopped during startup"))?
        .map_err(|error| JsError::new(&error))
}

/// Why a node that produces blocks runs no SNARK worker (P5.3): one prover
/// instance proves one thing at a time and cannot stop a SNARK proof for a
/// block proof, which could then miss its slot, including a won slot the
/// node learns late, after bootstrap or at an epoch change.
const SNARKER_WITH_PRODUCER: &str = "a web node that produces blocks cannot also run the \
    SNARK worker: a SNARK proof in progress cannot be stopped for a block proof, which \
    could then miss its slot. Run the SNARK worker in another tab, without a block \
    producer key";

async fn setup_node(
    block_producer: Option<AccountSecretKey>,
    seed_nodes_urls: Option<Vec<String>>,
    seed_nodes_addresses: Option<Vec<String>>,
    genesis_config_url: Option<String>,
    network: String,
    coinbase_receiver: Option<AccountPublicKey>,
    snarker: Option<(AccountPublicKey, u64)>,
    options: RunOptions,
) -> anyhow::Result<mina_node_common::Node<NodeService>> {
    let genesis_config: std::sync::Arc<GenesisConfig> =
        if let Some(genesis_config_url) = genesis_config_url {
            let bytes = ::mina_node::core::http::get_bytes(&genesis_config_url)
                .await
                .expect("failed to fetch genesis config");
            GenesisConfig::from_bytes(bytes)
                .expect("failed to read genesis config")
                .into()
        } else {
            ::mina_node::config::default_genesis_config(
                &mina_node::core::NetworkConfig::global().network_id,
            )
        };

    genesis_config.prepare_web_startup()?;
    let block_verifier_index = BlockVerifier::make().await;
    let work_verifier_index = TransactionVerifier::make().await;

    let mut node_builder: NodeBuilder = NodeBuilder::new(None, genesis_config);
    node_builder
        .block_verifier_index(block_verifier_index.clone())
        .work_verifier_index(work_verifier_index.clone());

    // TODO(binier): refactor
    let mut all_raw_peers = seed_nodes_addresses.unwrap_or_default();

    if let Some(seed_nodes_urls) = seed_nodes_urls {
        for seed_nodes_url in seed_nodes_urls {
            let peers = ::mina_node::core::http::get_bytes(&seed_nodes_url).await;
            match peers {
                Ok(s) => {
                    log::info!("Successfully fetched peers from {seed_nodes_url}");
                    all_raw_peers.extend(String::from_utf8_lossy(&s).split("\n").map(String::from));
                }
                Err(e) => {
                    log::error!("Failed to fetch peers from {seed_nodes_url}: {e}");
                }
            }
        }
    }

    node_builder.initial_peers(
        all_raw_peers
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .flat_map(|s| s.parse().ok())
            .inspect(|p| log::debug!("Using peer: {p:?}")),
    );

    // Block proofs or SNARK work, never both (`run` refuses that), are made
    // in an isolated prover instance with its own memory, never in this one
    // (I34c, P5.3). Its circuits start loading now, long before the first
    // won slot or job.
    if block_producer.is_some() || snarker.is_some() {
        node_builder.isolated_prover(
            block_producer.map(|key| (key, block_verifier_index)),
            snarker.is_some(),
            network,
        );
    }
    if let Some(receiver) = coinbase_receiver {
        node_builder
            .custom_coinbase_receiver(receiver.into())
            .context("failed to set coinbase receiver")?;
    }
    if let Some((public_key, fee)) = snarker {
        // The worker has the prover to itself: no block to give way to. It
        // picks jobs at random, OCaml's default, as the native worker does.
        node_builder.snarker(public_key, fee, SnarkerStrategy::default(), None);
    }

    if options.frontier_store {
        node_builder.frontier_store(options.persist_config(), options.frontier_store_reset);
    }

    if let Some(ice_servers) = options.ice_servers {
        log::info!("WebRTC ICE servers: {} given", ice_servers.len());
        node_builder.p2p_webrtc_config(WebrtcConfig {
            ice_servers: Some(ice_servers),
            ..Default::default()
        });
    }
    node_builder
        .p2p_custom_task_spawner(P2pTaskRemoteSpawner {})
        .unwrap();
    node_builder.gather_stats();
    node_builder.build().context("node build failed!")
}

pub(crate) fn keep_worker_alive_cursed_hack() {
    wasm_bindgen::throw_str("Cursed hack to keep workers alive. See https://github.com/rustwasm/wasm-bindgen/issues/2945");
}
