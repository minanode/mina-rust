//! The compiled devnet network config against Mesa devnet values read from
//! OCaml 4.0.2:
//!
//! - the six constraint system digests, against the chain id input traced
//!   with gdb from `mina internal chain-id` (`scripts/ocaml-chain-id-trace.sh`);
//! - the fork constants and the constraint constants that OCaml prints for
//!   the devnet profile and config;
//! - the genesis state hash of Mesa devnet, carried by every Mesa devnet
//!   block, computed with the compiled fork constants from the ledger hashes,
//!   total currencies and seeds of the Mesa devnet config;
//! - the genesis state hash with zero total currencies, which the chain id
//!   hashes, and the chain id the genesis reducer computes from it;
//! - the embedded genesis config (`DEVNET_CONFIG`), against the same values,
//!   and the genesis state hash it gives;
//! - the seeds, against the config's `peer_list_url`.
//!
//! The network config is global, so these tests live in their own test binary,
//! which uses devnet only.

use mina_core::{
    constants::{
        constraint_constants, PROTOCOL_CONSTANTS, PROTOCOL_NETWORK_VERSION,
        PROTOCOL_TRANSACTION_VERSION, TX_POOL_MAX_SIZE,
    },
    ChainId, NetworkConfig, DEVNET_CHAIN_ID,
};
use mina_curves::pasta::Fp;
use mina_node::transition_frontier::genesis::GenesisConfigLoaded;
use mina_p2p_messages::v2;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

/// `mina internal chain-id --from-config-hashes-only` with `MINA_PROFILE=devnet`
/// and the 4.0.2 devnet `config_84b8d969.json`.
const DEVNET_MESA_CHAIN_ID: &str =
    "ebfce0d570bc22eb041e1a7b0a46bbc3ed5d0a1030ef2ab5e36eef908b93eba8";

/// The six digests in the order the chain id hashes them, from the chain id
/// input traced with gdb (`digestif_blake2b_update`). The first five equal
/// `mina advanced constraint-system-digests` with the devnet profile; the
/// blockchain-step one is the fork-aware digest that command does not print
/// (it prints 9ea67872932cb16dd03c1654b7b83b76).
const DEVNET_MESA_DIGESTS: [&str; 6] = [
    // transaction-merge
    "d71089b3a1669535999e8f181cd59afc",
    // transaction-base
    "3bf6bb8a97665fe7a9df6fc146e4f942",
    // zkapp-opt_signed-opt_signed
    "614aec09ed5e4068f46d010f0070226b",
    // zkapp-opt_signed
    "0fe3381f501f432744727c296be464b0",
    // zkapp-proved
    "cad581432831f10fee99161532504937",
    // blockchain-step (fork-aware)
    "c321d649f132295917889ddf751731d0",
];

/// Genesis state hash with all total currencies zero, as hashed into the chain
/// id (from the gdb trace).
const CHAIN_ID_GENESIS_STATE_HASH: &str = "3NL4SkiA4eULVQWPGe4LVBn4Goixk3icTD2886iT4y23HRrTPrwN";

/// The real Mesa devnet genesis state hash: `genesis_state_hash` of every Mesa
/// devnet block, and the root the OCaml devnet daemon starts from.
const GENESIS_STATE_HASH: &str = "3NLT7n4LiVo6U4LXr9BjCEp9712fP61uXkRC6hnRnB682A8f4HrJ";

/// The Mesa devnet hard fork point, from `proof.fork` in `config_84b8d969.json`.
const FORK_STATE_HASH: &str = "3NLYmfj4U9Fbbvruz3QJj2j8WJyjhGtq4LgYvo7oW1WZm16uEKib";
const FORK_BLOCKCHAIN_LENGTH: u32 = 545433;
const FORK_GLOBAL_SLOT_SINCE_GENESIS: u32 = 859560;

/// `https://bootnodes.minaprotocol.com/networks/devnet.txt`, the config's
/// `peer_list_url`, as fetched on 2026-10-20.
const BOOTNODES_DEVNET_TXT: &str = "\
/dns4/seed-1.devnet.gcp.o1test.net/tcp/10003/p2p/12D3KooWAdgYL6hv18M3iDBdaK1dRygPivSfAfBNDzie6YqydVbs
/dns4/seed-2.devnet.gcp.o1test.net/tcp/32012/p2p/12D3KooWLjs54xHzVmMmGYb7W5RVibqbwD1co7M2ZMfPgPm7iAag
/dns4/seed-3.devnet.gcp.o1test.net/tcp/32013/p2p/12D3KooWEiGVAFC7curXWXiGZyMWnZK9h8BKr88U8D5PKV3dXciv
";

