//! The next hard fork's runtime config in OCaml's JSON layout.
//!
//! - `fork_config.json`: every account inline, as GraphQL `fork_config`
//!   returns it, plus the epoch ledgers' hashes ([`fork_config_parts`]). OCaml's release tools
//!   (`mina-create-genesis`, `mina-hf-create-runtime-config`,
//!   `mina-verify-packaged-fork-config`) take this file, and both nodes load it.
//! - `daemon.json`: the ledgers by hash, the fields of OCaml's
//!   `generate-hardfork-config` output ([`ForkRuntimeConfig`]), without
//!   `s3_data_hash`: that is the SHA3 of OCaml's RocksDB ledger tarballs, which
//!   this export does not write.
//!
//! Field order is OCaml's `Runtime_config.Json_layout` order.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/runtime_config/runtime_config.ml#L384-L536>

use serde::{Deserialize, Serialize};

use crate::rpc::{HardForkInputs, HardForkPoint, RpcHardForkExport};

/// `genesis` of the next chain: only the timestamp. The other protocol
/// constants stay the release's.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ForkGenesis {
    pub genesis_state_timestamp: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ForkProof {
    pub fork: HardForkPoint,
}

/// `ledger` by hash, as `Runtime_config.ledger_of_hashes` writes it.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/runtime_config/runtime_config.ml#L1582-L1591>
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ForkLedger {
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3_data_hash: Option<String>,
    pub add_genesis_winner: bool,
}

/// An epoch ledger by hash, with its seed (`Json_layout.Epoch_data.Data`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ForkEpochLedger {
    pub seed: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3_data_hash: Option<String>,
    pub hash: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ForkEpochData {
    pub staking: ForkEpochLedger,
    pub next: ForkEpochLedger,
}

/// The next chain's runtime config with its ledgers by hash: what
/// `Runtime_config.make_automatic_fork_config` writes.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/runtime_config/runtime_config.ml#L1708-L1743>
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ForkRuntimeConfig {
    pub genesis: ForkGenesis,
    pub proof: ForkProof,
    pub ledger: ForkLedger,
    pub epoch_data: ForkEpochData,
}

impl ForkRuntimeConfig {
    /// The config with the given genesis, staking and next ledger roots.
    pub fn new(inputs: &HardForkInputs, ledger: String, staking: String, next: String) -> Self {
        Self {
            genesis: ForkGenesis {
                genesis_state_timestamp: inputs.genesis_state_timestamp.clone(),
            },
            proof: ForkProof {
                fork: inputs.fork.clone(),
            },
            ledger: ForkLedger {
                hash: ledger,
                s3_data_hash: None,
                // The old chain's ledger already holds the genesis winner.
                add_genesis_winner: false,
            },
            epoch_data: ForkEpochData {
                staking: ForkEpochLedger {
                    seed: inputs.staking_epoch.seed.to_string(),
                    s3_data_hash: None,
                    hash: staking,
                },
                next: ForkEpochLedger {
                    seed: inputs.next_epoch.seed.to_string(),
                    s3_data_hash: None,
                    hash: next,
                },
            },
        }
    }

    /// With the current release's (stable) ledger roots, as OCaml writes in
    /// `fork_validation/legacy/daemon.json`.
    pub fn stable(export: &RpcHardForkExport) -> Self {
        Self::new(
            &export.inputs,
            export.ledger.hash.to_string(),
            export.staking.hash.to_string(),
            export.next.hash.to_string(),
        )
    }

    /// With OCaml 4.0.2's migrated ledger roots, as it writes in the main
    /// `daemon.json` of `generate-hardfork-config`.
    pub fn migrated(export: &RpcHardForkExport) -> Self {
        Self::new(
            &export.inputs,
            export.ledger.migrated_hash.to_string(),
            export.staking.migrated_hash.to_string(),
            export.next.migrated_hash.to_string(),
        )
    }
}

/// The JSON around the three account arrays of `fork_config.json`: the file
/// is `parts[0] ledger parts[1] staking parts[2] next parts[3]`, in the layout
/// of `Runtime_config.make_fork_config` (the GraphQL `fork_config` answer):
/// genesis timestamp, fork point, the genesis ledger's accounts and hash
/// without the genesis winner, and each epoch ledger's accounts and seed.
/// Unlike that answer, each epoch ledger also declares its hash: OCaml 4.0.2
/// compares the two epoch ledgers by their declared hashes and, when both are
/// missing, loads the staking ledger as the next one too.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_ledger_helper/genesis_ledger_helper.ml#L646-L660>
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/runtime_config/runtime_config.ml#L1593-L1667>
pub fn fork_config_parts(export: &RpcHardForkExport) -> [String; 4] {
    let inputs = &export.inputs;
    let genesis = ForkGenesis {
        genesis_state_timestamp: inputs.genesis_state_timestamp.clone(),
    };
    let proof = ForkProof {
        fork: inputs.fork.clone(),
    };
    [
        format!(
            "{{\"genesis\":{},\"proof\":{},\"ledger\":{{\"accounts\":",
            json(&genesis),
            json(&proof)
        ),
        format!(
            ",\"hash\":{},\"add_genesis_winner\":false}},\"epoch_data\":{{\"staking\":{{\"accounts\":",
            json(&export.ledger.hash.to_string())
        ),
        format!(
            ",\"seed\":{},\"hash\":{}}},\"next\":{{\"accounts\":",
            json(&inputs.staking_epoch.seed.to_string()),
            json(&export.staking.hash.to_string())
        ),
        format!(
            ",\"seed\":{},\"hash\":{}}}}}}}\n",
            json(&inputs.next_epoch.seed.to_string()),
            json(&export.next.hash.to_string())
        ),
    ]
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("serializing to a String cannot fail")
}

