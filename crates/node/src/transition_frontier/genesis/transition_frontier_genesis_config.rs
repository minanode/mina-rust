use std::{
    borrow::Cow,
    fs::File,
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use crate::{account::AccountSecretKey, daemon_json::EpochData};
use ledger::{
    proofs::caching::{ensure_path_exists, mina_cache_path},
    scan_state::currency::Balance,
    BaseLedger,
};
use mina_core::constants::{constraint_constants, DEFAULT_GENESIS_TIMESTAMP_MILLISECONDS};
use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    bigint::InvalidBigInt,
    binprot::{
        self,
        macros::{BinProtRead, BinProtWrite},
        BinProtRead, BinProtWrite,
    },
    v2::{self, PROTOCOL_CONSTANTS},
};
use serde::{Deserialize, Serialize};

use crate::{
    daemon_json::{self, AccountConfigError, DaemonJson},
    ProtocolConstants,
};

pub use GenesisConfig as TransitionFrontierGenesisConfig;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum GenesisConfig {
    Counts {
        whales: usize,
        fish: usize,
        non_stakers: NonStakers,
        constants: ProtocolConstants,
    },
    BalancesDelegateTable {
        table: Vec<(u64, Vec<u64>)>,
        constants: ProtocolConstants,
    },
    AccountsBinProt {
        bytes: Cow<'static, [u8]>,
        constants: ProtocolConstants,
    },
    Prebuilt(Cow<'static, [u8]>),
    DaemonJson(Box<DaemonJson>),
    DaemonJsonFile(PathBuf),
    /// Only what the genesis block commits to, without the ledgers.
    ///
    /// A node joining a network that has moved past its genesis syncs its
    /// ledgers from peers and never reads the genesis ones, so it can start
    /// from this. Its genesis block has no staged ledger, so a network still
    /// at its genesis needs one of the other variants.
    WithoutLedgers(Box<GenesisConfigLoaded>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum NonStakers {
    /// Adds all the accounts from the generated array to the ledger
    Fill,
    /// No non-staker accounts will be added to the ledger
    None,
    /// Add a precise amount of accounts, non greater than the amount of generated accounts
    Count(usize),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GenesisConfigLoaded {
    /// Preserved when converting daemon JSON to compact JSON or prebuilt data.
    #[serde(flatten)]
    pub runtime_config: Box<GenesisRuntimeConfig>,
    pub constants: ProtocolConstants,
    pub genesis_ledger_hash: v2::LedgerHash,
    pub genesis_total_currency: v2::CurrencyAmountStableV1,
    pub genesis_producer_stake_proof: v2::MinaBaseSparseLedgerBaseStableV2,
    pub staking_epoch_ledger_hash: v2::LedgerHash,
    pub staking_epoch_total_currency: v2::CurrencyAmountStableV1,
    pub staking_epoch_seed: v2::EpochSeed,
    pub next_epoch_ledger_hash: v2::LedgerHash,
    pub next_epoch_total_currency: v2::CurrencyAmountStableV1,
    pub next_epoch_seed: v2::EpochSeed,
}

/// Runtime settings shared by all genesis formats. Missing fields use the
/// selected network's compiled profile. Proof keys/digests remain release assets.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct GenesisRuntimeConfig {
    pub proof: Option<daemon_json::Proof>,
    pub daemon: Option<daemon_json::Daemon>,
}

impl GenesisRuntimeConfig {
    pub fn from_daemon(config: &DaemonJson) -> Self {
        Self {
            proof: config.proof.clone(),
            daemon: config.daemon.clone(),
        }
    }

    pub fn initialize(&self) -> Result<(), GenesisConfigError> {
        let defaults = mina_core::NetworkConfig::global().constraint_constants;
        let constants = self
            .proof
            .as_ref()
            .map(|proof| proof.constraint_constants(defaults))
            .transpose()
            .map_err(GenesisConfigError::Runtime)?
            .unwrap_or_else(|| defaults.clone());
        let stops = self
            .daemon
            .as_ref()
            .map(|daemon| daemon.stop_slots())
            .unwrap_or_default();
        mina_core::constants::initialize_runtime_constants(constants, stops)
            .map_err(GenesisConfigError::Runtime)
    }
}

// Versioned metadata precedes the legacy payload, so it can be read before
// loading any accounts or constructing consensus constants. Old raw bin_prot
// bundles remain readable. Never infer fork metadata from the selected network.
const PREBUILT_MAGIC: &[u8; 8] = b"MINAGEN1";
const MAX_RUNTIME_CONFIG_BYTES: usize = 1024 * 1024;

fn read_runtime_header(reader: &mut impl Read) -> Result<GenesisRuntimeConfig, GenesisConfigError> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_RUNTIME_CONFIG_BYTES {
        return Err(GenesisConfigError::Runtime(
            "prebuilt runtime header too large".into(),
        ));
    }
    let mut json = vec![0; length];
    reader.read_exact(&mut json)?;
    Ok(serde_json::from_slice(&json)?)
}

fn prebuilt_payload(mut bytes: &[u8]) -> Result<(GenesisRuntimeConfig, &[u8]), GenesisConfigError> {
    let runtime = if bytes.starts_with(PREBUILT_MAGIC) {
        bytes = &bytes[PREBUILT_MAGIC.len()..];
        read_runtime_header(&mut bytes)?
    } else {
        GenesisRuntimeConfig::default()
    };
    Ok((runtime, bytes))
}

/// A ledger built from inline accounts must have the hash the config declares
/// for it, if it declares one. (OCaml replaces it with the computed hash.)
fn check_declared_hash(
    declared: Option<&str>,
    computed: &v2::LedgerHash,
) -> Result<(), GenesisConfigError> {
    match declared {
        Some(declared) if declared != computed.to_string() => {
            Err(GenesisConfigError::LedgerHashMismatch {
                expected: declared.parse().map_err(|_| {
                    GenesisConfigError::Runtime(format!("bad ledger hash {declared}"))
                })?,
                computed: computed.clone(),
            })
        }
        _ => Ok(()),
    }
}

fn bp_num_delegators(i: usize) -> usize {
    (i.saturating_add(1))
        .checked_mul(2)
        .expect("overflow")
        .min(32)
}

#[derive(Debug, thiserror::Error)]
pub enum GenesisConfigError {
    #[error("invalid runtime configuration: {0}")]
    Runtime(String),
    #[error("{0}: inline accounts are required; hash-only daemon ledgers/tarballs are unsupported; use fork_config.json or a compact genesis config")]
    MissingAccounts(&'static str),
    #[error("no ledger in configuration")]
    NoLedger,
    #[error("declared and computed ledger hashes don't match: {expected} != {computed}")]
    LedgerHashMismatch {
        expected: v2::LedgerHash,
        computed: v2::LedgerHash,
    },
    #[error("account error: {0}")]
    Account(#[from] AccountConfigError),
    #[error("error loading genesis config from precomputed data: {0}")]
    Prebuilt(#[from] binprot::Error),
    #[error("error deserializing daemon.json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid bigint")]
    InvalidBigInt(#[from] InvalidBigInt),
}

/// Opens `path` behind a 1 MiB buffer. `serde_json::from_reader` and
/// `binprot_read` issue small reads (about one syscall per byte on a raw
/// `File`): the 736 MB mainnet daemon JSON took 696 s to parse unbuffered and
/// about 9.5 s buffered, the three ledger caches 29 s and 4 s (R31).
pub(crate) fn buffered(path: impl AsRef<std::path::Path>) -> std::io::Result<BufReader<File>> {
    Ok(BufReader::with_capacity(1 << 20, File::open(path)?))
}

impl GenesisConfig {
    /// Native startup checks the configured zero-currency genesis against
    /// the OCaml trace before installing a runtime fork's verifier/digests.
    pub fn prepare_runtime_fork(
        &self,
    ) -> Result<
        Option<&'static ledger::proofs::fork_circuits::RuntimeForkCircuits>,
        GenesisConfigError,
    > {
        self.initialize_runtime()?;
        if constraint_constants().fork
            == mina_core::NetworkConfig::global().constraint_constants.fork
        {
            return Ok(None);
        }
        let (_, loaded) = self.load()?;
        let zero = || v2::CurrencyAmountStableV1(0u64.into());
        let (_, _, hash) = loaded.protocol_states(zero(), zero(), zero())?;
        ledger::proofs::fork_circuits::prepare_runtime_fork(&hash, &loaded.constants)
            .map_err(GenesisConfigError::Runtime)
    }

    /// Web startup must refuse forks requiring a native circuit bundle before
    /// constructing a verifier or the networking service.
    pub fn initialize_web_runtime(&self) -> Result<(), GenesisConfigError> {
        self.initialize_runtime()?;
        ledger::proofs::fork_circuits::ensure_web_fork_supported(
            &constraint_constants().fork,
            &mina_core::NetworkConfig::global().constraint_constants.fork,
        )
        .map_err(GenesisConfigError::Runtime)
    }

    /// The web worker's startup boundary. Keep the guard error in the message
    /// sent back to `run()`, which rejects the page's startup promise.
    pub fn prepare_web_startup(&self) -> anyhow::Result<()> {
        self.initialize_web_runtime()
            .map_err(|error| anyhow::anyhow!("invalid runtime genesis config: {error}"))
    }

    pub fn runtime_config(&self) -> Result<GenesisRuntimeConfig, GenesisConfigError> {
        Ok(match self {
            Self::DaemonJson(config) => GenesisRuntimeConfig::from_daemon(config),
            Self::DaemonJsonFile(path) => {
                GenesisRuntimeConfig::from_daemon(&serde_json::from_reader(buffered(path)?)?)
            }
            Self::WithoutLedgers(loaded) => (*loaded.runtime_config).clone(),
            Self::Prebuilt(bytes) => prebuilt_payload(bytes)?.0,
            _ => GenesisRuntimeConfig::default(),
        })
    }

    pub fn initialize_runtime(&self) -> Result<(), GenesisConfigError> {
        self.runtime_config()?.initialize()
    }

    pub fn default_constants(timestamp_ms: u64) -> ProtocolConstants {
        ProtocolConstants {
            k: 290.into(),
            slots_per_epoch: 7140.into(),
            slots_per_sub_window: 7.into(),
            grace_period_slots: 2160.into(),
            delta: 0.into(),
            genesis_state_timestamp: v2::BlockTimeTimeStableV1(
                v2::UnsignedExtendedUInt64Int64ForVersionTagsStableV1(timestamp_ms.into()),
            ),
        }
    }

    // This is a stub for the moment until PR #420 is merged, which implements this for
    // real. In case of conflict, delete this stub and put the real implementation here.
    pub fn protocol_constants(&self) -> Result<ProtocolConstants, GenesisConfigError> {
        match self {
            Self::Counts { constants, .. }
            | Self::BalancesDelegateTable { constants, .. }
            | Self::AccountsBinProt { constants, .. } => Ok(constants.clone()),
            // Skip only the small runtime header, never load the ledgers.
            Self::Prebuilt(bytes) => Ok(ProtocolConstants::binprot_read(
                &mut prebuilt_payload(bytes)?.1,
            )?),
            Self::WithoutLedgers(loaded) => Ok(loaded.constants.clone()),
            Self::DaemonJson(config) => Ok(config
                .genesis
                .as_ref()
                .map(|g: &daemon_json::Genesis| g.protocol_constants())
                .unwrap_or(Self::default_constants(
                    DEFAULT_GENESIS_TIMESTAMP_MILLISECONDS,
                ))),
            Self::DaemonJsonFile(path) => {
                Self::DaemonJson(Box::new(serde_json::from_reader(buffered(path)?)?))
                    .protocol_constants()
            }
        }
    }

    pub fn override_genesis_state_timestamp(&mut self, timestamp: v2::BlockTimeTimeStableV1) {
        match self {
            Self::Counts { constants, .. }
            | Self::BalancesDelegateTable { constants, .. }
            | Self::AccountsBinProt { constants, .. } => {
                constants.genesis_state_timestamp = timestamp;
            }
            _ => todo!(),
        }
    }

    pub fn load(
        &self,
    ) -> anyhow::Result<(Vec<ledger::Mask>, GenesisConfigLoaded), GenesisConfigError> {
        self.initialize_runtime()?;
        Ok(match self {
            Self::Counts {
                whales,
                fish,
                non_stakers,
                constants,
            } => {
                let (whales, fish) = (*whales, *fish);
                let delegator_balance =
                    |balance: u64| move |i| balance.checked_div(i as u64).expect("division by 0");
                let whales = (0..whales).map(|i| {
                    let balance = 8333_u64;
                    let delegators = (1..=bp_num_delegators(i)).map(delegator_balance(50_000_000));
                    (balance, delegators)
                });
                let fish = (0..fish).map(|i| {
                    let balance = 6333_u64;
                    let delegators = (1..=bp_num_delegators(i)).map(delegator_balance(5_000_000));
                    (balance, delegators)
                });
                let delegator_table = whales.chain(fish);
                let (mut mask, genesis_total_currency) =
                    Self::build_ledger_from_balances_delegator_table(delegator_table, non_stakers)?;
                let genesis_ledger_hash = ledger_hash(&mut mask);
                let staking_epoch_total_currency = genesis_total_currency.clone();
                let next_epoch_total_currency = genesis_total_currency.clone();
                let staking_epoch_seed = v2::EpochSeed::zero();
                let next_epoch_seed = v2::EpochSeed::zero();

                let load_result = GenesisConfigLoaded {
                    runtime_config: self.runtime_config()?.into(),
                    constants: constants.clone(),
                    genesis_ledger_hash: genesis_ledger_hash.clone(),
                    genesis_total_currency,
                    genesis_producer_stake_proof: create_genesis_producer_stake_proof(&mask),
                    staking_epoch_ledger_hash: genesis_ledger_hash.clone(),
                    staking_epoch_total_currency,
                    next_epoch_ledger_hash: genesis_ledger_hash,
                    next_epoch_total_currency,
                    staking_epoch_seed,
                    next_epoch_seed,
                };
                let masks = vec![mask];
                (masks, load_result)
            }
            Self::BalancesDelegateTable { table, constants } => {
                let table = table.iter().map(|(bp_balance, delegators)| {
                    let delegators = delegators.iter().copied();
                    (*bp_balance, delegators)
                });
                let (mut mask, genesis_total_currency) =
                    Self::build_ledger_from_balances_delegator_table(table, &NonStakers::None)?;
                let genesis_ledger_hash = ledger_hash(&mut mask);
                let staking_epoch_total_currency = genesis_total_currency.clone();
                let next_epoch_total_currency = genesis_total_currency.clone();
                let staking_epoch_seed = v2::EpochSeed::zero();
                let next_epoch_seed = v2::EpochSeed::zero();

                let load_result = GenesisConfigLoaded {
                    runtime_config: self.runtime_config()?.into(),
                    constants: constants.clone(),
                    genesis_ledger_hash: genesis_ledger_hash.clone(),
                    genesis_total_currency,
                    genesis_producer_stake_proof: create_genesis_producer_stake_proof(&mask),
                    staking_epoch_ledger_hash: genesis_ledger_hash.clone(),
                    staking_epoch_total_currency,
                    next_epoch_ledger_hash: genesis_ledger_hash,
                    next_epoch_total_currency,
                    staking_epoch_seed,
                    next_epoch_seed,
                };
                let masks = vec![mask];
                (masks, load_result)
            }
            Self::Prebuilt(bytes) => PrebuiltGenesisConfig::load_from_bytes(bytes)?,
            Self::AccountsBinProt { bytes, .. } => {
                let mut bytes = bytes.as_ref();
                let _expected_hash = Option::<v2::LedgerHash>::binprot_read(&mut bytes)?;
                let hashes = Vec::<(u64, v2::LedgerHash)>::binprot_read(&mut bytes)?
                    .into_iter()
                    .map(|(idx, hash)| Ok((idx, hash.0.to_field()?)))
                    .collect::<Result<Vec<_>, InvalidBigInt>>()?;
                let accounts = Vec::<ledger::Account>::binprot_read(&mut bytes)?;

                let (mut mask, _total_currency) = Self::build_ledger_from_accounts_and_hashes(
                    accounts.into_iter().map(Ok),
                    hashes,
                )?;
                let _ledger_hash = ledger_hash(&mut mask);

                todo!()

                // // TODO(tizoc): currently this doesn't really do much, because now we load the hashes
                // // from the bin_prot data too to speed up the loading. Maybe add some flag
                // // to force the rehashing and validation of the loaded ledger hashes.
                // if let Some(expected_hash) = expected_hash.filter(|h| h != &ledger_hash) {
                //     anyhow::bail!("ledger hash mismatch after building the mask! expected: '{expected_hash}', got '{ledger_hash}'");
                // }
                //
                // let load_result = GenesisConfigLoaded {
                //     constants: constants.clone(),
                //     ledger_hash,
                //     total_currency,
                //     genesis_producer_stake_proof: genesis_producer_stake_proof(&mask),
                // };
                // (mask, load_result)
            }
            Self::DaemonJson(config) => {
                let mut masks = Vec::new();
                let constants = config
                    .genesis
                    .as_ref()
                    .map_or(PROTOCOL_CONSTANTS, |genesis| genesis.protocol_constants());
                let ledger = config.ledger.as_ref().ok_or(GenesisConfigError::NoLedger)?;
                // OCaml can resolve hash-only configs via RocksDB tarballs;
                // this loader supports inline accounts, not that format.
                // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_ledger_helper/genesis_ledger_helper.ml#L579-L614>
                ledger
                    .accounts
                    .as_ref()
                    .ok_or(GenesisConfigError::MissingAccounts("ledger.accounts"))?;
                let accounts = ledger
                    .accounts_with_genesis_winner()
                    .iter()
                    .map(daemon_json::Account::to_account)
                    .collect::<Result<Vec<_>, _>>()?;
                let (mask, total_currency, genesis_ledger_hash) =
                    Self::build_or_load_ledger(ledger.ledger_name(), accounts.into_iter())?;

                masks.push(mask.clone());
                if let Some(expected_hash) = config.ledger.as_ref().and_then(|l| l.hash.as_ref()) {
                    if expected_hash != &genesis_ledger_hash.to_string() {
                        return Err(GenesisConfigError::LedgerHashMismatch {
                            expected: expected_hash.parse().unwrap(),
                            computed: genesis_ledger_hash.clone(),
                        });
                    }
                }

                let staking_epoch_ledger_hash;
                let staking_epoch_total_currency;
                let staking_epoch_seed: v2::EpochSeed;
                let next_epoch_ledger_hash;
                let next_epoch_total_currency;
                let next_epoch_seed: v2::EpochSeed;
                let genesis_producer_stake_proof: v2::MinaBaseSparseLedgerBaseStableV2;
                // TODO(devnet): handle other cases here, right now this works
                // only for the post-fork genesis
                if let Some(data) = &config.epoch_data {
                    let accounts = data
                        .staking
                        .accounts
                        .as_ref()
                        .ok_or(GenesisConfigError::MissingAccounts(
                            "epoch_data.staking.accounts",
                        ))?
                        .iter()
                        .map(daemon_json::Account::to_account)
                        .collect::<Result<Vec<_>, _>>()?;
                    let (staking_ledger_mask, total_currency, hash) = Self::build_or_load_ledger(
                        data.staking.ledger_name(),
                        accounts.into_iter(),
                    )?;
                    check_declared_hash(data.staking.hash.as_deref(), &hash)?;
                    staking_epoch_ledger_hash = hash;
                    staking_epoch_total_currency = total_currency;
                    staking_epoch_seed = v2::EpochSeed::from_str(&data.staking.seed).unwrap();
                    genesis_producer_stake_proof =
                        create_genesis_producer_stake_proof(&staking_ledger_mask);
                    masks.push(staking_ledger_mask);

                    // Without `next`, the next epoch ledger is the staking one, as in
                    // OCaml's `Runtime_config.Epoch_data`.
                    let next = data.next.as_ref().unwrap_or(&data.staking);
                    // OCaml compares the declared Option hashes, including
                    // None = None. Reuse the staking ledger but keep next's seed.
                    // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_ledger_helper/genesis_ledger_helper.ml#L643-L668>
                    if next.hash == data.staking.hash {
                        next_epoch_ledger_hash = staking_epoch_ledger_hash.clone();
                        next_epoch_total_currency = staking_epoch_total_currency.clone();
                    } else {
                        let accounts = next
                            .accounts
                            .as_ref()
                            .ok_or(GenesisConfigError::MissingAccounts(
                                "epoch_data.next.accounts",
                            ))?
                            .iter()
                            .map(daemon_json::Account::to_account)
                            .collect::<Result<Vec<_>, _>>()?;
                        let (next_mask, total_currency, hash) =
                            Self::build_or_load_ledger(next.ledger_name(), accounts.into_iter())?;
                        check_declared_hash(next.hash.as_deref(), &hash)?;
                        if hash != genesis_ledger_hash && hash != staking_epoch_ledger_hash {
                            masks.push(next_mask);
                        }
                        next_epoch_ledger_hash = hash;
                        next_epoch_total_currency = total_currency;
                    }
                    next_epoch_seed = v2::EpochSeed::from_str(&next.seed).unwrap();
                } else {
                    staking_epoch_ledger_hash = genesis_ledger_hash.clone();
                    staking_epoch_total_currency = total_currency.clone();
                    staking_epoch_seed = v2::EpochSeed::zero();
                    next_epoch_ledger_hash = genesis_ledger_hash.clone();
                    next_epoch_total_currency = total_currency.clone();
                    next_epoch_seed = v2::EpochSeed::zero();
                    genesis_producer_stake_proof = create_genesis_producer_stake_proof(&mask);
                }

                let result = GenesisConfigLoaded {
                    runtime_config: GenesisRuntimeConfig::from_daemon(config).into(),
                    constants,
                    genesis_ledger_hash,
                    genesis_total_currency: total_currency,
                    genesis_producer_stake_proof,
                    staking_epoch_ledger_hash,
                    staking_epoch_total_currency,
                    next_epoch_ledger_hash,
                    next_epoch_total_currency,
                    staking_epoch_seed,
                    next_epoch_seed,
                };
                (masks, result)
            }
            Self::DaemonJsonFile(path) => {
                let c = serde_json::from_reader(buffered(path)?)?;
                Self::DaemonJson(c).load()?
            }
            Self::WithoutLedgers(loaded) => (Vec::new(), (**loaded).clone()),
        })
    }

    /// Reads a genesis config file: a [`PrebuiltGenesisConfig`] in bin_prot,
    /// or a [`GenesisConfigLoaded`] in JSON for [`Self::WithoutLedgers`].
    ///
    /// Prebuilt data has a versioned runtime header, or (for legacy bundles)
    /// starts with protocol constants. Neither begins with `{` on our networks.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, GenesisConfigError> {
        if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'{') {
            Ok(Self::WithoutLedgers(Box::new(serde_json::from_slice(
                &bytes,
            )?)))
        } else {
            Ok(Self::Prebuilt(bytes.into()))
        }
    }

    fn build_or_load_ledger(
        ledger_name: String,
        accounts: impl Iterator<Item = ledger::Account>,
    ) -> Result<(ledger::Mask, v2::CurrencyAmountStableV1, LedgerHash), GenesisConfigError> {
        mina_core::info!(
            mina_core::log::system_time();
            kind = "ledger loading",
            message = "loading the ledger",
            ledger_name = ledger_name,
        );
        match LedgerAccountsWithHash::load(ledger_name)? {
            Some(accounts_with_hash) => {
                let (mask, total_currency) = Self::build_ledger_from_accounts_and_hashes(
                    accounts_with_hash
                        .accounts
                        .iter()
                        .map(ledger::Account::try_from),
                    accounts_with_hash
                        .hashes
                        .into_iter()
                        .map(|(n, h)| Ok((n, h.to_field()?)))
                        .collect::<Result<Vec<_>, InvalidBigInt>>()?,
                )?;
                mina_core::info!(
                    mina_core::log::system_time();
                    kind = "ledger loaded",
                    message = "loaded from cache",
                    ledger_hash = accounts_with_hash.ledger_hash.to_string(),
                );
                Ok((mask, total_currency, accounts_with_hash.ledger_hash))
            }
            None => {
                let (mut mask, total_currency) =
                    Self::build_ledger_from_accounts(accounts.into_iter().map(Ok))?;
                let hash = ledger_hash(&mut mask);
                let ledger_accounts = LedgerAccountsWithHash {
                    accounts: mask.fold(Vec::new(), |mut acc, a| {
                        acc.push(a.into());
                        acc
                    }),
                    ledger_hash: hash.clone(),
                    hashes: mask
                        .get_raw_inner_hashes()
                        .into_iter()
                        .map(|(idx, hash)| (idx, v2::LedgerHash::from_fp(hash)))
                        .collect(),
                };
                ledger_accounts.cache()?;
                mina_core::info!(
                    mina_core::log::system_time();
                    kind = "ledger loaded",
                    message = "built from config and cached",
                    ledger_hash = hash.to_string(),
                );
                Ok((mask, total_currency, hash))
            }
        }
    }

    fn build_ledger_from_balances_delegator_table(
        block_producers: impl IntoIterator<Item = (u64, impl IntoIterator<Item = u64>)>,
        non_stakers: &NonStakers,
    ) -> Result<(ledger::Mask, v2::CurrencyAmountStableV1), InvalidBigInt> {
        let mut counter = 0;
        let mut total_balance = 0;

        fn create_account(
            counter: &mut u64,
            balance: u64,
            delegate: Option<mina_signer::CompressedPubKey>,
            total_balance: &mut u64,
        ) -> ledger::Account {
            let sec_key = AccountSecretKey::deterministic(*counter);
            let pub_key: mina_signer::CompressedPubKey = sec_key.public_key().try_into().unwrap(); // unwrap: `sec_key` was generated
            let account_id = ledger::AccountId::new(pub_key.clone(), Default::default());
            let mut account =
                ledger::Account::create_with(account_id, Balance::from_mina(balance).unwrap());
            account.delegate = delegate;
            *total_balance = total_balance.checked_add(balance).expect("overflow");
            *counter = counter.checked_add(1).expect("overflow");
            // println!("Created account with balance: {}, total_balance: {}", balance, *total_balance); // Debug print
            account
        }

        let mut accounts = genesis_account_iter().map(Ok).collect::<Vec<_>>();

        // Process block producers and their delegators
        for (bp_balance, delegators) in block_producers {
            let bp_account = create_account(&mut counter, bp_balance, None, &mut total_balance);
            let bp_pub_key = bp_account.public_key.clone();
            accounts.push(Ok(bp_account));

            for balance in delegators {
                let delegator_account = create_account(
                    &mut counter,
                    balance,
                    Some(bp_pub_key.clone()),
                    &mut total_balance,
                );
                accounts.push(Ok(delegator_account));
            }
        }

        let remaining_accounts =
            AccountSecretKey::max_deterministic_count().saturating_sub(counter as usize);

        let non_staker_count = match non_stakers {
            NonStakers::Fill => remaining_accounts,
            NonStakers::None => 0,
            NonStakers::Count(count) => *std::cmp::min(count, &remaining_accounts),
        };

        let non_staker_total = total_balance.checked_mul(20).expect("overflow") / 80;
        let non_staker_balance = non_staker_total
            .checked_div(non_staker_count as u64)
            .unwrap_or(0);

        mina_core::debug!("Non staker total balance: {}", non_staker_total);

        // Process non-stakers
        if matches!(non_stakers, NonStakers::Fill | NonStakers::Count(_)) {
            for _ in 0..non_staker_count {
                let non_staker_account =
                    create_account(&mut counter, non_staker_balance, None, &mut total_balance);
                accounts.push(Ok(non_staker_account));
            }
        }

        Self::build_ledger_from_accounts(accounts)
    }

    fn build_ledger_from_accounts(
        accounts: impl IntoIterator<Item = Result<ledger::Account, InvalidBigInt>>,
    ) -> Result<(ledger::Mask, v2::CurrencyAmountStableV1), InvalidBigInt> {
        let db =
            ledger::Database::create_with_token_owners(constraint_constants().ledger_depth as u8);
        let mask = ledger::Mask::new_root(db);
        let (mask, total_currency) = accounts.into_iter().try_fold(
            (mask, 0),
            |(mut mask, mut total_currency): (ledger::Mask, u64), account| {
                let account = account?;
                let account_id = account.id();
                // Only the default token counts towards the total currency that
                // stake is measured against.
                //
                // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/genesis_data.ml#L3-L11>
                if account.token_id.is_default() {
                    total_currency = total_currency
                        .checked_add(account.balance.as_u64())
                        .expect("overflow");
                }
                mask.get_or_create_account(account_id, account).unwrap();
                Ok((mask, total_currency))
            },
        )?;

        Ok((mask, v2::CurrencyAmountStableV1(total_currency.into())))
    }

    fn build_ledger_from_accounts_and_hashes(
        accounts: impl IntoIterator<Item = Result<ledger::Account, InvalidBigInt>>,
        hashes: Vec<(u64, Fp)>,
    ) -> Result<(ledger::Mask, v2::CurrencyAmountStableV1), InvalidBigInt> {
        let (mask, total_currency) = Self::build_ledger_from_accounts(accounts)?;

        // Must happen after the accounts have been set to avoid
        // cache invalidations.
        mask.set_raw_inner_hashes(hashes);

        Ok((mask, total_currency))
    }
}

fn ledger_hash(mask: &mut ledger::Mask) -> v2::LedgerHash {
    v2::MinaBaseLedgerHash0StableV1(mask.merkle_root().into()).into()
}

fn genesis_account_iter() -> impl Iterator<Item = ledger::Account> {
    std::iter::once({
        // add genesis producer as the first account.
        let pub_key = AccountSecretKey::genesis_producer().public_key();
        let account_id = ledger::AccountId::new(pub_key.try_into().unwrap(), Default::default()); // unwrap: `producer` is hardcoded
        ledger::Account::create_with(account_id, Balance::from_u64(0))
    })
}

fn create_genesis_producer_stake_proof(
    mask: &ledger::Mask,
) -> v2::MinaBaseSparseLedgerBaseStableV2 {
    let producer = AccountSecretKey::genesis_producer().public_key();
    let producer_id =
        ledger::AccountId::new(producer.try_into().unwrap(), ledger::TokenId::default()); // unwrap: `producer` is hardcoded
    let sparse_ledger =
        ledger::sparse_ledger::SparseLedger::of_ledger_subset_exn(mask.clone(), &[producer_id]);
    (&sparse_ledger).into()
}

#[derive(Debug, BinProtRead, BinProtWrite)]
struct LedgerAccountsWithHash {
    ledger_hash: LedgerHash,
    accounts: Vec<MinaBaseAccountBinableArgStableV2>,
    hashes: Vec<(u64, LedgerHash)>,
}

impl LedgerAccountsWithHash {
    fn cache(&self) -> Result<(), std::io::Error> {
        let cache_dir = mina_cache_path("ledgers").unwrap();
        let cache_file = cache_dir.join(format!("{}.bin", self.ledger_hash));
        ensure_path_exists(cache_dir)?;
        write_ledger_cache(&cache_file, |file| self.binprot_write(file))
    }

    fn load(ledger_name: String) -> Result<Option<Self>, binprot::Error> {
        let cache_filename = mina_cache_path(format!("ledgers/{}.bin", ledger_name)).unwrap();
        if cache_filename.is_file() {
            let mut file = buffered(cache_filename)?;
            LedgerAccountsWithHash::binprot_read(&mut file).map(Some)
        } else {
            Ok(None)
        }
    }
}

fn write_ledger_cache(
    path: &Path,
    write: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    // Each writer owns a different file in the cache directory. Renaming it
    // only after a complete, synced write keeps both existing open readers
    // and new readers on a whole bin_prot payload.
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    write(file.as_file_mut())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

use mina_p2p_messages::v2::{LedgerHash, MinaBaseAccountBinableArgStableV2};

/// Precalculated genesis configuration.
#[derive(Debug, Serialize, Deserialize)]
pub struct PrebuiltGenesisConfig {
    #[serde(flatten)]
    runtime_config: GenesisRuntimeConfig,
    constants: ProtocolConstants,
    accounts: Vec<MinaBaseAccountBinableArgStableV2>,
    ledger_hash: LedgerHash,
    hashes: Vec<(u64, LedgerHash)>,
    staking_epoch_data: PrebuiltGenesisEpochData,
    next_epoch_data: PrebuiltGenesisEpochData,
}

impl PrebuiltGenesisConfig {
    /// Rebuilds all three Merkle roots from accounts, ignoring the stored roots
    /// and inner-hash caches. Use this for checking untrusted prebuilt inputs;
    /// `load` intentionally uses those caches for normal node startup.
    pub fn recompute_ledger_hashes(self) -> Result<[LedgerHash; 3], GenesisConfigError> {
        self.runtime_config.initialize()?;
        let rebuild = |accounts: Vec<MinaBaseAccountBinableArgStableV2>| {
            let (mut mask, _) = GenesisConfig::build_ledger_from_accounts(
                accounts.into_iter().map(|account| (&account).try_into()),
            )?;
            Ok::<_, GenesisConfigError>(ledger_hash(&mut mask))
        };
        Ok([
            rebuild(self.accounts)?,
            rebuild(self.staking_epoch_data.accounts)?,
            rebuild(self.next_epoch_data.accounts)?,
        ])
    }

    /// Roots of the genesis, staking-epoch and next-epoch ledgers.
    pub fn ledger_hashes(&self) -> [&LedgerHash; 3] {
        [
            &self.ledger_hash,
            &self.staking_epoch_data.ledger_hash,
            &self.next_epoch_data.ledger_hash,
        ]
    }

    pub fn read<R: Read>(mut reader: R) -> Result<Self, GenesisConfigError> {
        let mut prefix = [0; 8];
        reader.read_exact(&mut prefix)?;
        let runtime_config = if &prefix == PREBUILT_MAGIC {
            read_runtime_header(&mut reader)?
        } else {
            GenesisRuntimeConfig::default()
        };
        let prefix = if &prefix == PREBUILT_MAGIC {
            &[][..]
        } else {
            &prefix[..]
        };
        let mut reader = prefix.chain(reader);
        Ok(Self {
            runtime_config,
            constants: BinProtRead::binprot_read(&mut reader)?,
            accounts: BinProtRead::binprot_read(&mut reader)?,
            ledger_hash: BinProtRead::binprot_read(&mut reader)?,
            hashes: BinProtRead::binprot_read(&mut reader)?,
            staking_epoch_data: BinProtRead::binprot_read(&mut reader)?,
            next_epoch_data: BinProtRead::binprot_read(&mut reader)?,
        })
    }

    pub fn store<W: Write>(&self, mut writer: W) -> Result<(), std::io::Error> {
        let json = serde_json::to_vec(&self.runtime_config)?;
        writer.write_all(PREBUILT_MAGIC)?;
        writer.write_all(&(json.len() as u32).to_le_bytes())?;
        writer.write_all(&json)?;
        self.constants.binprot_write(&mut writer)?;
        self.accounts.binprot_write(&mut writer)?;
        self.ledger_hash.binprot_write(&mut writer)?;
        self.hashes.binprot_write(&mut writer)?;
        self.staking_epoch_data.binprot_write(&mut writer)?;
        self.next_epoch_data.binprot_write(&mut writer)
    }

    /// Loads a bin_prot-encoded config straight into ledgers.
    ///
    /// Unlike [`Self::read`] followed by [`Self::load`], it decodes one account
    /// at a time. A decoded wire-format account takes several KB, mostly its
    /// inline zkApp fields, and `Vec` decoding reserves the whole vector up
    /// front, so mainnet's three ledgers of about 289k accounts would not fit
    /// in the 4 GiB of a wasm32 web node.
    pub fn load_from_bytes(
        bytes: &[u8],
    ) -> Result<(Vec<ledger::Mask>, GenesisConfigLoaded), GenesisConfigError> {
        let (runtime_config, mut bytes) = prebuilt_payload(bytes)?;
        runtime_config.initialize()?;
        let bytes = &mut bytes;
        let constants = ProtocolConstants::binprot_read(bytes)?;
        let (genesis_mask, genesis_total_currency, genesis_ledger_hash) =
            read_prebuilt_ledger(bytes)?;
        let (staking_ledger_mask, staking_epoch_total_currency, staking_epoch_ledger_hash) =
            read_prebuilt_ledger(bytes)?;
        let staking_epoch_seed = v2::EpochSeed::binprot_read(bytes)?;
        let (_mask, next_epoch_total_currency, next_epoch_ledger_hash) =
            read_prebuilt_ledger(bytes)?;
        let next_epoch_seed = v2::EpochSeed::binprot_read(bytes)?;

        let load_result = GenesisConfigLoaded {
            runtime_config: runtime_config.into(),
            constants,
            genesis_ledger_hash,
            genesis_total_currency,
            genesis_producer_stake_proof: create_genesis_producer_stake_proof(&staking_ledger_mask),
            staking_epoch_ledger_hash,
            staking_epoch_total_currency,
            next_epoch_ledger_hash,
            next_epoch_total_currency,
            staking_epoch_seed,
            next_epoch_seed,
        };
        Ok((vec![genesis_mask, staking_ledger_mask], load_result))
    }

    pub fn load(self) -> Result<(Vec<ledger::Mask>, GenesisConfigLoaded), GenesisConfigError> {
        self.runtime_config.initialize()?;
        let mut masks = Vec::new();
        let (mask, genesis_total_currency) = GenesisConfig::build_ledger_from_accounts_and_hashes(
            self.accounts.into_iter().map(|acc| (&acc).try_into()),
            self.hashes
                .into_iter()
                .map(|(n, h)| Ok((n, h.to_field()?)))
                .collect::<Result<Vec<_>, InvalidBigInt>>()?,
        )?;
        masks.push(mask);
        let (staking_ledger_mask, staking_epoch_total_currency) =
            GenesisConfig::build_ledger_from_accounts_and_hashes(
                self.staking_epoch_data
                    .accounts
                    .into_iter()
                    .map(|acc| (&acc).try_into()),
                self.staking_epoch_data
                    .hashes
                    .into_iter()
                    .map(|(n, h)| Ok((n, h.to_field()?)))
                    .collect::<Result<Vec<_>, InvalidBigInt>>()?,
            )?;
        let (_mask, next_epoch_total_currency) =
            GenesisConfig::build_ledger_from_accounts_and_hashes(
                self.next_epoch_data
                    .accounts
                    .into_iter()
                    .map(|acc| (&acc).try_into()),
                self.next_epoch_data
                    .hashes
                    .into_iter()
                    .map(|(n, h)| Ok((n, h.to_field()?)))
                    .collect::<Result<Vec<_>, InvalidBigInt>>()?,
            )?;

        let load_result = GenesisConfigLoaded {
            runtime_config: self.runtime_config.into(),
            constants: self.constants,
            genesis_ledger_hash: self.ledger_hash,
            genesis_total_currency,
            genesis_producer_stake_proof: create_genesis_producer_stake_proof(&staking_ledger_mask),
            staking_epoch_ledger_hash: self.staking_epoch_data.ledger_hash.clone(),
            staking_epoch_total_currency,
            next_epoch_ledger_hash: self.next_epoch_data.ledger_hash.clone(),
            next_epoch_total_currency,
            staking_epoch_seed: self.staking_epoch_data.seed,
            next_epoch_seed: self.next_epoch_data.seed,
        };
        masks.push(staking_ledger_mask);
        Ok((masks, load_result))
    }

    /// Builds a prebuilt config from the accounts of its three ledgers, each
    /// in ledger index order (account `i` at leaf `i`), as an OCaml ledger
    /// database holds them. Each ledger's root is computed and must equal its
    /// expected hash. Runtime settings are initialized before constructing
    /// the ledgers and preserved in the resulting config.
    ///
    /// This is how a network's genesis is built from the RocksDB ledger
    /// tarballs OCaml publishes for it, whose config file names the ledgers
    /// by hash and has no accounts. OCaml loads such a ledger as it is and
    /// fails if its root is not the config's hash.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_ledger_helper/genesis_ledger_helper.ml#L424-L463>
    pub fn from_ledger_accounts(
        runtime_config: GenesisRuntimeConfig,
        constants: ProtocolConstants,
        genesis: (Vec<ledger::Account>, LedgerHash),
        staking_epoch: (Vec<ledger::Account>, LedgerHash, v2::EpochSeed),
        next_epoch: (Vec<ledger::Account>, LedgerHash, v2::EpochSeed),
    ) -> Result<Self, GenesisConfigError> {
        runtime_config.initialize()?;
        let (accounts, ledger_hash, hashes) = prebuilt_ledger(genesis.0, &genesis.1)?;
        let epoch_data = |(accounts, hash, seed)| {
            let (accounts, ledger_hash, hashes) = prebuilt_ledger(accounts, &hash)?;
            Ok::<_, GenesisConfigError>(PrebuiltGenesisEpochData {
                accounts,
                ledger_hash,
                hashes,
                seed,
            })
        };
        Ok(Self {
            runtime_config,
            constants,
            accounts,
            ledger_hash,
            hashes,
            staking_epoch_data: epoch_data(staking_epoch)?,
            next_epoch_data: epoch_data(next_epoch)?,
        })
    }

    #[allow(clippy::result_unit_err)]
    pub fn from_loaded(
        (masks, data): (Vec<ledger::Mask>, GenesisConfigLoaded),
    ) -> Result<Self, ()> {
        let find_mask_by_hash = |hash: &v2::LedgerHash| {
            masks
                .iter()
                .find(|&m| m.clone().merkle_root() == hash.to_field().unwrap())
                .ok_or(())
        };
        let inner_hashes = |mask: &ledger::Mask| {
            mask.get_raw_inner_hashes()
                .into_iter()
                .map(|(idx, hash)| (idx, v2::LedgerHash::from_fp(hash)))
                .collect()
        };
        let genesis_mask = find_mask_by_hash(&data.genesis_ledger_hash)?;
        let epoch_data = |hash: v2::LedgerHash, seed: v2::EpochSeed| {
            find_mask_by_hash(&hash).map(|mask| PrebuiltGenesisEpochData {
                accounts: mask.to_list().into_iter().map(Into::into).collect(),
                ledger_hash: hash,
                hashes: inner_hashes(mask),
                seed,
            })
        };
        Ok(Self {
            runtime_config: *data.runtime_config,
            constants: data.constants,
            accounts: genesis_mask.to_list().into_iter().map(Into::into).collect(),
            ledger_hash: data.genesis_ledger_hash,
            hashes: inner_hashes(genesis_mask),
            staking_epoch_data: epoch_data(
                data.staking_epoch_ledger_hash,
                data.staking_epoch_seed,
            )?,
            next_epoch_data: epoch_data(data.next_epoch_ledger_hash, data.next_epoch_seed)?,
        })
    }
}

/// Builds one ledger of a [`PrebuiltGenesisConfig`] from its accounts, in
/// ledger index order, and checks its root: the accounts in wire format, the
/// root and the inner hashes.
#[allow(clippy::type_complexity)]
fn prebuilt_ledger(
    accounts: Vec<ledger::Account>,
    expected_hash: &LedgerHash,
) -> Result<
    (
        Vec<MinaBaseAccountBinableArgStableV2>,
        LedgerHash,
        Vec<(u64, LedgerHash)>,
    ),
    GenesisConfigError,
> {
    let wire_accounts = accounts.iter().map(Into::into).collect();
    let (mut mask, _total_currency) =
        GenesisConfig::build_ledger_from_accounts(accounts.into_iter().map(Ok))?;
    let ledger_hash = ledger_hash(&mut mask);
    if &ledger_hash != expected_hash {
        return Err(GenesisConfigError::LedgerHashMismatch {
            expected: expected_hash.clone(),
            computed: ledger_hash,
        });
    }
    let hashes = mask
        .get_raw_inner_hashes()
        .into_iter()
        .map(|(idx, hash)| (idx, v2::LedgerHash::from_fp(hash)))
        .collect();
    Ok((wire_accounts, ledger_hash, hashes))
}

/// Reads one ledger of a [`PrebuiltGenesisConfig`] (its accounts, root hash and
/// inner hashes, in that order) into a new ledger, one account at a time.
fn read_prebuilt_ledger(
    bytes: &mut &[u8],
) -> Result<(ledger::Mask, v2::CurrencyAmountStableV1, LedgerHash), GenesisConfigError> {
    let len = binprot::Nat0::binprot_read(bytes)?.0;
    let mut decode_error = None;
    let accounts =
        (0..len).map_while(
            |_| match MinaBaseAccountBinableArgStableV2::binprot_read(bytes) {
                Ok(account) => Some((&account).try_into()),
                Err(error) => {
                    decode_error = Some(error);
                    None
                }
            },
        );
    let (mask, total_currency) = GenesisConfig::build_ledger_from_accounts(accounts)?;
    if let Some(error) = decode_error {
        return Err(error.into());
    }
    let ledger_hash = LedgerHash::binprot_read(bytes)?;
    let hashes = Vec::<(u64, LedgerHash)>::binprot_read(bytes)?
        .into_iter()
        .map(|(n, h)| Ok((n, h.to_field()?)))
        .collect::<Result<Vec<_>, InvalidBigInt>>()?;
    // Must happen after the accounts have been set to avoid cache
    // invalidations.
    mask.set_raw_inner_hashes(hashes);
    Ok((mask, total_currency, ledger_hash))
}

#[derive(Debug, Serialize, Deserialize, BinProtRead, BinProtWrite)]
pub struct PrebuiltGenesisEpochData {
    accounts: Vec<MinaBaseAccountBinableArgStableV2>,
    ledger_hash: LedgerHash,
    hashes: Vec<(u64, LedgerHash)>,
    seed: v2::EpochSeed,
}

impl TryFrom<DaemonJson> for PrebuiltGenesisConfig {
    type Error = GenesisConfigError;

    fn try_from(config: DaemonJson) -> Result<Self, Self::Error> {
        let runtime_config = GenesisRuntimeConfig::from_daemon(&config);
        runtime_config.initialize()?;
        let constants = config
            .genesis
            .as_ref()
            .map_or(PROTOCOL_CONSTANTS, |genesis| genesis.protocol_constants());
        let ledger = config.ledger.as_ref().ok_or(GenesisConfigError::NoLedger)?;
        let ledger_accounts = ledger
            .accounts_with_genesis_winner()
            .iter()
            .map(daemon_json::Account::to_account)
            .collect::<Result<Vec<_>, _>>()?;
        let accounts = ledger_accounts.iter().map(Into::into).collect();
        let (mut mask, _total_currency) =
            GenesisConfig::build_ledger_from_accounts(ledger_accounts.into_iter().map(Ok))?;
        let ledger_hash = ledger_hash(&mut mask);
        if let Some(expected_hash) = &ledger.hash {
            if expected_hash != &ledger_hash.to_string() {
                return Err(GenesisConfigError::LedgerHashMismatch {
                    expected: expected_hash.parse().unwrap(),
                    computed: ledger_hash,
                });
            }
        }
        let hashes = mask
            .get_raw_inner_hashes()
            .into_iter()
            .map(|(idx, hash)| (idx, v2::LedgerHash::from_fp(hash)))
            .collect();
        let daemon_json::Epochs { staking, next } = config.epoch_data.unwrap();
        let staking_epoch_data = staking.try_into().unwrap();
        let next_epoch_data = next.unwrap().try_into().unwrap();
        let result = PrebuiltGenesisConfig {
            runtime_config,
            constants,
            accounts,
            ledger_hash,
            hashes,
            staking_epoch_data,
            next_epoch_data,
        };
        Ok(result)
    }
}

impl TryFrom<EpochData> for PrebuiltGenesisEpochData {
    type Error = GenesisConfigError;

    fn try_from(value: EpochData) -> Result<Self, Self::Error> {
        let EpochData {
            accounts,
            hash,
            s3_data_hash: _,
            seed,
        } = value;

        let expected_ledger_hash = hash.unwrap().parse().unwrap();
        let seed = seed.parse().unwrap();
        let ledger_accounts = accounts
            .unwrap()
            .iter()
            .map(daemon_json::Account::to_account)
            .collect::<Result<Vec<_>, _>>()?;
        let (accounts, ledger_hash, hashes) =
            prebuilt_ledger(ledger_accounts, &expected_ledger_hash)?;

        Ok(Self {
            accounts,
            ledger_hash,
            hashes,
            seed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_cache_readers_never_observe_partial_write() {
        use std::sync::mpsc;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.bin");
        let old = b"complete old cache";
        let new = b"complete replacement cache";
        std::fs::write(&path, old).unwrap();
        // Keep a reader open before publication, as the bin_prot loader does.
        let mut open_reader = File::open(&path).unwrap();
        let (partial_tx, partial_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let writer_path = &path;
            let writer = scope.spawn(move || {
                write_ledger_cache(writer_path, |file| {
                    file.write_all(&new[..4])?;
                    partial_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                    file.write_all(&new[4..])
                })
                .unwrap();
            });
            partial_rx.recv().unwrap();
            let mut observed = Vec::new();
            let read_result = open_reader.read_to_end(&mut observed);
            let fresh_read = std::fs::read(&path);
            // Release and join before asserting, so failures cannot deadlock.
            resume_tx.send(()).unwrap();
            writer.join().unwrap();
            read_result.unwrap();
            assert_eq!(observed, old, "already-open reader saw a partial cache");
            assert_eq!(fresh_read.unwrap(), old, "new reader saw a partial cache");
            assert_eq!(std::fs::read(&path).unwrap(), new);
        });
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn ledger_cache_failed_write_preserves_published_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.bin");
        let old = b"complete old cache";
        std::fs::write(&path, old).unwrap();
        let error = write_ledger_cache(&path, |file| {
            file.write_all(b"partial")?;
            Err(std::io::Error::other("injected write failure"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "injected write failure");
        assert_eq!(std::fs::read(&path).unwrap(), old);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn ledger_cache_concurrent_binprot_readers_and_writers() {
        use std::sync::Barrier;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.bin");
        // The OCaml rehearsal root is a fixed wire-format input here. This
        // tests cache I/O, not the relationship between a root and accounts.
        let hash: LedgerHash = "jwk9QANSsLrKqwKVNv4iNuv6ZCRojrxu3M6bnNBV1kVKDVGhB4P"
            .parse()
            .unwrap();
        let cache = LedgerAccountsWithHash {
            ledger_hash: hash.clone(),
            accounts: Vec::new(),
            hashes: (0..256).map(|index| (index, hash.clone())).collect(),
        };
        write_ledger_cache(&path, |file| cache.binprot_write(file)).unwrap();
        let start = Barrier::new(4);
        std::thread::scope(|scope| {
            for worker in 0..4 {
                let (path, cache, start) = (&path, &cache, &start);
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..64 {
                        if worker < 2 {
                            write_ledger_cache(path, |file| cache.binprot_write(file)).unwrap();
                        } else {
                            let mut reader = BufReader::new(File::open(path).unwrap());
                            let loaded = LedgerAccountsWithHash::binprot_read(&mut reader).unwrap();
                            assert_eq!(loaded.ledger_hash, cache.ledger_hash);
                            assert!(loaded.accounts.is_empty());
                            assert_eq!(loaded.hashes, cache.hashes);
                            assert_eq!(reader.read(&mut [0]).unwrap(), 0);
                        }
                    }
                });
            }
        });
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    fn rehearsal_daemon() -> DaemonJson {
        let mut config: DaemonJson = serde_json::from_str(include_str!(
            "../../../../../tests/files/fork-export/fork/rust/fork_config.json"
        ))
        .unwrap();
        // These tests exercise the ledger loader with the process's release
        // constants; the accounts and expected roots are OCaml's rehearsal.
        config.proof = None;
        config
    }

    #[test]
    fn absent_epoch_hashes_reuse_staking_ledger_with_next_seed() {
        // OCaml genesis_ledger_helper.ml:643-668 compares Option hashes,
        // including None = None, before loading the next ledger.
        let mut config = rehearsal_daemon();
        let epochs = config.epoch_data.as_mut().unwrap();
        epochs.staking.hash = None;
        let next = epochs.next.as_mut().unwrap();
        next.hash = None;
        let next_seed: v2::EpochSeed = next.seed.parse().unwrap();
        let (_, loaded) = GenesisConfig::DaemonJson(Box::new(config)).load().unwrap();
        assert_eq!(
            loaded.staking_epoch_ledger_hash.to_string(),
            "jwk9QANSsLrKqwKVNv4iNuv6ZCRojrxu3M6bnNBV1kVKDVGhB4P"
        );
        assert_eq!(
            loaded.next_epoch_ledger_hash,
            loaded.staking_epoch_ledger_hash
        );
        assert_eq!(
            loaded.next_epoch_total_currency,
            loaded.staking_epoch_total_currency
        );
        assert_eq!(loaded.next_epoch_seed, next_seed);
    }

    #[test]
    fn hash_only_daemon_and_missing_epoch_accounts_return_clear_errors() {
        let hash_only: DaemonJson = serde_json::from_str(include_str!(
            "../../../../../tests/files/fork-export/fork/rust/daemon.json"
        ))
        .unwrap();
        for (field, mut config) in [
            ("ledger.accounts", hash_only),
            ("epoch_data.staking.accounts", rehearsal_daemon()),
            ("epoch_data.next.accounts", rehearsal_daemon()),
        ] {
            config.proof = None;
            match field {
                "epoch_data.staking.accounts" => {
                    config.epoch_data.as_mut().unwrap().staking.accounts = None
                }
                "epoch_data.next.accounts" => {
                    config
                        .epoch_data
                        .as_mut()
                        .unwrap()
                        .next
                        .as_mut()
                        .unwrap()
                        .accounts = None
                }
                _ => {}
            }
            let result =
                std::panic::catch_unwind(|| GenesisConfig::DaemonJson(Box::new(config)).load());
            let error = result
                .expect("missing arrays must not panic")
                .unwrap_err()
                .to_string();
            assert!(error.contains(field) && error.contains("inline"), "{error}");
        }
    }

    #[test]
    fn prebuilt_runtime_header_and_legacy_payload() {
        let runtime_config: GenesisRuntimeConfig = serde_json::from_value(serde_json::json!({
            "proof": {"fork": {"state_hash": "3NKHyxzgM9z4C1wUz5ZhJBmNhtDzDQGr1PGvWRwiCUcFvsdTco2T",
                "blockchain_length": 548146, "global_slot_since_genesis": 958440},
                "coinbase_amount": "360", "block_window_duration_ms": 90000},
            "daemon": {"slot_tx_end": 10, "slot_chain_end": 20}
        }))
        .unwrap();
        let epoch = || PrebuiltGenesisEpochData {
            accounts: vec![],
            ledger_hash: v2::LedgerHash::zero(),
            hashes: vec![],
            seed: v2::EpochSeed::zero(),
        };
        let prebuilt = PrebuiltGenesisConfig {
            runtime_config: runtime_config.clone(),
            constants: PROTOCOL_CONSTANTS.clone(),
            accounts: vec![],
            ledger_hash: v2::LedgerHash::zero(),
            hashes: vec![],
            staking_epoch_data: epoch(),
            next_epoch_data: epoch(),
        };
        let mut bytes = Vec::new();
        prebuilt.store(&mut bytes).unwrap();
        let decoded = PrebuiltGenesisConfig::read(bytes.as_slice()).unwrap();
        assert_eq!(
            serde_json::to_value(&decoded).unwrap(),
            serde_json::to_value(&prebuilt).unwrap()
        );
        let config = GenesisConfig::from_bytes(bytes.clone()).unwrap();
        assert_eq!(config.protocol_constants().unwrap(), PROTOCOL_CONSTANTS);
        assert_eq!(
            serde_json::to_value(config.runtime_config().unwrap()).unwrap(),
            serde_json::to_value(runtime_config).unwrap()
        );
        let (_, payload) = prebuilt_payload(&bytes).unwrap();
        let legacy = GenesisConfig::from_bytes(payload.to_vec()).unwrap();
        assert!(legacy.runtime_config().unwrap().proof.is_none());
        assert_eq!(legacy.protocol_constants().unwrap(), PROTOCOL_CONSTANTS);
        assert!(PrebuiltGenesisConfig::read(payload)
            .unwrap()
            .runtime_config
            .daemon
            .is_none());
        for length in 8..12 {
            assert!(GenesisConfig::Prebuilt(bytes[..length].to_vec().into())
                .runtime_config()
                .is_err());
        }
        let mut truncated = bytes[..12].to_vec();
        truncated[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(GenesisConfig::Prebuilt(truncated.into())
            .runtime_config()
            .is_err());
    }

    #[test]
    fn without_ledgers_round_trip() {
        let config = GenesisConfig::Counts {
            whales: 1,
            fish: 1,
            non_stakers: NonStakers::None,
            constants: GenesisConfig::default_constants(DEFAULT_GENESIS_TIMESTAMP_MILLISECONDS),
        };
        let (masks, loaded) = config.load().unwrap();
        assert!(!masks.is_empty());

        let config = GenesisConfig::from_bytes(serde_json::to_vec(&loaded).unwrap()).unwrap();
        assert!(matches!(config, GenesisConfig::WithoutLedgers(_)));
        let (masks, reloaded) = config.load().unwrap();
        assert!(masks.is_empty());
        assert_eq!(
            serde_json::to_value(&reloaded).unwrap(),
            serde_json::to_value(&loaded).unwrap()
        );
        assert_eq!(config.protocol_constants().unwrap(), loaded.constants);
    }

    fn accounts(balances: &[u64], token_id: ledger::TokenId) -> Vec<ledger::Account> {
        balances
            .iter()
            .enumerate()
            .map(|(i, balance)| {
                let public_key = AccountSecretKey::deterministic(i as u64)
                    .public_key()
                    .try_into()
                    .unwrap();
                let id = ledger::AccountId::new(public_key, token_id.clone());
                ledger::Account::create_with(id, Balance::from_u64(*balance))
            })
            .collect()
    }

    fn root(accounts: &[ledger::Account]) -> LedgerHash {
        let (mut mask, _) =
            GenesisConfig::build_ledger_from_accounts(accounts.iter().cloned().map(Ok)).unwrap();
        ledger_hash(&mut mask)
    }

    #[test]
    fn prebuilt_from_ledger_accounts() {
        let genesis = accounts(&[1, 20, 300], ledger::TokenId::default());
        // An account of another token doesn't count towards the total
        // currency.
        let mut staking = accounts(&[4000, 50000], ledger::TokenId::default());
        staking.extend(accounts(&[7], ledger::TokenId(Fp::from(7u64))));
        let next = accounts(&[600000], ledger::TokenId::default());
        let staking_seed = v2::EpochSeed::zero();
        let next_seed: v2::EpochSeed = "2vbVSm3AnPnhSaTBdQeU65gBAJmFiG6hHixXBQjeQ9MEBrtepDiL"
            .parse()
            .unwrap();
        let constants = GenesisConfig::default_constants(DEFAULT_GENESIS_TIMESTAMP_MILLISECONDS);
        let hashes = [root(&genesis), root(&staking), root(&next)];
        assert_ne!(hashes[0], hashes[1]);

        let config = PrebuiltGenesisConfig::from_ledger_accounts(
            GenesisRuntimeConfig::default(),
            constants.clone(),
            (genesis.clone(), hashes[0].clone()),
            (staking.clone(), hashes[1].clone(), staking_seed.clone()),
            (next.clone(), hashes[2].clone(), next_seed.clone()),
        )
        .unwrap();
        assert_eq!(config.ledger_hashes(), [&hashes[0], &hashes[1], &hashes[2]]);

        // It is stored and loaded like any prebuilt config.
        let mut bytes = Vec::new();
        config.store(&mut bytes).unwrap();
        let (masks, loaded) = PrebuiltGenesisConfig::load_from_bytes(&bytes).unwrap();
        assert_eq!(loaded.constants, constants);
        assert_eq!(loaded.genesis_ledger_hash, hashes[0]);
        assert_eq!(loaded.staking_epoch_ledger_hash, hashes[1]);
        assert_eq!(loaded.next_epoch_ledger_hash, hashes[2]);
        assert_eq!(loaded.staking_epoch_seed, staking_seed);
        assert_eq!(loaded.next_epoch_seed, next_seed);
        let amount = |nanomina: u64| v2::CurrencyAmountStableV1(nanomina.into());
        assert_eq!(loaded.genesis_total_currency, amount(321));
        assert_eq!(loaded.staking_epoch_total_currency, amount(54000));
        assert_eq!(loaded.next_epoch_total_currency, amount(600000));
        let roots: Vec<_> = masks
            .into_iter()
            .map(|mut mask| ledger_hash(&mut mask))
            .collect();
        assert_eq!(roots, [hashes[0].clone(), hashes[1].clone()]);

        // A ledger whose root is not the expected one is refused.
        let error = PrebuiltGenesisConfig::from_ledger_accounts(
            GenesisRuntimeConfig::default(),
            constants,
            (genesis, hashes[0].clone()),
            (staking, hashes[1].clone(), staking_seed),
            (next, hashes[0].clone(), next_seed),
        )
        .unwrap_err();
        assert!(
            matches!(
                &error,
                GenesisConfigError::LedgerHashMismatch { expected, computed }
                    if expected == &hashes[0] && computed == &hashes[2]
            ),
            "{error}"
        );
    }
}
