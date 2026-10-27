//! `mina ledger hash` and `mina ledger currency`: offline, on an exported
//! ledger file.

use std::{
    collections::BTreeMap,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use mina_node::daemon_json::{self, to_mina_string};
use mina_p2p_messages::v2::TokenIdKeyHash;
use serde_json::json;

use super::node_api::HashMismatch;

/// Reads a file `mina ledger export` (OCaml's or ours) wrote.
pub fn read_ledger_file(path: &Path) -> Result<Vec<daemon_json::Account>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("could not parse the ledger JSON in {}", path.display()))
}

/// Recomputes the Merkle root of a ledger file.
pub fn ledger_file_hash(path: &Path) -> Result<(String, usize)> {
    let accounts = read_ledger_file(path)?;
    let hash = daemon_json::ledger_hash_of_accounts(&accounts)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("bad account in {}", path.display()))?;
    Ok((hash.to_string(), accounts.len()))
}

#[derive(Debug, clap::Args)]
pub struct Hash {
    /// A ledger file as `mina ledger export` writes it (JSON).
    #[arg(long, value_name = "FILE")]
    pub ledger_file: PathBuf,

    /// Exit with status 4 unless the Merkle root is this ledger hash.
    #[arg(long, value_name = "HASH")]
    pub expect: Option<String>,

    /// Print a JSON object (`ledger_hash`, `num_accounts`) instead of the
    /// hash.
    #[arg(long)]
    pub json: bool,
}

impl Hash {
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/init/client.ml#L841-L888>
    pub fn run(self) -> Result<()> {
        let (hash, num_accounts) = ledger_file_hash(&self.ledger_file)?;
        if self.json {
            println!(
                "{}",
                json!({ "ledger_hash": hash, "num_accounts": num_accounts })
            );
        } else {
            println!("{hash}");
        }
        match self.expect {
            Some(expected) if expected != hash => {
                Err(HashMismatch(format!("ledger hash {hash}, expected {expected}")).into())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct Currency {
    /// A ledger file as `mina ledger export` writes it (JSON).
    #[arg(long, value_name = "FILE")]
    pub ledger_file: PathBuf,
}

impl Currency {
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/init/client.ml#L890-L957>
    pub fn run(self) -> Result<()> {
        let accounts = read_ledger_file(&self.ledger_file)?;
        // OCaml adds with UInt64 arithmetic, wrapping.
        let mut totals = BTreeMap::<ledger::TokenId, u64>::new();
        for account in &accounts {
            let token = account
                .token_id()
                .map_err(|e| anyhow::anyhow!("{e}"))
                .context("bad token id")?;
            let total = totals.entry(token).or_default();
            *total = total.wrapping_add(account.balance().as_u64());
        }
        for (token, total) in totals {
            if token.is_default() {
                println!("MINA: {}", to_mina_string(total));
            } else {
                println!(
                    "TOKEN {}: {}",
                    TokenIdKeyHash::from(token),
                    to_mina_string(total)
                );
            }
        }
        Ok(())
    }
}
