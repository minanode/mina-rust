//! `mina ledger export`, against a running node's HTTP API.

use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::{
    file::ledger_file_hash,
    node_api::{self, HashMismatch, NodeRefused},
    Network,
};

/// The ledgers of `mina ledger export`, by OCaml's names.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ExportKind {
    /// The best tip's staged ledger
    StagedLedger,
    /// The best tip's snarked ledger
    SnarkedLedger,
    /// The ledger the current epoch stakes from
    StakingEpochLedger,
    /// The ledger the next epoch will stake from
    NextEpochLedger,
}

impl ExportKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::StagedLedger => "staged-ledger",
            Self::SnarkedLedger => "snarked-ledger",
            Self::StakingEpochLedger => "staking-epoch-ledger",
            Self::NextEpochLedger => "next-epoch-ledger",
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct Export {
    /// Which ledger of the node's best tip.
    #[arg(value_enum)]
    pub kind: ExportKind,

    /// The node's HTTP address (its `--port`). The node must run with
    /// `--enable-ledger-export`.
    #[arg(
        long,
        default_value = "http://localhost:3000",
        env = "MINA_NODE_URL",
        value_name = "URL"
    )]
    pub node: String,

    /// Write the accounts to this file instead of standard output; a summary
    /// goes to standard output.
    #[arg(long, short, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Recompute the written file's Merkle root and exit with status 4 unless
    /// it is the node's (needs `--output`).
    #[arg(long, requires = "output")]
    pub verify: bool,
}

impl Export {
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/init/client.ml#L774-L839>
    pub fn run(self, network: Network) -> Result<()> {
        let status = node_api::check_node(&self.node, &network, false)?;
        let url = format!(
            "{}/ledger/export/{}",
            node_api::base_url(&self.node),
            self.kind.as_str()
        );
        // A mainnet ledger is about 300 MB of JSON.
        let mut response = node_api::client(Duration::from_secs(900))?
            .get(&url)
            .send()
            .with_context(|| format!("failed to reach the node at {url}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let enable_hint = if status.as_u16() == 403 || status.as_u16() == 404 {
                "; start an operator node with --enable-ledger-export and use its private HTTP endpoint (public GraphQL does not serve exports)"
            } else {
                ""
            };
            let body: Value = response.json().unwrap_or(Value::Null);
            let message = body["error"].as_str().unwrap_or("no error message");
            return Err(NodeRefused(format!(
                "the node did not export the {}: HTTP {status}, {message}{enable_hint}",
                self.kind.as_str()
            ))
            .into());
        }
        let header = |name: &str| -> Result<String> {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
                .with_context(|| format!("the node's answer has no {name} header"))
        };
        let ledger_hash = header("x-mina-ledger-hash")?;
        let num_accounts: u64 = header("x-mina-num-accounts")?.parse()?;
        let state_hash = header("x-mina-state-hash")?;
        let blockchain_length: u64 = header("x-mina-blockchain-length")?.parse()?;
        let global_slot: u64 = header("x-mina-global-slot-since-genesis")?.parse()?;
        let epoch: u64 = header("x-mina-epoch")?.parse()?;

        let bytes = match &self.output {
            Some(path) => {
                let file = File::create(path)
                    .with_context(|| format!("failed to create {}", path.display()))?;
                let mut out = BufWriter::new(file);
                let bytes = response.copy_to(&mut out)?;
                out.flush()?;
                bytes
            }
            None => {
                let mut out = io::stdout().lock();
                let bytes = response.copy_to(&mut out)?;
                out.flush()?;
                bytes
            }
        };

        let mut summary = json!({
            "kind": self.kind.as_str(),
            "ledger_hash": ledger_hash,
            "num_accounts": num_accounts,
            "state_hash": state_hash,
            "blockchain_length": blockchain_length,
            "global_slot_since_genesis": global_slot,
            "epoch": epoch,
            "network_id": status.network_id,
            "sync_status": status.sync_status,
            "bytes": bytes,
            "output": self.output.as_ref().map(|p| p.display().to_string()),
        });
        let mismatch = if self.verify {
            let path = self.output.as_ref().expect("clap requires --output");
            let (recomputed, _) = ledger_file_hash(path)?;
            summary["recomputed_ledger_hash"] = json!(recomputed);
            (recomputed != ledger_hash).then(|| {
                HashMismatch(format!(
                    "{} recomputes to {recomputed}, the node said {ledger_hash}",
                    path.display()
                ))
            })
        } else {
            None
        };
        // The accounts may be on standard output: the summary then goes to
        // standard error.
        if self.output.is_some() {
            println!("{summary}");
        } else {
            eprintln!("{summary}");
        }
        match mismatch {
            Some(mismatch) => Err(mismatch.into()),
            None => Ok(()),
        }
    }
}
