use std::{
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
};

use anyhow::Context;
use mina_node::{
    daemon_json::DaemonJson,
    transition_frontier::genesis::{GenesisConfig, GenesisRuntimeConfig, PrebuiltGenesisConfig},
};
use mina_p2p_messages::v2;
use reqwest::Url;

use structopt::StructOpt;

mod rocksdb_dump;
mod tarball;

#[derive(StructOpt)]
struct Args {
    /// A daemon.json config. With the three `--*-dump` options, its ledgers
    /// need only their hashes, and their accounts come from the dumps.
    #[structopt(short, long)]
    input: Option<PathBuf>,
    /// A daemon.json config to download.
    #[structopt(long)]
    url: Option<Url>,
    /// A prebuilt config, as written by this tool, instead of a daemon.json.
    #[structopt(long)]
    prebuilt: Option<PathBuf>,
    /// The genesis ledger, as a `mina-rocksdb-scanner dump` of its RocksDB
    /// tarball. Needs `--input` and the other two dumps.
    #[structopt(long, requires_all = &["input", "staking-dump", "next-dump"])]
    genesis_dump: Option<PathBuf>,
    /// The staking epoch ledger, dumped in the same way.
    #[structopt(long, requires = "genesis-dump")]
    staking_dump: Option<PathBuf>,
    /// The next epoch ledger, dumped in the same way.
    #[structopt(long, requires = "genesis-dump")]
    next_dump: Option<PathBuf>,
    /// Print where OCaml publishes the RocksDB tarball of each of the
    /// config's ledgers, one line each: the ledger (genesis, staking or
    /// next), its hash, the tarball's URL and the config's `s3_data_hash`
    /// (the tarball's SHA3-256), then exit.
    #[structopt(long, requires = "input")]
    tarball_urls: bool,
    /// Write only what the genesis block commits to, as JSON, without the
    /// ledgers. Enough for a node that syncs its ledgers from peers.
    #[structopt(long)]
    without_ledgers: bool,
    /// Recompute all three roots from prebuilt accounts, ignoring stored roots
    /// and cached hashes, and write the roots as JSON. Does not start a node.
    #[structopt(long, requires = "prebuilt", conflicts_with = "without-ledgers")]
    recompute_ledger_roots: bool,
    #[structopt(short, long, required_unless = "tarball-urls")]
    output: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let Args {
        input,
        url,
        prebuilt,
        genesis_dump,
        staking_dump,
        next_dump,
        tarball_urls,
        without_ledgers,
        recompute_ledger_roots,
        output,
    } = Args::from_args();

    if tarball_urls {
        let input = input.expect("structopt requires --input");
        let config = serde_json::from_slice::<DaemonJson>(&fs::read(input)?)?;
        for ledger in TarballLedgers::of(&config)?.0 {
            println!(
                "{} {} {}/{} {}",
                ledger.name,
                ledger.hash,
                tarball::S3_BUCKET,
                tarball::tarball_name(ledger.tarball_prefix, &ledger.hash),
                ledger.s3_data_hash.as_deref().unwrap_or("-"),
            );
        }
        return Ok(());
    }
    let output = output.expect("structopt requires --output");

    let prebuilt_config = if let Some(prebuilt) = prebuilt {
        PrebuiltGenesisConfig::read(File::open(prebuilt)?)?
    } else if let Some(genesis_dump) = genesis_dump {
        let input = input.expect("structopt requires --input");
        let config = serde_json::from_slice::<DaemonJson>(&fs::read(input)?)?;
        let staking_dump = staking_dump.expect("structopt requires --staking-dump");
        let next_dump = next_dump.expect("structopt requires --next-dump");
        from_dumps(&config, [&genesis_dump, &staking_dump, &next_dump])?
    } else {
        let data = if let Some(input) = input {
            fs::read(input)?
        } else if let Some(url) = url {
            reqwest::blocking::get(url)?.bytes()?.to_vec()
        } else {
            anyhow::bail!("must provide either `--input`, `--url` or `--prebuilt`");
        };
        let daemon_json = serde_json::from_slice::<DaemonJson>(&data)?;
        PrebuiltGenesisConfig::try_from(daemon_json)?
    };

    if recompute_ledger_roots {
        let [genesis, staking, next] = prebuilt_config.recompute_ledger_hashes()?;
        serde_json::to_writer(
            File::create(output)?,
            &serde_json::json!({
                "genesis_ledger_hash": genesis,
                "staking_epoch_ledger_hash": staking,
                "next_epoch_ledger_hash": next,
            }),
        )?;
        return Ok(());
    }

    let [genesis, staking, next] = prebuilt_config.ledger_hashes();
    println!("genesis ledger {genesis}\nstaking epoch ledger {staking}\nnext epoch ledger {next}");

