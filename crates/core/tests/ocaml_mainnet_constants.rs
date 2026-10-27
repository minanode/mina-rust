//! P11d: the mainnet protocol constants mina-rust hard-codes, against OCaml.
//!
//! `tests/files/ocaml/mainnet-4.0.2-constants.json` holds OCaml 4.0.2's
//! mainnet values, each read one or more ways: from the `mina` binary, the
//! chain id input traced with gdb, the packaged runtime config, the OCaml
//! source at the binary's commit, a live OCaml node's GraphQL and a Mesa
//! block. `scripts/ocaml-constants-fixture.py` regenerates it.
//!
//! Every observation of a constant must equal our value. Every constant in
//! the fixture must be accounted for below: compared here, compared in
//! mina-tree or mina-node, used as an input, or listed as not checked with
//! the reason. Devnet is not covered yet: P1 is changing its constants.

use std::collections::{BTreeMap, BTreeSet};

use mina_core::{
    consensus::ConsensusConstants,
    constants::{
        checkpoint_window_size_in_slots, slots_per_window, CHECKPOINTS_PER_YEAR,
        PROTOCOL_CONSTANTS, PROTOCOL_NETWORK_VERSION, PROTOCOL_TRANSACTION_VERSION,
        PROTOCOL_VERSION, TX_POOL_MAX_SIZE,
    },
    network::{mainnet, NetworkId},
    ChainId, NetworkConfig, MAINNET_CHAIN_ID,
};
use mina_p2p_messages::v2;
use serde::Deserialize;
use serde_json::{json, Value};

const FIXTURE: &str = include_str!("../../../tests/files/ocaml/mainnet-4.0.2-constants.json");

/// The digests in `mainnet::CONSTRAINT_SYSTEM_DIGESTS`, in its order.
const DIGEST_NAMES: [&str; 6] = [
    "transaction-merge",
    "transaction-base",
    "zkapp-opt_signed-opt_signed",
    "zkapp-opt_signed",
    "zkapp-proved",
    "blockchain-step",
];

/// Compared only in `crates/ledger/tests/ocaml_mainnet_constants.rs`, which
/// compares `txpool_max_size` too.
const CHECKED_IN_MINA_TREE: &[&str] = &[
    "max_action_elements",
    "max_event_elements",
    "max_zkapp_segment_per_transaction",
    "minimum_user_command_fee",
    "zkapp_cmd_limit_hardcap",
];

/// Used in `crates/node/tests/ocaml_mainnet_constants.rs`: the genesis inputs
/// it builds the genesis state from, with the compiled fork constants, to
/// compare its hash with `chain_id.genesis_state_hash`, and the most levels
/// OCaml answers in a ledger sync query.
const CHECKED_IN_MINA_NODE: &[&str] = &[
    "genesis_ledger_hash",
    "next_epoch_ledger_hash",
    "next_epoch_seed",
    "staking_epoch_ledger_hash",
    "staking_epoch_seed",
    "sync_ledger_max_subtree_depth",
];

/// Inputs this test recomputes the chain id from, with our constants.
const INPUTS: &[&str] = &["chain_id.genesis_state_hash", "genesis_state_timestamp_ms"];

const NOT_CHECKED: &[(&str, &str)] = &[
    (
        "chain_id.genesis_constants_hash",
        "a hash of k, slots per epoch, slots per sub-window, delta, pool size and the genesis \
         timestamp, each compared on its own and together through the chain id",
    ),
    (
        "digest.blockchain-step-without-fork",
        "what `constraint-system-digests` prints: compiled without the fork constants, so no \
         chain id commits to it",
    ),
    (
        "zkapp_cmd_limit",
        "a block producer's limit (`create_diff`), not a validation rule; mina-rust produces \
         no Mesa blocks yet (P4)",
    ),
    (
        "default_transaction_fee",
        "a client default, not a protocol constant",
    ),
    (
        "default_snark_worker_fee",
        "a SNARK worker default, not a protocol constant",
    ),
    (
        "sync_ledger_default_subtree_depth",
        "OCaml's own query depth, not a limit: mina-node checks that its \
         `SYNC_LEDGER_QUERY_DEPTH` stays within `sync_ledger_max_subtree_depth`",
    ),
    (
        "vrf_threshold_f",
        "a local in mina-vrf's `Threshold::new` (3/4 there too)",
    ),
];

#[derive(Deserialize)]
struct Fixture {
    ocaml_commit: String,
    profile: String,
    constants: BTreeMap<String, Vec<Observation>>,
}

#[derive(Deserialize)]
struct Observation {
    value: Value,
    how: String,
}

