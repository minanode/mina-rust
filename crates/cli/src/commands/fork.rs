//! `mina fork export`: the next hard fork's runtime config and ledgers from a
//! running node's own state, as OCaml 4.0.2's
//! `mina advanced generate-hardfork-config` writes them from its daemon.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/init/client.ml#L2255-L2290>

use std::{
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Context, Result};
use mina_node::{
    daemon_json::{self, HardForkManifest},
    ledger::{merkle_root_of_leaves, migrated_empty_account, LEDGER_DEPTH},
};
use mina_p2p_messages::v2::LedgerHash;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{
    node_api::{self, HashMismatch, NodeRefused},
    Network,
};

#[derive(Debug, clap::Args)]
pub struct Fork {
    #[command(subcommand)]
    pub command: ForkCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum ForkCommand {
    /// Write the next hard fork's runtime config and ledgers from a running
    /// node's state at the stop slots.
    Export(Export),
}

impl Fork {
    pub fn run(self, network: Network) -> Result<()> {
        let result = match self.command {
            ForkCommand::Export(cmd) => cmd.run(network),
        };
        if let Err(err) = result {
            node_api::exit_with(err);
        }
        Ok(())
    }
}

/// Files `mina fork export` writes into `--out`, in writing order.
pub const FILES: [&str; 5] = [
    "fork_config.json",
    "daemon.json",
    "daemon-migrated.json",
    "manifest.json",
    "activated",
];

#[derive(Debug, clap::Args)]
pub struct Export {
    /// The node's HTTP address (its `--port`). The node must run with
    /// `--enable-ledger-export`.
    #[arg(
        long,
        default_value = "http://localhost:3000",
        env = "MINA_NODE_URL",
        value_name = "URL"
    )]
    pub node: String,

    /// Directory to create. It must not exist; it gets `fork_config.json`
    /// (every account inline, OCaml's GraphQL `fork_config` layout),
    /// `daemon.json` and `daemon-migrated.json` (ledgers by stable and by
    /// OCaml 4.0.2's migrated root), `manifest.json` and, last, an empty
    /// `activated`.
    #[arg(long, value_name = "DIR")]
    pub out: PathBuf,

    /// Continue from this best-chain block instead of the newest block
    /// before `slot_tx_end`.
    #[arg(long, conflicts_with = "height", value_name = "HASH")]
    pub state_hash: Option<String>,

    /// Continue from the best-chain block at this height.
    #[arg(long, value_name = "HEIGHT")]
    pub height: Option<u32>,

    /// Export even if the best tip has not reached slot_tx_end, so that the
    /// fork block may still change
    #[arg(long)]
    pub allow_before_stop: bool,

    /// Recompute the three ledgers' stable and migrated roots from
    /// fork_config.json and exit with status 4 unless they are the node's
    #[arg(long)]
    pub verify: bool,
}