    if without_ledgers {
        let (_masks, loaded) = prebuilt_config.load()?;
        serde_json::to_writer(File::create(&output)?, &loaded)?;
        // Check that it reads back as a genesis config.
        let config = GenesisConfig::from_bytes(fs::read(&output)?)?;
        anyhow::ensure!(
            matches!(config, GenesisConfig::WithoutLedgers(_)),
            "the written config does not read back as one without ledgers"
        );
    } else {
        prebuilt_config.store(File::create(output)?)?;
    }

    Ok(())
}

/// One of the three ledgers of a config whose ledgers are RocksDB tarballs.
struct TarballLedger {
    name: &'static str,
    tarball_prefix: &'static str,
    hash: v2::LedgerHash,
    s3_data_hash: Option<String>,
    seed: Option<v2::EpochSeed>,
}

/// The genesis, staking epoch and next epoch ledgers of a config.
struct TarballLedgers([TarballLedger; 3]);

impl TarballLedgers {
    fn of(config: &DaemonJson) -> anyhow::Result<Self> {
        let ledger = config.ledger.as_ref().context("the config has no ledger")?;
        let hash = |hash: Option<&String>, name| -> anyhow::Result<v2::LedgerHash> {
            let hash = hash.with_context(|| format!("the {name} ledger has no hash"))?;
            hash.parse()
                .map_err(|error| anyhow::anyhow!("the {name} ledger's hash {hash}: {error:?}"))
        };
        let epochs = config
            .epoch_data
            .as_ref()
            .context("the config has no epoch ledgers")?;
        let next = epochs
            .next
            .as_ref()
            .context("the config has no next epoch ledger")?;
        let epoch = |data: &mina_node::daemon_json::EpochData, name| {
            Ok::<_, anyhow::Error>(TarballLedger {
                name,
                tarball_prefix: tarball::EPOCH_LEDGER_PREFIX,
                hash: hash(data.hash.as_ref(), name)?,
                s3_data_hash: data.s3_data_hash.clone(),
                seed: Some(
                    data.seed
                        .parse()
                        .map_err(|error| anyhow::anyhow!("the {name} seed: {error:?}"))?,
                ),
            })
        };
        Ok(Self([
            TarballLedger {
                name: "genesis",
                tarball_prefix: tarball::GENESIS_LEDGER_PREFIX,
                hash: hash(ledger.hash.as_ref(), "genesis")?,
                s3_data_hash: ledger.s3_data_hash.clone(),
                seed: None,
            },
            epoch(&epochs.staking, "staking")?,
            epoch(next, "next")?,
        ]))
    }
}

/// A prebuilt config from a daemon.json that names its ledgers by hash, and
/// the dumps of their RocksDB tarballs, in the order genesis, staking epoch,
/// next epoch.
fn from_dumps(config: &DaemonJson, dumps: [&Path; 3]) -> anyhow::Result<PrebuiltGenesisConfig> {
    let runtime_config = GenesisRuntimeConfig::from_daemon(config);
    runtime_config.initialize()?;
    let constants = config
        .genesis
        .as_ref()
        .map_or(v2::PROTOCOL_CONSTANTS, |genesis| {
            genesis.protocol_constants()
        });
    let ledger_depth = mina_node::core::constants::constraint_constants().ledger_depth as usize;
    let [genesis, staking, next] = TarballLedgers::of(config)?.0;
    let read = |ledger: &TarballLedger, path: &Path| -> anyhow::Result<Vec<ledger::Account>> {
        let started = std::time::Instant::now();
        let dump = rocksdb_dump::read(BufReader::new(File::open(path)?), ledger_depth)
            .with_context(|| format!("{} ledger dump {path:?}", ledger.name))?;
        anyhow::ensure!(
            dump.root == ledger.hash,
            "the {} ledger dump {path:?} has root {}, not the config's {}",
            ledger.name,
            dump.root,
            ledger.hash
        );
        println!(
            "{} ledger {}: {} accounts ({} of other tokens), total currency {}, read in {:.1} s",
            ledger.name,
            ledger.hash,
            dump.accounts.len(),
            dump.custom_token_accounts(),
            dump.total_currency(),
            started.elapsed().as_secs_f64(),
        );
        Ok(dump.accounts)
    };
    let genesis_accounts = read(&genesis, dumps[0])?;
    let staking_accounts = read(&staking, dumps[1])?;
    let next_accounts = read(&next, dumps[2])?;

    let started = std::time::Instant::now();
    let config = PrebuiltGenesisConfig::from_ledger_accounts(
        runtime_config,
        constants,
        (genesis_accounts, genesis.hash),
        (
            staking_accounts,
            staking.hash,
            staking.seed.expect("epoch ledgers have a seed"),
        ),
        (
            next_accounts,
            next.hash,
            next.seed.expect("epoch ledgers have a seed"),
        ),
    )?;
    println!(
        "rebuilt the three ledgers, every root equal to the config's, in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    Ok(config)
}