fn fixture() -> Fixture {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("fixture should parse");
    assert_eq!(fixture.profile, "mainnet");
    fixture
}

/// The single value of a constant, after checking that OCaml's sources agree.
fn ocaml<'a>(fixture: &'a Fixture, name: &str) -> &'a Value {
    let observations = fixture
        .constants
        .get(name)
        .unwrap_or_else(|| panic!("{name} is missing from the fixture: regenerate it"));
    let first = &observations[0].value;
    for observation in observations {
        assert_eq!(
            &observation.value, first,
            "OCaml's sources disagree on {name}: {} ({}) vs {first}",
            observation.value, observation.how
        );
    }
    first
}

fn init_mainnet() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| NetworkConfig::init("mainnet").expect("mainnet network config"));
}

/// The protocol constants with the Mesa genesis timestamp, as a node started
/// from the Mesa genesis config has them.
fn protocol_constants(fixture: &Fixture) -> v2::MinaBaseProtocolConstantsCheckedValueStableV1 {
    let timestamp = ocaml(fixture, "genesis_state_timestamp_ms")
        .as_u64()
        .unwrap();
    let mut constants = PROTOCOL_CONSTANTS.clone();
    constants.genesis_state_timestamp = v2::BlockTimeTimeStableV1(timestamp.into());
    constants
}