fn devnet() -> &'static NetworkConfig {
    let _ = NetworkConfig::init("devnet");
    let config = NetworkConfig::global();
    assert_eq!(config.name, "devnet");
    config
}

fn constants() -> v2::MinaBaseProtocolConstantsCheckedValueStableV1 {
    let mut constants = PROTOCOL_CONSTANTS.clone();
    constants.genesis_state_timestamp = OffsetDateTime::parse("2026-08-19T18:00:00Z", &Rfc3339)
        .unwrap()
        .into();
    constants
}

fn amount(nanomina: u64) -> v2::CurrencyAmountStableV1 {
    v2::CurrencyAmountStableV1(nanomina.into())
}

#[test]
fn devnet_digests_are_the_traced_ones() {
    let digests: Vec<String> = devnet()
        .constraint_system_digests
        .iter()
        .map(hex::encode)
        .collect();
    assert_eq!(digests, DEVNET_MESA_DIGESTS);
}

#[test]
fn devnet_chain_id_from_traced_inputs() {
    let chain_id = ChainId::compute(
        devnet().constraint_system_digests,
        &CHAIN_ID_GENESIS_STATE_HASH.parse().unwrap(),
        &constants(),
        PROTOCOL_TRANSACTION_VERSION,
        PROTOCOL_NETWORK_VERSION,
        &v2::UnsignedExtendedUInt32StableV1::from(TX_POOL_MAX_SIZE),
    );
    assert_eq!(chain_id.to_hex(), DEVNET_MESA_CHAIN_ID);
    assert_eq!(chain_id, DEVNET_CHAIN_ID);
}

#[test]
fn devnet_constraint_constants_are_mesa() {
    devnet();
    let constants = constraint_constants();
    // `mina advanced compile-time-constants` with MINA_PROFILE=devnet.
    assert_eq!(constants.block_window_duration_ms, 90000);
    assert_eq!(constants.coinbase_amount, 360_000_000_000);

    let fork = constants.fork.as_ref().expect("devnet has a fork point");
    let fork_state_hash: v2::StateHash = FORK_STATE_HASH.parse().unwrap();
    let fork_state_hash: Fp = fork_state_hash.to_field().unwrap();
    assert_eq!(fork.state_hash, fork_state_hash);
    assert_eq!(fork.blockchain_length, FORK_BLOCKCHAIN_LENGTH);
    assert_eq!(
        fork.global_slot_since_genesis,
        FORK_GLOBAL_SLOT_SINCE_GENESIS
    );
}

/// The Mesa devnet genesis from the OCaml config and the first Mesa blocks.
fn mesa_genesis() -> GenesisConfigLoaded {
    let staking_epoch_ledger_hash: v2::LedgerHash =
        "jwbFn9V1eyDa5AgLkHAwsVffHSjLLhfw8WpKYM6bJiKXb5Uhbti"
            .parse()
            .unwrap();
    GenesisConfigLoaded {
        runtime_config: Default::default(),
        constants: constants(),
        genesis_ledger_hash: "jwrCSCRNpNx3ZBh6D4bUAnZtqPacKXXe6veGC9kQfHrxWqmEbJS"
            .parse()
            .unwrap(),
        // Not part of the protocol state; only the genesis block proof uses it.
        genesis_producer_stake_proof: v2::MinaBaseSparseLedgerBaseStableV2 {
            indexes: std::iter::empty().collect(),
            depth: 35u64.into(),
            tree: v2::MinaBaseSparseLedgerBaseStableV2Tree::Hash(staking_epoch_ledger_hash.clone()),
        },
        // Read from the first Mesa devnet blocks (545435), whose consensus
        // state still carries the genesis values.
        genesis_total_currency: amount(1588907694000001000),
        staking_epoch_ledger_hash,
        staking_epoch_total_currency: amount(1582881647000001000),
        staking_epoch_seed: "2vamERQZWT1miAnrxLJsjPmZmoCSemrz9JugjN8weQJeqhU7ZD38"
            .parse()
            .unwrap(),
        next_epoch_ledger_hash: "jxUYRdFcuDDMyEtRXkAEo74EFpFSAjcVf6pBX9j56siW7rFZFRq"
            .parse()
            .unwrap(),
        next_epoch_total_currency: amount(1585748525000001000),
        next_epoch_seed: "2vbVSm3AnPnhSaTBdQeU65gBAJmFiG6hHixXBQjeQ9MEBrtepDiL"
            .parse()
            .unwrap(),
    }
}