/// One exported ledger, without its accounts.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HardForkLedgerSummary {
    pub hash: String,
    pub migrated_hash: String,
    pub num_accounts: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HardForkLedgerSummaries {
    pub ledger: HardForkLedgerSummary,
    pub staking: HardForkLedgerSummary,
    pub next: HardForkLedgerSummary,
}

/// The first line of the node's `GET /hardfork/config` answer, before
/// `fork_config.json`: the inputs, the ledger roots and both hash-only
/// configs.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HardForkManifest {
    pub format: String,
    pub chain_id: Option<String>,
    pub inputs: HardForkInputs,
    pub ledgers: HardForkLedgerSummaries,
    pub daemon: ForkRuntimeConfig,
    pub daemon_migrated: ForkRuntimeConfig,
}

impl HardForkManifest {
    pub const FORMAT: &'static str = "mina-rust-hardfork-export/1";

    pub fn of_export(export: &RpcHardForkExport) -> Self {
        let summary = |l: &crate::rpc::HardForkLedger| HardForkLedgerSummary {
            hash: l.hash.to_string(),
            migrated_hash: l.migrated_hash.to_string(),
            num_accounts: l.num_accounts,
        };
        Self {
            format: Self::FORMAT.to_owned(),
            chain_id: export.chain_id.clone(),
            inputs: export.inputs.clone(),
            ledgers: HardForkLedgerSummaries {
                ledger: summary(&export.ledger),
                staking: summary(&export.staking),
                next: summary(&export.next),
            },
            daemon: ForkRuntimeConfig::stable(export),
            daemon_migrated: ForkRuntimeConfig::migrated(export),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use mina_core::constants::StopSlots;
    use mina_p2p_messages::v2::{EpochSeed, LedgerHash, StateHash};

    use super::*;
    use crate::rpc::{HardForkBlock, HardForkBlockSpec, HardForkEpochInput, HardForkLedger};

    /// OCaml 4.0.2's `mina ledger export` sample and its `mina ledger hash`.
    const SAMPLE: &str = include_str!("testdata/ocaml-ledger-export-sample.json");
    const SAMPLE_HASH: &str = "jwUATMQxpP7A3xwoW7uEwS3M83zGi3prDUPSqEMpqUzvssSMaW3";
    const FORK_STATE_HASH: &str = "3NLYmfj4U9Fbbvruz3QJj2j8WJyjhGtq4LgYvo7oW1WZm16uEKib";

    fn export() -> RpcHardForkExport {
        let hash = LedgerHash::from_str(SAMPLE_HASH).unwrap();
        let block = HardForkBlock {
            state_hash: StateHash::from_str(FORK_STATE_HASH).unwrap(),
            blockchain_length: 545433,
            curr_global_slot_since_hard_fork: 413539,
            global_slot_since_genesis: 859399,
            epoch: 57,
            staged_ledger_hash: hash.clone(),
            snarked_ledger_hash: hash.clone(),
        };
        let ledger = HardForkLedger {
            hash: hash.clone(),
            migrated_hash: hash.clone(),
            num_accounts: 17,
            accounts_json: SAMPLE.to_owned(),
        };
        RpcHardForkExport {
            inputs: crate::rpc::HardForkInputs {
                spec: HardForkBlockSpec::StopSlot,
                stop_slots: StopSlots::default(),
                best_tip: block.clone(),
                stop_reached: true,
                fork_block: block,
                genesis_slot_since_hard_fork: 413700,
                fork: HardForkPoint {
                    state_hash: StateHash::from_str(FORK_STATE_HASH).unwrap(),
                    blockchain_length: 545433,
                    global_slot_since_genesis: 859560,
                },
                genesis_state_timestamp: "2026-08-19T18:00:00.000000Z".to_owned(),
                genesis_state_timestamp_ms: 1_787_162_400_000,
                staking_epoch: HardForkEpochInput {
                    ledger_hash: hash.clone(),
                    seed: EpochSeed::zero(),
                },
                next_epoch: HardForkEpochInput {
                    ledger_hash: hash,
                    seed: EpochSeed::zero(),
                },
            },
            chain_id: None,
            ledger: ledger.clone(),
            staking: ledger.clone(),
            next: ledger,
        }
    }

    fn fork_config(export: &RpcHardForkExport) -> String {
        let [p0, p1, p2, p3] = fork_config_parts(export);
        [
            p0.as_str(),
            &export.ledger.accounts_json,
            &p1,
            &export.staking.accounts_json,
            &p2,
            &export.next.accounts_json,
            &p3,
        ]
        .concat()
    }

    /// The layout of GraphQL `fork_config` (`Runtime_config.make_fork_config`
    /// through `Json_layout`): no `daemon`, no other genesis constants; the
    /// epoch ledgers add their hashes, which `Json_layout.Epoch_data.Data`
    /// allows after the seed.
    #[test]
    fn fork_config_has_ocamls_layout() {
        let export = export();
        let text = fork_config(&export);
        let prefix = format!(
            "{{\"genesis\":{{\"genesis_state_timestamp\":\"2026-08-19T18:00:00.000000Z\"}},\
             \"proof\":{{\"fork\":{{\"state_hash\":\"{FORK_STATE_HASH}\",\
             \"blockchain_length\":545433,\"global_slot_since_genesis\":859560}}}},\
             \"ledger\":{{\"accounts\":["
        );
        assert!(text.starts_with(&prefix), "{}", &text[..300]);
        assert!(text.contains(&format!(
            "]\n,\"hash\":\"{SAMPLE_HASH}\",\"add_genesis_winner\":false}},\
             \"epoch_data\":{{\"staking\":{{\"accounts\":["
        )));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let keys = |v: &serde_json::Value| {
            let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        assert_eq!(keys(&value), ["epoch_data", "genesis", "ledger", "proof"]);
        assert_eq!(
            keys(&value["ledger"]),
            ["accounts", "add_genesis_winner", "hash"]
        );
        assert_eq!(
            keys(&value["epoch_data"]["staking"]),
            ["accounts", "hash", "seed"]
        );
        assert_eq!(
            keys(&value["epoch_data"]["next"]),
            ["accounts", "hash", "seed"]
        );
        assert!(text.ends_with(&format!(
            "]\n,\"seed\":\"{seed}\",\"hash\":\"{SAMPLE_HASH}\"}}}}}}\n",
            seed = EpochSeed::zero()
        )));
        assert_eq!(value["ledger"]["accounts"].as_array().unwrap().len(), 17);
    }

    /// Our loader reads the file back, with the fork point and no genesis
    /// winner added.
    #[test]
    fn fork_config_loads_as_daemon_json() {
        let export = export();
        let config: crate::daemon_json::DaemonJson =
            serde_json::from_str(&fork_config(&export)).unwrap();
        let ledger = config.ledger.unwrap();
        assert_eq!(ledger.add_genesis_winner, Some(false));
        assert_eq!(ledger.accounts_with_genesis_winner().len(), 17);
        assert_eq!(
            crate::daemon_json::ledger_hash_of_accounts(ledger.accounts.as_ref().unwrap())
                .unwrap()
                .to_string(),
            SAMPLE_HASH
        );
        let fork = config.proof.unwrap();
        let constants = fork
            .constraint_constants(mina_core::NetworkConfig::global().constraint_constants)
            .unwrap();
        let point = constants.fork.unwrap();
        assert_eq!(point.blockchain_length, 545433);
        assert_eq!(point.global_slot_since_genesis, 859560);
        let epochs = config.epoch_data.unwrap();
        assert_eq!(epochs.staking.hash.as_deref(), Some(SAMPLE_HASH));
        assert_eq!(epochs.next.unwrap().accounts.unwrap().len(), 17);
    }

    /// `daemon.json` has exactly the fields of OCaml's
    /// `make_automatic_fork_config` output, minus `s3_data_hash`.
    #[test]
    fn hash_only_config_layout() {
        let config = ForkRuntimeConfig::stable(&export());
        let text = serde_json::to_string(&config).unwrap();
        assert_eq!(
            text,
            format!(
                "{{\"genesis\":{{\"genesis_state_timestamp\":\"2026-08-19T18:00:00.000000Z\"}},\
                 \"proof\":{{\"fork\":{{\"state_hash\":\"{FORK_STATE_HASH}\",\
                 \"blockchain_length\":545433,\"global_slot_since_genesis\":859560}}}},\
                 \"ledger\":{{\"hash\":\"{SAMPLE_HASH}\",\"add_genesis_winner\":false}},\
                 \"epoch_data\":{{\"staking\":{{\"seed\":\"{seed}\",\"hash\":\"{SAMPLE_HASH}\"}},\
                 \"next\":{{\"seed\":\"{seed}\",\"hash\":\"{SAMPLE_HASH}\"}}}}}}",
                seed = EpochSeed::zero()
            )
        );
        let back: ForkRuntimeConfig = serde_json::from_str(&text).unwrap();
        assert_eq!(back, config);
    }
}