/// Our value of each constant: (fixture name, where in mina-rust, value).
/// A name may appear more than once when several places hard-code it.
fn ours(fixture: &Fixture) -> Vec<(String, &'static str, Value)> {
    init_mainnet();
    let cc = &mainnet::CONSTRAINT_CONSTANTS;
    let pc = &PROTOCOL_CONSTANTS;
    let consensus = ConsensusConstants::create(cc, &protocol_constants(fixture));
    let fork = cc.fork.as_ref().expect("mainnet has fork constants");
    let config = NetworkConfig::global();
    assert_eq!(
        config.name,
        mainnet::NAME,
        "the network config is mainnet's"
    );

    let ours: Vec<(&str, &str, Value)> = vec![
        // Protocol constants (`PROTOCOL_CONSTANTS`, p2p-messages
        // `v2/manual.rs`), the defaults behind every genesis config.
        ("k", "PROTOCOL_CONSTANTS.k", json!(pc.k.as_u32())),
        (
            "delta",
            "PROTOCOL_CONSTANTS.delta",
            json!(pc.delta.as_u32()),
        ),
        (
            "slots_per_epoch",
            "PROTOCOL_CONSTANTS.slots_per_epoch",
            json!(pc.slots_per_epoch.as_u32()),
        ),
        (
            "slots_per_sub_window",
            "PROTOCOL_CONSTANTS.slots_per_sub_window",
            json!(pc.slots_per_sub_window.as_u32()),
        ),
        (
            "grace_period_slots",
            "PROTOCOL_CONSTANTS.grace_period_slots",
            json!(pc.grace_period_slots.as_u32()),
        ),
        (
            "default_genesis_state_timestamp_ms",
            "PROTOCOL_CONSTANTS.genesis_state_timestamp",
            json!(pc.genesis_state_timestamp.as_u64()),
        ),
        // Constraint constants (`network.rs`, `mainnet::CONSTRAINT_CONSTANTS`).
        (
            "sub_windows_per_window",
            "mainnet::CONSTRAINT_CONSTANTS.sub_windows_per_window",
            json!(cc.sub_windows_per_window),
        ),
        (
            "ledger_depth",
            "mainnet::CONSTRAINT_CONSTANTS.ledger_depth",
            json!(cc.ledger_depth),
        ),
        (
            "work_delay",
            "mainnet::CONSTRAINT_CONSTANTS.work_delay",
            json!(cc.work_delay),
        ),
        (
            "block_window_duration_ms",
            "mainnet::CONSTRAINT_CONSTANTS.block_window_duration_ms",
            json!(cc.block_window_duration_ms),
        ),
        (
            "transaction_capacity_log_2",
            "mainnet::CONSTRAINT_CONSTANTS.transaction_capacity_log_2",
            json!(cc.transaction_capacity_log_2),
        ),
        (
            "pending_coinbase_depth",
            "mainnet::CONSTRAINT_CONSTANTS.pending_coinbase_depth",
            json!(cc.pending_coinbase_depth),
        ),
        (
            "coinbase_amount",
            "mainnet::CONSTRAINT_CONSTANTS.coinbase_amount",
            json!(cc.coinbase_amount),
        ),
        (
            "supercharged_coinbase_factor",
            "mainnet::CONSTRAINT_CONSTANTS.supercharged_coinbase_factor",
            json!(cc.supercharged_coinbase_factor),
        ),
        (
            "account_creation_fee",
            "mainnet::CONSTRAINT_CONSTANTS.account_creation_fee",
            json!(cc.account_creation_fee),
        ),
        (
            "fork.state_hash",
            "mainnet::CONSTRAINT_CONSTANTS.fork.state_hash",
            json!(v2::StateHash::from_fp(fork.state_hash).to_string()),
        ),
        (
            "fork.blockchain_length",
            "mainnet::CONSTRAINT_CONSTANTS.fork.blockchain_length",
            json!(fork.blockchain_length),
        ),
        (
            "fork.global_slot_since_genesis",
            "mainnet::CONSTRAINT_CONSTANTS.fork.global_slot_since_genesis",
            json!(fork.global_slot_since_genesis),
        ),
        // Consensus constants derived from them (`consensus.rs`,
        // `constants.rs`).
        (
            "slots_per_window",
            "ConsensusConstants::create(..).slots_per_window",
            json!(consensus.slots_per_window),
        ),
        (
            "slots_per_window",
            "constants::slots_per_window(&PROTOCOL_CONSTANTS)",
            json!(slots_per_window(pc)),
        ),
        (
            "epoch_duration_ms",
            "ConsensusConstants::create(..).epoch_duration",
            json!(consensus.epoch_duration),
        ),
        (
            "delta_duration_ms",
            "ConsensusConstants::create(..).delta_duration",
            json!(consensus.delta_duration),
        ),
        (
            "checkpoints_per_year",
            "constants::CHECKPOINTS_PER_YEAR",
            json!(CHECKPOINTS_PER_YEAR),
        ),
        (
            "checkpoint_window_size_in_slots",
            "ConsensusConstants::create(..).checkpoint_window_size_in_slots",
            json!(consensus.checkpoint_window_size_in_slots),
        ),
        (
            "checkpoint_window_size_in_slots",
            "constants::checkpoint_window_size_in_slots()",
            json!(checkpoint_window_size_in_slots()),
        ),
        // Versions and the pool size (`constants.rs`).
        (
            "protocol_version.transaction",
            "constants::PROTOCOL_VERSION.transaction",
            json!(PROTOCOL_VERSION.transaction.as_u64()),
        ),
        (
            "protocol_version.network",
            "constants::PROTOCOL_VERSION.network",
            json!(PROTOCOL_VERSION.network.as_u64()),
        ),
        (
            "protocol_version.patch",
            "constants::PROTOCOL_VERSION.patch",
            json!(PROTOCOL_VERSION.patch.as_u64()),
        ),
        (
            "protocol_version.transaction",
            "constants::PROTOCOL_TRANSACTION_VERSION (chain id)",
            json!(PROTOCOL_TRANSACTION_VERSION),
        ),
        (
            "protocol_version.network",
            "constants::PROTOCOL_NETWORK_VERSION (chain id)",
            json!(PROTOCOL_NETWORK_VERSION),
        ),
        (
            "txpool_max_size",
            "constants::TX_POOL_MAX_SIZE",
            json!(TX_POOL_MAX_SIZE),
        ),
        // The network (`network.rs`).
        ("network_name", "mainnet::NAME", json!(mainnet::NAME)),
        ("network_name", "NetworkConfig::name", json!(config.name)),
        (
            "signature_kind",
            "NetworkConfig::network_id",
            json!(match config.network_id {
                NetworkId::MAINNET => "mainnet",
                NetworkId::TESTNET => "testnet",
            }),
        ),
        (
            "network_id_byte",
            "NetworkConfig::network_id",
            json!(config.network_id.clone() as u8),
        ),
        (
            "signature_prefix",
            "mainnet::SIGNATURE_PREFIX",
            json!(mainnet::SIGNATURE_PREFIX),
        ),
        (
            "signature_prefix",
            "NetworkConfig::signature_prefix",
            json!(param_string(config.signature_prefix)),
        ),
        (
            "zkapp_body_hash_prefix",
            "mainnet::ACCOUNT_UPDATE_HASH_PARAM",
            json!(mainnet::ACCOUNT_UPDATE_HASH_PARAM),
        ),
        (
            "zkapp_body_hash_prefix",
            "NetworkConfig::account_update_hash_param",
            json!(param_string(config.account_update_hash_param)),
        ),
        // The chain id (`chain_id.rs`, `network.rs`).
        (
            "digest_order",
            "mainnet::CONSTRAINT_SYSTEM_DIGESTS (order)",
            json!(DIGEST_NAMES),
        ),
        (
            "chain_id",
            "MAINNET_CHAIN_ID",
            json!(MAINNET_CHAIN_ID.to_hex()),
        ),
        (
            "chain_id",
            "ChainId::compute(our digests, constants, versions and pool size; OCaml's genesis \
             state hash and timestamp)",
            json!(recomputed_chain_id(fixture).to_hex()),
        ),
    ];
    let mut ours: Vec<_> = ours
        .into_iter()
        .map(|(name, site, value)| (name.to_owned(), site, value))
        .collect();
    assert_eq!(mainnet::CONSTRAINT_SYSTEM_DIGESTS.len(), DIGEST_NAMES.len());
    for (name, digest) in DIGEST_NAMES.iter().zip(mainnet::CONSTRAINT_SYSTEM_DIGESTS) {
        ours.push((
            format!("digest.{name}"),
            "mainnet::CONSTRAINT_SYSTEM_DIGESTS",
            json!(hex::encode(digest)),
        ));
    }
    ours
}

