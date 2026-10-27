//! P11d: mina-node's mainnet constants against OCaml 4.0.2, with values from
//! `tests/files/ocaml/mainnet-4.0.2-constants.json`
//! (`scripts/ocaml-constants-fixture.py`; see
//! private qualification evidence). The rest of the fixture is
//! checked by `crates/core/tests/ocaml_mainnet_constants.rs`, whose
//! `CHECKED_IN_MINA_NODE` lists the names used here.
//!
//! The genesis state: `mina internal chain-id` hashes the genesis state hash
//! computed with zero total currencies, which `scripts/ocaml-chain-id-trace.sh`
//! reads with gdb. This builds that state from the genesis inputs of OCaml's
//! packaged runtime config (ledger hashes, seeds, genesis timestamp) and
//! mina-rust's own constants: the fork point in `mainnet::CONSTRAINT_CONSTANTS`
//! (no `set_fork_override`, as for the relay and the web node), the protocol
//! constants, the pending coinbase depth, the genesis producer key and the
//! empty block body hash. Its hash, and the chain id from it, must equal
//! OCaml's.
//!
//! The network config is global, so this lives in its own test binary.

use mina_core::{
    constants::{
        constraint_constants, PROTOCOL_CONSTANTS, PROTOCOL_NETWORK_VERSION,
        PROTOCOL_TRANSACTION_VERSION, TX_POOL_MAX_SIZE,
    },
    network::mainnet,
    ChainId, NetworkConfig,
};
use mina_node::{
    ledger::read::MAX_SERVED_CHILD_HASHES_DEPTH,
    transition_frontier::{
        genesis::GenesisConfigLoaded, sync::ledger::snarked::SYNC_LEDGER_QUERY_DEPTH,
    },
};
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
fn mainnet_mesa_chain_id_genesis_state_hash() {
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
    let loaded = GenesisConfigLoaded {
        runtime_config: Default::default(),
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
        ocaml(&fixture, "chain_id").as_str().unwrap()
    );
}

/// OCaml answers `What_child_hashes` queries up to `sync_ledger_max_subtree_depth`
/// levels deep. Our server must answer as deep, and our queries must not ask
/// deeper. OCaml's own query depth (`sync_ledger_default_subtree_depth`) is
/// its choice, not a limit, so it is not compared.
#[test]
fn sync_ledger_depths_within_ocaml_limit() {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("fixture should parse");
    let max = ocaml(&fixture, "sync_ledger_max_subtree_depth")
        .as_u64()
        .unwrap() as usize;
    assert_eq!(MAX_SERVED_CHILD_HASHES_DEPTH, max);
    assert!(
        SYNC_LEDGER_QUERY_DEPTH <= max,
        "SYNC_LEDGER_QUERY_DEPTH {SYNC_LEDGER_QUERY_DEPTH} is deeper than OCaml answers ({max})"
    );
}