#[test]
fn devnet_mesa_genesis_and_chain_id() {
    let config = devnet();
    let mut loaded = mesa_genesis();
    loaded.runtime_config = serde_json::from_value(serde_json::json!({
        "proof": {"fork": {"state_hash": FORK_STATE_HASH, "blockchain_length": FORK_BLOCKCHAIN_LENGTH,
            "global_slot_since_genesis": FORK_GLOBAL_SLOT_SINCE_GENESIS},
            "block_window_duration_ms": 90000, "coinbase_amount": "360"}
    })).unwrap();
    let genesis_config = mina_node::transition_frontier::genesis::GenesisConfig::from_bytes(
        serde_json::to_vec(&loaded).unwrap(),
    )
    .unwrap();
    let (_, loaded) = genesis_config.load().unwrap();

    // Runtime metadata agrees with the compiled devnet fork and OCaml.
    let (_, genesis, genesis_hash) = loaded
        .protocol_states(
            loaded.genesis_total_currency.clone(),
            loaded.staking_epoch_total_currency.clone(),
            loaded.next_epoch_total_currency.clone(),
        )
        .unwrap();
    assert_eq!(genesis_hash.to_string(), GENESIS_STATE_HASH);
    let consensus = &genesis.body.consensus_state;
    assert_eq!(
        consensus.blockchain_length.as_u32(),
        FORK_BLOCKCHAIN_LENGTH + 1
    );
    assert_eq!(
        consensus.global_slot_since_genesis.as_u32(),
        FORK_GLOBAL_SLOT_SINCE_GENESIS
    );
    assert_eq!(genesis.previous_state_hash.to_string(), FORK_STATE_HASH);

    let (_, _, chain_id_genesis_hash) = loaded
        .protocol_states(amount(0), amount(0), amount(0))
        .unwrap();
    assert_eq!(
        chain_id_genesis_hash.to_string(),
        CHAIN_ID_GENESIS_STATE_HASH
    );

    // As the genesis reducer computes it.
    let chain_id = ChainId::compute(
        config.constraint_system_digests,
        &chain_id_genesis_hash,
        &genesis.body.constants,
        PROTOCOL_TRANSACTION_VERSION,
        PROTOCOL_NETWORK_VERSION,
        &v2::UnsignedExtendedUInt32StableV1::from(TX_POOL_MAX_SIZE),
    );
    assert_eq!(chain_id, DEVNET_CHAIN_ID);
}

/// The embedded devnet genesis config, which `ledger-tool` writes from the
/// RocksDB ledger tarballs of the Mesa devnet config, has the OCaml values and
/// gives the Mesa devnet genesis block.
#[test]
fn embedded_devnet_config_is_the_mesa_genesis() {
    devnet();
    let (masks, loaded) = mina_node::config::DEVNET_CONFIG.load().unwrap();
    assert!(masks.is_empty(), "it has no ledgers");

    // Every field but the stake proof, which `mesa_genesis` stands in for
    // (`config::tests::devnet_config` checks it).
    let mut expected = mesa_genesis();
    expected.genesis_producer_stake_proof = loaded.genesis_producer_stake_proof.clone();
    assert_eq!(
        serde_json::to_value(&loaded).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );

    let (_, genesis, genesis_hash) = loaded
        .protocol_states(
            loaded.genesis_total_currency.clone(),
            loaded.staking_epoch_total_currency.clone(),
            loaded.next_epoch_total_currency.clone(),
        )
        .unwrap();
    assert_eq!(genesis_hash.to_string(), GENESIS_STATE_HASH);
    assert_eq!(
        genesis.body.consensus_state.blockchain_length.as_u32(),
        FORK_BLOCKCHAIN_LENGTH + 1
    );
}

#[test]
fn devnet_seeds_are_the_bootnodes_list() {
    let bootnodes: Vec<&str> = BOOTNODES_DEVNET_TXT.lines().collect();
    assert_eq!(devnet().default_peers, bootnodes);
    assert_eq!(mina_node::p2p::DEVNET_SEEDS, bootnodes.as_slice());
}