/// The string a Poseidon parameter was made from. `LazyParam` keeps it but
/// only shows it through `Debug`.
fn param_string(param: &poseidon::hash::LazyParam) -> String {
    let debug = format!("{param:?}");
    let start = debug
        .find("string: \"")
        .unwrap_or_else(|| panic!("no string in {debug}"))
        + "string: \"".len();
    let end = start + debug[start..].find('"').unwrap();
    debug[start..end].to_owned()
}

fn recomputed_chain_id(fixture: &Fixture) -> ChainId {
    let genesis_state_hash: v2::StateHash = ocaml(fixture, "chain_id.genesis_state_hash")
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    ChainId::compute(
        &mainnet::CONSTRAINT_SYSTEM_DIGESTS,
        &genesis_state_hash,
        &protocol_constants(fixture),
        PROTOCOL_TRANSACTION_VERSION,
        PROTOCOL_NETWORK_VERSION,
        &v2::UnsignedExtendedUInt32StableV1::from(TX_POOL_MAX_SIZE),
    )
}

#[test]
fn mainnet_constants_match_ocaml() {
    let fixture = fixture();
    let mut mismatches = Vec::new();
    let mut report = format!(
        "mainnet constants against OCaml {}:\n",
        &fixture.ocaml_commit[..8]
    );
    for (name, site, value) in ours(&fixture) {
        let observations = fixture
            .constants
            .get(&name)
            .unwrap_or_else(|| panic!("{name} is missing from the fixture: regenerate it"));
        for observation in observations {
            let ok = observation.value == value;
            report += &format!(
                "  {} {name} = {value} ({site}); OCaml {} from {}\n",
                if ok { "ok      " } else { "MISMATCH" },
                observation.value,
                observation.how,
            );
            if !ok {
                mismatches.push(format!(
                    "{name}: ours {value} ({site}), OCaml {} ({})",
                    observation.value, observation.how
                ));
            }
        }
    }
    println!("{report}");
    assert!(
        mismatches.is_empty(),
        "{} mismatches with OCaml:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

/// Every constant in the fixture is compared somewhere, used, or listed as
/// not checked with a reason, and OCaml's sources agree on each.
#[test]
fn every_ocaml_constant_is_accounted_for() {
    let fixture = fixture();
    let compared = ours(&fixture);
    let mut accounted: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, _, _) in &compared {
        accounted
            .entry(name.as_str())
            .or_default()
            .push("compared here");
    }
    for name in CHECKED_IN_MINA_TREE {
        accounted.entry(*name).or_default().push("mina-tree");
    }
    for name in CHECKED_IN_MINA_NODE {
        accounted.entry(*name).or_default().push("mina-node");
    }
    for name in INPUTS {
        accounted.entry(*name).or_default().push("input");
    }
    for (name, _) in NOT_CHECKED {
        accounted.entry(*name).or_default().push("not checked");
    }
    let in_fixture: BTreeSet<&str> = fixture.constants.keys().map(String::as_str).collect();
    let missing: Vec<_> = accounted
        .keys()
        .filter(|name| !in_fixture.contains(*name))
        .collect();
    let unaccounted: Vec<_> = in_fixture
        .iter()
        .filter(|name| !accounted.contains_key(*name))
        .collect();
    let twice: Vec<_> = accounted
        .iter()
        .filter(|(_, how)| {
            let kinds: BTreeSet<_> = how.iter().collect();
            kinds.len() > 1
        })
        .collect();
    assert!(missing.is_empty(), "expected in the fixture: {missing:?}");
    assert!(
        unaccounted.is_empty(),
        "in the fixture but neither compared, used nor listed as not checked: {unaccounted:?}"
    );
    assert!(twice.is_empty(), "accounted for more than once: {twice:?}");
    for name in in_fixture {
        ocaml(&fixture, name);
    }
}