impl Export {
    pub fn run(self, network: Network) -> Result<()> {
        if self.out.exists() {
            bail!(
                "{} already exists; choose a new directory, as OCaml's \
                 generate-hardfork-config requires",
                self.out.display()
            );
        }
        let status = node_api::check_node(&self.node, &network, false)?;
        let mut url = reqwest::Url::parse(&format!(
            "{}/hardfork/config",
            node_api::base_url(&self.node)
        ))?;
        {
            let mut query = url.query_pairs_mut();
            if let Some(hash) = &self.state_hash {
                query.append_pair("state_hash", hash);
            } else if let Some(height) = self.height {
                query.append_pair("height", &height.to_string());
            }
            if self.allow_before_stop {
                query.append_pair("allow_before_stop", "true");
            }
        }
        // Three ledgers; mainnet's are about 300 MB of JSON each.
        let response = node_api::client(Duration::from_secs(1800))?
            .get(url.clone())
            .send()
            .with_context(|| format!("failed to reach the node at {url}"))?;
        if !response.status().is_success() {
            let code = response.status();
            let body: Value = response.json().unwrap_or(Value::Null);
            let message = body["error"].as_str().unwrap_or("no error message");
            return Err(NodeRefused(format!(
                "the node did not export the hard fork config: HTTP {code}, {message}"
            ))
            .into());
        }
        let mut body = BufReader::with_capacity(1 << 20, response);
        let mut line = String::new();
        body.read_line(&mut line)
            .context("failed to read the export's manifest")?;
        let manifest: HardForkManifest =
            serde_json::from_str(&line).context("the export's manifest is not valid")?;
        if manifest.format != HardForkManifest::FORMAT {
            bail!("unknown export format {}", manifest.format);
        }
        if !manifest.inputs.stop_reached && !self.allow_before_stop {
            return Err(NodeRefused(format!(
                "the best tip (slot {}) has not reached slot_tx_end {:?}: a block before it may \
                 still arrive; wait, or pass --allow-before-stop",
                manifest.inputs.best_tip.curr_global_slot_since_hard_fork,
                manifest.inputs.stop_slots.slot_tx_end
            ))
            .into());
        }

        // Write next to the target, then rename, so a partial export never
        // looks complete.
        let parent = self
            .out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let name = self
            .out
            .file_name()
            .context("--out has no directory name")?
            .to_string_lossy()
            .into_owned();
        let build = parent.join(format!(".{name}.partial-{}", std::process::id()));
        fs::create_dir(&build).with_context(|| format!("failed to create {}", build.display()))?;

        let _partial = PartialExport(build.clone());
        let mut files = serde_json::Map::new();
        let (bytes, sha256) = copy_hashing(&mut body, &build.join(FILES[0]))?;
        files.insert(FILES[0].into(), json!({ "bytes": bytes, "sha256": sha256 }));
        for (file, config) in [
            (FILES[1], &manifest.daemon),
            (FILES[2], &manifest.daemon_migrated),
        ] {
            let mut text = serde_json::to_vec(config)?;
            text.push(b'\n');
            fs::write(build.join(file), &text)?;
            files.insert(
                file.into(),
                json!({ "bytes": text.len(), "sha256": hex::encode(Sha256::digest(&text)) }),
            );
        }

        let mut summary = json!({
            "out": self.out.display().to_string(),
            "fork": manifest.inputs.fork,
            "genesis_state_timestamp": manifest.inputs.genesis_state_timestamp,
            "stop_reached": manifest.inputs.stop_reached,
            "ledgers": manifest.ledgers,
            "network_id": status.network_id,
        });
        let mismatch = if self.verify {
            let verified = verify_fork_config(&build.join(FILES[0]), &manifest)?;
            summary["verified"] = verified.0;
            verified.1
        } else {
            None
        };

        // Verification is part of construction: a mismatch must never write
        // an activation marker or publish the package.
        if let Some(mismatch) = mismatch {
            return Err(mismatch.into());
        }
        let mut record = serde_json::to_value(&manifest)?;
        record["files"] = Value::Object(files);
        record["source"] = json!({
            "node": self.node,
            "network_id": status.network_id,
            "sync_status": status.sync_status,
            "cli_commit": mina_node::BuildEnv::get().git.commit_hash,
        });
        fs::write(
            build.join(FILES[3]),
            serde_json::to_string_pretty(&record)? + "\n",
        )?;
        File::create(build.join(FILES[4]))?;
        fs::rename(&build, &self.out).with_context(|| {
            format!(
                "failed to move {} to {}",
                build.display(),
                self.out.display()
            )
        })?;
        println!("{summary}");
        Ok(())
    }
}

/// Remove only this invocation's temporary directory on every failure. Once
/// renamed successfully the old path no longer exists.
struct PartialExport(PathBuf);

impl Drop for PartialExport {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Copies the rest of `body` to `path`, returning its size and SHA-256.
fn copy_hashing(body: &mut impl Read, path: &Path) -> Result<(u64, String)> {
    let file =
        File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    let mut out = BufWriter::new(file);
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 16];
    let mut total = 0u64;
    loop {
        let n = body.read(&mut buf).context("failed to read the export")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        total += n as u64;
    }
    out.flush()?;
    Ok((total, hex::encode(hasher.finalize())))
}

/// Recomputes each ledger's stable and migrated root from `fork_config.json`.
fn verify_fork_config(
    path: &Path,
    manifest: &HardForkManifest,
) -> Result<(Value, Option<HashMismatch>)> {
    let file = File::open(path)?;
    let config: daemon_json::DaemonJson = serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("{} is not a runtime config", path.display()))?;
    let epochs = config.epoch_data.context("no epoch_data")?;
    let ledgers = [
        (
            "ledger",
            config.ledger.and_then(|l| l.accounts),
            &manifest.ledgers.ledger,
        ),
        (
            "staking",
            epochs.staking.accounts,
            &manifest.ledgers.staking,
        ),
        (
            "next",
            epochs.next.and_then(|n| n.accounts),
            &manifest.ledgers.next,
        ),
    ];
    let mut report = serde_json::Map::new();
    let mut mismatches = Vec::new();
    for (name, accounts, expected) in ledgers {
        let accounts = accounts.with_context(|| format!("{name} has no accounts"))?;
        let leaves = accounts
            .iter()
            .map(|a| a.to_account().map(|a| a.hash()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
        let root = |empty| LedgerHash::from_fp(merkle_root_of_leaves(&leaves, LEDGER_DEPTH, empty));
        let stable = root(ledger::Account::empty().hash()).to_string();
        let migrated = root(migrated_empty_account().hash()).to_string();
        if stable != expected.hash || migrated != expected.migrated_hash {
            mismatches.push(format!(
                "{name}: recomputed {stable} / {migrated}, the node said {} / {}",
                expected.hash, expected.migrated_hash
            ));
        }
        report.insert(
            name.into(),
            json!({ "hash": stable, "migrated_hash": migrated, "num_accounts": accounts.len() }),
        );
    }
    let mismatch = (!mismatches.is_empty()).then(|| HashMismatch(mismatches.join("; ")));
    Ok((Value::Object(report), mismatch))
}
