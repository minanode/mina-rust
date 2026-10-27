//! I5d2: the compact Mesa mainnet genesis,
//! `genesis_ledgers/mesa-mainnet-without-ledgers.json` (`--genesis-config`),
//! is the genesis a node started with the full daemon JSON (`--config`) has:
//! the same constants, ledger hashes, seeds, fork point and daemon settings,
//! the same genesis block and the same chain id.
//!
//! Every expected value is OCaml's: its packaged 4.0.2 mainnet config
//! (`tests/files/ocaml/mainnet-4.0.2-config.json`, `config_84b8d969.json` of
//! the 4.0.2 deb, SHA-256 `6e954987...`, the daemon JSON without its accounts),
//! P11d's constants fixture, and the genesis block a live 4.0.2 daemon serves
//! over GraphQL (`GenesisBlock.json`). The daemon JSON itself (736 MB) is not
//! in the repository; without its accounts it is byte for byte OCaml's config
//! (I5d2's note).
use mina_core::{
    constants::{
        constraint_constants, stop_slots, PROTOCOL_NETWORK_VERSION, PROTOCOL_TRANSACTION_VERSION,
        TX_POOL_MAX_SIZE,
    },
    network::mainnet,
    ChainId, NetworkConfig,
};
use mina_node::{
    daemon_json::{Daemon, DaemonJson},
    transition_frontier::genesis::{GenesisConfig, GenesisConfigLoaded, GenesisRuntimeConfig},
};
use mina_p2p_messages::v2;
use serde_json::Value;

const COMPACT: &[u8] = include_bytes!("../../../genesis_ledgers/mesa-mainnet-without-ledgers.json");
const OCAML_CONFIG: &str = include_str!("../../../tests/files/ocaml/mainnet-4.0.2-config.json");
const OCAML_CONSTANTS: &str =
    include_str!("../../../tests/files/ocaml/mainnet-4.0.2-constants.json");
const OCAML_GENESIS_BLOCK: &str =
    include_str!("../native/tests/files/graphql/ocaml/GenesisBlock.json");

/// A constant of the P11d fixture, after checking that OCaml's sources agree.
fn ocaml(fixture: &Value, name: &str) -> Value {
    let observations = fixture["constants"][name]
        .as_array()
        .unwrap_or_else(|| panic!("{name} is missing from the fixture"));
    let first = &observations[0]["value"];
    for observation in observations {
        assert_eq!(
            &observation["value"], first,
            "OCaml's sources disagree on {name}"
        );
    }
    first.clone()
}

fn ocaml_u64(fixture: &Value, name: &str) -> u64 {
    ocaml(fixture, name).as_u64().unwrap()
}

fn ocaml_str(fixture: &Value, name: &str) -> String {
    ocaml(fixture, name).as_str().unwrap().to_owned()
}

fn zero() -> v2::CurrencyAmountStableV1 {
    v2::CurrencyAmountStableV1(0u64.into())
}

/// Mainnet, once per process: the tests share it.
fn mainnet() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| NetworkConfig::init("mainnet").unwrap());
}

fn compact() -> GenesisConfig {
    GenesisConfig::from_bytes(COMPACT.to_vec()).expect("the compact config reads")
}

/// The default must derive OCaml's pinned mainnet chain id, with no external
/// config. The fixture records `mina internal chain-id`, its traced input,
/// and GraphQL of OCaml 4.0.2 at commit 84b8d969 (P11d).
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/chain_id/chain_id.ml#L44-L70>
#[test]
fn default_mainnet_genesis_has_ocaml_chain_id() {
    mainnet();
    let config = mina_node::config::default_genesis_config(&mina_core::network::NetworkId::MAINNET);
    let (masks, loaded) = config.load().unwrap();
    assert!(masks.is_empty());
    let (_, genesis, genesis_hash) = loaded.protocol_states(zero(), zero(), zero()).unwrap();
    let chain_id = ChainId::compute(
        &mainnet::CONSTRAINT_SYSTEM_DIGESTS,
        &genesis_hash,
        &genesis.body.constants,
        PROTOCOL_TRANSACTION_VERSION,
        PROTOCOL_NETWORK_VERSION,
        &v2::UnsignedExtendedUInt32StableV1::from(TX_POOL_MAX_SIZE),
    );
    let fixture: Value = serde_json::from_str(OCAML_CONSTANTS).unwrap();
    assert_eq!(chain_id.to_hex(), ocaml_str(&fixture, "chain_id"));
}

