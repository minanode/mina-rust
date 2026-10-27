//! `mina ledger`: OCaml's ledger commands for operators.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/init/client.ml#L2551-L2561>

pub mod export;
pub mod file;

use super::{node_api, Network};

#[derive(Debug, clap::Args)]
pub struct Ledger {
    #[command(subcommand)]
    pub command: LedgerCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum LedgerCommand {
    /// Write a ledger of a running node's best tip, in OCaml-compatible JSON
    /// (compact account lines, rather than OCaml's pretty-printed whitespace).
    Export(export::Export),
    /// Print the Merkle root of the ledger in an exported ledger file.
    Hash(file::Hash),
    /// Print the total currency of each token in an exported ledger file.
    Currency(file::Currency),
}

impl Ledger {
    pub fn run(self, network: Network) -> anyhow::Result<()> {
        let result = match self.command {
            LedgerCommand::Export(cmd) => cmd.run(network),
            LedgerCommand::Hash(cmd) => cmd.run(),
            LedgerCommand::Currency(cmd) => cmd.run(),
        };
        if let Err(err) = result {
            node_api::exit_with(err);
        }
        Ok(())
    }
}
