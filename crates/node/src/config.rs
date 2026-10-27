use std::{str::FromStr, sync::Arc};

use mina_core::consensus::ConsensusConstants;
use mina_p2p_messages::v2::CurrencyFeeStableV1;
use serde::{Deserialize, Serialize};

use crate::{
    account::AccountPublicKey,
    transition_frontier::{archive::archive_config::ArchiveConfig, genesis::GenesisConfig},
};
pub use crate::{
    block_producer::BlockProducerConfig, ledger::LedgerConfig, p2p::P2pConfig, snark::SnarkConfig,
    snark_pool::SnarkPoolConfig, transition_frontier::TransitionFrontierConfig,
};
pub use mina_p2p_messages::v2::MinaBaseProtocolConstantsCheckedValueStableV1 as ProtocolConstants;

// TODO(binier): maybe make sure config is immutable.

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Config {
    pub ledger: LedgerConfig,
    pub snark: SnarkConfig,
    pub p2p: P2pConfig,
    pub transition_frontier: TransitionFrontierConfig,
    pub archive: Option<ArchiveConfig>,
    pub block_producer: Option<BlockProducerConfig>,
    pub global: GlobalConfig,
    pub tx_pool: ledger::transaction_pool::Config,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GlobalConfig {
    pub build: Box<BuildEnv>,
    pub snarker: Option<SnarkerConfig>,
    pub consensus_constants: ConsensusConstants,
    pub client_port: Option<u16>,
    pub testing_run: bool,
    pub chain_id_override: Option<mina_core::ChainId>,
    pub skip_proof_verification: bool,
    /// The operator may start, stop and re-price the SNARK worker while the
    /// node runs (P5.4, `--allow-worker-control`). Never on with the public
    /// HTTP API: it changes where fees go.
    #[serde(default)]
    pub worker_control: bool,
    /// How the SNARK worker selects jobs (`--snarker-strategy`), also one the
    /// operator starts while the node runs (P5.4), which has no
    /// [`SnarkerConfig`] to take it from.
    #[serde(default)]
    pub snarker_strategy: SnarkerStrategy,
    /// Native worker lead for a producer's known won slots. Also applies to
    /// workers started later through the dashboard.
    #[serde(default)]
    pub block_first_lead: Option<std::time::Duration>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SnarkerConfig {
    pub public_key: AccountPublicKey,
    pub fee: CurrencyFeeStableV1,
    pub strategy: SnarkerStrategy,
    pub auto_commit: bool,
    /// A scheduling lead for a worker whose prover competes with the block
    /// producer's (P5.3). The worker then takes no job while
    /// a block is produced or a known won slot's block is due within this
    /// lead ([`crate::State::snark_worker_may_take_job`]). `None`: the worker
    /// has no scheduling lead (including the web node's isolated prover).
    #[serde(default)]
    pub block_first_lead: Option<std::time::Duration>,
    /// The worker's prover has no zkApp step circuits: the web node's
    /// isolated prover (P5.3), which loads only the transaction provers.
    /// The worker then takes no zkApp command job and leaves those to other
    /// workers, as P5.2's did before P5.2e. `false` for the native node.
    #[serde(default)]
    pub skip_zkapp_jobs: bool,
}

impl SnarkerConfig {
    /// The native worker for `setting`: the key and fee are the setting's,
    /// the rest `previous`'s if the worker had a configuration, otherwise
    /// what `--run-snarker` gives: `strategy`, the node's job selection, and
    /// a prover of its own.
    pub fn with_setting(
        setting: &SnarkerSetting,
        previous: Option<&SnarkerConfig>,
        strategy: SnarkerStrategy,
    ) -> Self {
        let fee = CurrencyFeeStableV1(setting.fee.into());
        match previous {
            Some(previous) => Self {
                public_key: setting.public_key.clone(),
                fee,
                ..previous.clone()
            },
            None => Self {
                public_key: setting.public_key.clone(),
                fee,
                strategy,
                auto_commit: true,
                block_first_lead: None,
                skip_zkapp_jobs: false,
            },
        }
    }

    pub fn setting(&self) -> SnarkerSetting {
        SnarkerSetting {
            public_key: self.public_key.clone(),
            fee: self.fee.0.as_u64(),
        }
    }
}

/// What the operator chooses for the SNARK worker at runtime (P5.4): the key
/// that receives the fees and the fee asked per bundle, in nanomina.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SnarkerSetting {
    pub public_key: AccountPublicKey,
    pub fee: u64,
}

/// How the SNARK worker picks its next job among those it may take
/// ([`crate::snark_pool::SnarkPoolState::jobs_to_prove_iter`]): OCaml's
/// `--work-selection`, whose default is `rand`. OCaml's third method,
/// `roffset`, is not implemented.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/cli_entrypoint/mina_cli_entrypoint.ml#L267-L272>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/cli_entrypoint/mina_cli_entrypoint.ml#L946-L953>
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SnarkerStrategy {
    /// OCaml's `seq`: the first in scan-state order.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector/sequence.ml#L1-L8>
    Sequential,
    /// OCaml's `rand`, the default: any of them, uniformly.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/work_selector/random.ml#L4-L11>
    #[default]
    Random,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BuildEnv {
    pub time: String,
    pub version: String,
    pub git: GitBuildEnv,
    pub cargo: CargoBuildEnv,
    pub rustc: RustCBuildEnv,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GitBuildEnv {
    pub commit_time: String,
    pub commit_hash: String,
    pub branch: String,
    #[serde(default)]
    pub describe: String,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CargoBuildEnv {
    pub features: String,
    pub opt_level: u8,
    pub target: String,
    pub is_debug: bool,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RustCBuildEnv {
    pub channel: String,
    pub commit_date: String,
    pub commit_hash: String,
    pub host: String,
    pub version: String,
    pub llvm_version: String,
}

#[allow(clippy::eq_op)]
impl BuildEnv {
    pub fn get() -> Self {
        Self {
            time: env!("VERGEN_BUILD_TIMESTAMP").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            git: GitBuildEnv {
                commit_time: env!("VERGEN_GIT_COMMIT_TIMESTAMP").to_owned(),
                commit_hash: env!("VERGEN_GIT_SHA").to_owned(),
                branch: env!("VERGEN_GIT_BRANCH").to_owned(),
                describe: env!("VERGEN_GIT_DESCRIBE").to_owned(),
            },
            cargo: CargoBuildEnv {
                features: env!("VERGEN_CARGO_FEATURES").to_owned(),
                opt_level: env!("VERGEN_CARGO_OPT_LEVEL").parse().unwrap(),
                target: env!("VERGEN_CARGO_TARGET_TRIPLE").to_owned(),
                is_debug: env!("VERGEN_CARGO_DEBUG") == "true",
            },
            rustc: RustCBuildEnv {
                channel: env!("VERGEN_RUSTC_CHANNEL").to_owned(),
                commit_date: env!("VERGEN_RUSTC_COMMIT_DATE").to_owned(),
                commit_hash: env!("VERGEN_RUSTC_COMMIT_HASH").to_owned(),
                host: env!("VERGEN_RUSTC_HOST_TRIPLE").to_owned(),
                version: env!("VERGEN_RUSTC_SEMVER").to_owned(),
                llvm_version: env!("VERGEN_RUSTC_LLVM_VERSION").to_owned(),
            },
        }
    }
}

#[derive(thiserror::Error, Debug)]
#[error("invalid strategy: {0}! expected one of: seq/sequential/rand/random")]
pub struct SnarkerStrategyParseError(String);

impl FromStr for SnarkerStrategy {
    type Err = SnarkerStrategyParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "seq" | "sequential" => SnarkerStrategy::Sequential,
            "rand" | "random" => SnarkerStrategy::Random,
            other => return Err(SnarkerStrategyParseError(other.to_owned())),
        })
    }
}

lazy_static::lazy_static! {
    /// The Mesa mainnet genesis, embedded without ledgers so release binaries
    /// can bootstrap from peers without a separate genesis config file.
    pub static ref MAINNET_CONFIG: Arc<GenesisConfig> = {
        let bytes = include_bytes!("../../../genesis_ledgers/mesa-mainnet-without-ledgers.json");
        Arc::new(
            GenesisConfig::from_bytes(bytes.to_vec())
                .expect("the embedded mainnet genesis config is valid"),
        )
    };

    /// The Mesa devnet genesis, without its ledgers: what the genesis block
    /// commits to. Devnet has moved past its genesis, so a node starting from
    /// it syncs its ledgers from peers. `ledger-tool` writes it from the
    /// RocksDB ledger tarballs of the Mesa devnet config
    /// (`genesis_ledgers/README.md`).
    pub static ref DEVNET_CONFIG: Arc<GenesisConfig> = {
        let bytes = include_bytes!("../../../genesis_ledgers/mesa-devnet-without-ledgers.json");
        Arc::new(
            GenesisConfig::from_bytes(bytes.to_vec())
                .expect("the embedded devnet genesis config is valid"),
        )
    };
}

/// Select the embedded genesis for the configured network. Explicit genesis
/// configurations are handled by the caller before falling back to this.
pub fn default_genesis_config(network: &mina_core::network::NetworkId) -> Arc<GenesisConfig> {
    match network {
        mina_core::network::NetworkId::MAINNET => MAINNET_CONFIG.clone(),
        mina_core::network::NetworkId::TESTNET => DEVNET_CONFIG.clone(),
    }
}

#[cfg(test)]
mod tests {
    use mina_p2p_messages::v2;
    use time::{format_description::well_known::Rfc3339, OffsetDateTime};

    use super::DEVNET_CONFIG;
    use crate::transition_frontier::genesis::GenesisConfig;

    #[test]
    fn build_env_uses_crate_version() {
        assert_eq!(super::BuildEnv::get().version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn default_genesis_matches_network() {
        use mina_core::network::NetworkId;

        for (network, expected) in [
            (NetworkId::MAINNET, &*super::MAINNET_CONFIG),
            (NetworkId::TESTNET, &*DEVNET_CONFIG),
        ] {
            assert!(std::sync::Arc::ptr_eq(
                &super::default_genesis_config(&network),
                expected,
            ));
        }
    }

    /// The embedded devnet genesis against the Mesa devnet config of OCaml
    /// 4.0.2, `genesis_ledgers/devnet.json` (`config_84b8d969.json` in the
    /// mina-devnet-config deb). The total currencies are the default-token
    /// balance sums of its three ledgers, which the first Mesa devnet blocks
    /// carry too (R13).
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/genesis_ledgers/devnet.json#L1-L33>
    #[test]
    fn devnet_config() {
        assert!(matches!(**DEVNET_CONFIG, GenesisConfig::WithoutLedgers(_)));
        let (masks, config) = DEVNET_CONFIG.load().expect("should be loadable");
        assert!(masks.is_empty());

        let hash = |hash: &str| -> v2::LedgerHash { hash.parse().unwrap() };
        let seed = |seed: &str| -> v2::EpochSeed { seed.parse().unwrap() };
        let amount = |nanomina: u64| v2::CurrencyAmountStableV1(nanomina.into());
        assert_eq!(
            config.constants.genesis_state_timestamp,
            OffsetDateTime::parse("2026-08-19T18:00:00Z", &Rfc3339)
                .unwrap()
                .into()
        );
        assert_eq!(
            config.genesis_ledger_hash,
            hash("jwrCSCRNpNx3ZBh6D4bUAnZtqPacKXXe6veGC9kQfHrxWqmEbJS")
        );
        assert_eq!(
            config.staking_epoch_ledger_hash,
            hash("jwbFn9V1eyDa5AgLkHAwsVffHSjLLhfw8WpKYM6bJiKXb5Uhbti")
        );
        assert_eq!(
            config.staking_epoch_seed,
            seed("2vamERQZWT1miAnrxLJsjPmZmoCSemrz9JugjN8weQJeqhU7ZD38")
        );
        assert_eq!(
            config.next_epoch_ledger_hash,
            hash("jxUYRdFcuDDMyEtRXkAEo74EFpFSAjcVf6pBX9j56siW7rFZFRq")
        );
        assert_eq!(
            config.next_epoch_seed,
            seed("2vbVSm3AnPnhSaTBdQeU65gBAJmFiG6hHixXBQjeQ9MEBrtepDiL")
        );
        assert_eq!(config.genesis_total_currency, amount(1588907694000001000));
        assert_eq!(
            config.staking_epoch_total_currency,
            amount(1582881647000001000)
        );
        assert_eq!(
            config.next_epoch_total_currency,
            amount(1585748525000001000)
        );

        // The genesis block producer's stake proof is a path in the staking
        // epoch ledger.
        let mut proof =
            ledger::sparse_ledger::SparseLedger::try_from(&config.genesis_producer_stake_proof)
                .unwrap();
        assert_eq!(
            v2::LedgerHash::from_fp(proof.merkle_root()),
            config.staking_epoch_ledger_hash
        );
    }
}