#[test]
fn compact_mainnet_genesis_is_ocamls() {
    mainnet();
    let fixture: Value = serde_json::from_str(OCAML_CONSTANTS).unwrap();
    assert_eq!(fixture["profile"], "mainnet");
    let ocaml_config: DaemonJson = serde_json::from_str(OCAML_CONFIG).unwrap();
    let block: Value = serde_json::from_str(OCAML_GENESIS_BLOCK).unwrap();
    let block = &block["data"]["genesisBlock"];

    let config = compact();
    assert!(matches!(config, GenesisConfig::WithoutLedgers(_)));
    let (masks, loaded) = config.load().unwrap();
    assert!(masks.is_empty(), "no genesis ledger is loaded");

    // The runtime settings are the daemon JSON's: its `proof` (the fork
    // point) and `daemon` sections, as `--config` takes them.
    assert_eq!(
        serde_json::to_value(&*loaded.runtime_config).unwrap(),
        serde_json::to_value(GenesisRuntimeConfig::from_daemon(&ocaml_config)).unwrap()
    );
    let fork = constraint_constants().fork.as_ref().expect("a fork point");
    assert_eq!(
        v2::StateHash::from_fp(fork.state_hash).to_string(),
        ocaml_str(&fixture, "fork.state_hash")
    );
    assert_eq!(
        u64::from(fork.blockchain_length),
        ocaml_u64(&fixture, "fork.blockchain_length")
    );
    assert_eq!(
        u64::from(fork.global_slot_since_genesis),
        ocaml_u64(&fixture, "fork.global_slot_since_genesis")
    );
    // No stop slots, and OCaml's transaction pool size, as with `--config`.
    assert_eq!(stop_slots().slot_tx_end, None);
    assert_eq!(stop_slots().slot_chain_end, None);
    let daemon = loaded.runtime_config.daemon.clone().unwrap();
    assert_eq!(
        daemon.tx_pool_max_size() as u64,
        ocaml_u64(&fixture, "txpool_max_size")
    );
    assert_eq!(daemon.slot_tx_end(), None);
    assert_eq!(
        daemon.peer_list_url(),
        ocaml_config.daemon.as_ref().and_then(Daemon::peer_list_url)
    );

    // The protocol constants.
    let constants = &loaded.constants;
    assert_eq!(u64::from(constants.k.as_u32()), ocaml_u64(&fixture, "k"));
    assert_eq!(
        u64::from(constants.slots_per_epoch.as_u32()),
        ocaml_u64(&fixture, "slots_per_epoch")
    );
    assert_eq!(
        u64::from(constants.slots_per_sub_window.as_u32()),
        ocaml_u64(&fixture, "slots_per_sub_window")
    );
    assert_eq!(
        u64::from(constants.grace_period_slots.as_u32()),
        ocaml_u64(&fixture, "grace_period_slots")
    );
    assert_eq!(
        u64::from(constants.delta.as_u32()),
        ocaml_u64(&fixture, "delta")
    );
    assert_eq!(
        constants.genesis_state_timestamp.0 .0.as_u64(),
        ocaml_u64(&fixture, "genesis_state_timestamp_ms")
    );
    assert_eq!(
        serde_json::to_value(constants).unwrap(),
        serde_json::to_value(ocaml_config.genesis.as_ref().unwrap().protocol_constants()).unwrap(),
        "the constants `--config` derives from OCaml's config"
    );

    // The ledgers and seeds, as OCaml's config names them.
    let ledger = ocaml_config.ledger.as_ref().unwrap();
    let epochs = ocaml_config.epoch_data.as_ref().unwrap();
    let next = epochs.next.as_ref().unwrap();
    assert_eq!(
        Some(loaded.genesis_ledger_hash.to_string()),
        ledger.hash.clone()
    );
    assert_eq!(
        Some(loaded.staking_epoch_ledger_hash.to_string()),
        epochs.staking.hash.clone()
    );
    assert_eq!(loaded.staking_epoch_seed.to_string(), epochs.staking.seed);
    assert_eq!(
        Some(loaded.next_epoch_ledger_hash.to_string()),
        next.hash.clone()
    );
    assert_eq!(loaded.next_epoch_seed.to_string(), next.seed);
    for (ours, name) in [
        (&loaded.genesis_ledger_hash, "genesis_ledger_hash"),
        (
            &loaded.staking_epoch_ledger_hash,
            "staking_epoch_ledger_hash",
        ),
        (&loaded.next_epoch_ledger_hash, "next_epoch_ledger_hash"),
    ] {
        assert_eq!(ours.to_string(), ocaml_str(&fixture, name));
    }

    // The genesis block, with the total currencies the compact config
    // carries (which the daemon JSON's accounts sum to): OCaml's.
    let (_, genesis, genesis_hash) = loaded
        .protocol_states(
            loaded.genesis_total_currency.clone(),
            loaded.staking_epoch_total_currency.clone(),
            loaded.next_epoch_total_currency.clone(),
        )
        .unwrap();
    assert_eq!(
        genesis_hash.to_string(),
        block["stateHash"].as_str().unwrap()
    );
    let consensus = &genesis.body.consensus_state;
    assert_eq!(
        consensus.blockchain_length.as_u32().to_string(),
        block["protocolState"]["consensusState"]["blockHeight"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        genesis.previous_state_hash.to_string(),
        block["protocolState"]["previousStateHash"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        consensus.next_epoch_data.seed.to_string(),
        block["protocolState"]["consensusState"]["nextEpochData"]["seed"]
            .as_str()
            .unwrap()
    );

    // The chain id, from the genesis state with zero total currencies.
    let (_, genesis, chain_id_genesis_hash) =
        loaded.protocol_states(zero(), zero(), zero()).unwrap();
    assert_eq!(
        chain_id_genesis_hash.to_string(),
        ocaml_str(&fixture, "chain_id.genesis_state_hash")
    );
    let chain_id = ChainId::compute(
        &mainnet::CONSTRAINT_SYSTEM_DIGESTS,
        &chain_id_genesis_hash,
        &genesis.body.constants,
        PROTOCOL_TRANSACTION_VERSION,
        PROTOCOL_NETWORK_VERSION,
        &v2::UnsignedExtendedUInt32StableV1::from(TX_POOL_MAX_SIZE),
    );
    assert_eq!(chain_id.to_hex(), ocaml_str(&fixture, "chain_id"));
}

/// The compact config reads back as `ledger-tool` wrote it, and the genesis
/// it gives through the store's eyes (`GenesisConfigLoaded`, kept as JSON by
/// the genesis cache) is stable.
#[test]
fn compact_mainnet_genesis_round_trips() {
    mainnet();
    let GenesisConfig::WithoutLedgers(loaded) = compact() else {
        panic!("not a config without ledgers");
    };
    let written = serde_json::to_vec(&*loaded).unwrap();
    let reread: GenesisConfigLoaded = serde_json::from_slice(&written).unwrap();
    assert_eq!(written, serde_json::to_vec(&reread).unwrap());
    assert_eq!(
        serde_json::from_slice::<Value>(&written).unwrap(),
        serde_json::from_slice::<Value>(COMPACT).unwrap()
    );
}
