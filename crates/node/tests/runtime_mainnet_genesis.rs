//! P11 runtime path, using P11d's independently collected OCaml 4.0.2 fixture.
use mina_core::{
    constants::{
        constraint_constants, PROTOCOL_CONSTANTS, PROTOCOL_NETWORK_VERSION,
        PROTOCOL_TRANSACTION_VERSION, TX_POOL_MAX_SIZE,
    },
    network::mainnet,
    ChainId, NetworkConfig,
};
use mina_node::transition_frontier::genesis::GenesisConfigLoaded;
use mina_p2p_messages::v2;
use serde_json::Value;

const FIXTURE: &str = include_str!("../../../tests/files/ocaml/mainnet-4.0.2-constants.json");

/// The value of a constant, after checking that OCaml's sources agree on it.
fn ocaml(fixture: &Value, name: &str) -> Value {
    let observations = fixture["constants"][name]
        .as_array()
        .unwrap_or_else(|| panic!("{name} is missing from the fixture: regenerate it"));
    let first = &observations[0]["value"];
    for observation in observations {
        assert_eq!(
            &observation["value"], first,
            "OCaml's sources disagree on {name}"
        );
    }
    first.clone()
}

fn parse<T: std::str::FromStr>(fixture: &Value, name: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    ocaml(fixture, name).as_str().unwrap().parse().unwrap()
}

fn zero() -> v2::CurrencyAmountStableV1 {
    v2::CurrencyAmountStableV1(0u64.into())
}

#[test]
fn configured_mainnet_genesis_preserves_ocaml_chain_id() {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("fixture should parse");
    assert_eq!(fixture["profile"], "mainnet");
    NetworkConfig::init("mainnet").unwrap();
    assert!(
        constraint_constants().fork.is_some(),
        "the compiled mainnet constants carry the Mesa fork point"
    );

    let mut constants = PROTOCOL_CONSTANTS.clone();
    let timestamp = ocaml(&fixture, "genesis_state_timestamp_ms")
        .as_u64()
        .unwrap();
    constants.genesis_state_timestamp = v2::BlockTimeTimeStableV1(timestamp.into());

    let staking_epoch_ledger_hash: v2::LedgerHash = parse(&fixture, "staking_epoch_ledger_hash");
    let runtime_config = serde_json::from_value(serde_json::json!({
        "proof": {
            "fork": {"state_hash": ocaml(&fixture, "fork.state_hash"),
                "blockchain_length": ocaml(&fixture, "fork.blockchain_length"),
                "global_slot_since_genesis": ocaml(&fixture, "fork.global_slot_since_genesis")},
            "coinbase_amount": "360", "block_window_duration_ms": 90000
        },
        "daemon": {"slot_tx_end": 100, "slot_chain_end": 200}
    }))
    .unwrap();
    let loaded = GenesisConfigLoaded {
        runtime_config,
        constants,
        genesis_ledger_hash: parse(&fixture, "genesis_ledger_hash"),
        genesis_total_currency: zero(),
        // Not part of the protocol state; only the genesis block proof uses it.
        genesis_producer_stake_proof: v2::MinaBaseSparseLedgerBaseStableV2 {
            indexes: std::iter::empty().collect(),
            depth: 35u64.into(),
            tree: v2::MinaBaseSparseLedgerBaseStableV2Tree::Hash(staking_epoch_ledger_hash.clone()),
        },
        staking_epoch_ledger_hash,
        staking_epoch_total_currency: zero(),
        staking_epoch_seed: parse(&fixture, "staking_epoch_seed"),
        next_epoch_ledger_hash: parse(&fixture, "next_epoch_ledger_hash"),
        next_epoch_total_currency: zero(),
        next_epoch_seed: parse(&fixture, "next_epoch_seed"),
    };

    // Round-trip through the compact format used by relays and web nodes.
    let config = mina_node::transition_frontier::genesis::GenesisConfig::from_bytes(
        serde_json::to_vec(&loaded).unwrap(),
    )
    .unwrap();
    let (_, loaded) = config.load().unwrap();
    assert_eq!(mina_core::constants::stop_slots().slot_tx_end, Some(100));
    assert_eq!(mina_core::constants::stop_slots().slot_chain_end, Some(200));
    let (_, genesis, genesis_hash) = loaded.protocol_states(zero(), zero(), zero()).unwrap();
    let fork_length: u32 = ocaml(&fixture, "fork.blockchain_length")
        .as_u64()
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(
        genesis.body.consensus_state.blockchain_length.as_u32(),
        fork_length + 1
    );
    assert_eq!(
        genesis_hash.to_string(),
        ocaml(&fixture, "chain_id.genesis_state_hash")
            .as_str()
            .unwrap(),
        "the genesis state hash with zero currencies differs from OCaml's"
    );

    let chain_id = ChainId::compute(
        &mainnet::CONSTRAINT_SYSTEM_DIGESTS,
        &genesis_hash,
        &genesis.body.constants,
        PROTOCOL_TRANSACTION_VERSION,
        PROTOCOL_NETWORK_VERSION,
        &v2::UnsignedExtendedUInt32StableV1::from(TX_POOL_MAX_SIZE),
    );
    assert_eq!(
        chain_id.to_hex(),
        "0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886"
    );
    config.initialize_runtime().unwrap();
    let mut conflict = config.runtime_config().unwrap();
    conflict
        .proof
        .as_mut()
        .unwrap()
        .fork
        .as_mut()
        .unwrap()
        .blockchain_length += 1;
    assert!(conflict.initialize().is_err());
}
