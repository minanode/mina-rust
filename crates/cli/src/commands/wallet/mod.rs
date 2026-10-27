pub mod address;
pub mod balance;
pub mod delegate;
pub mod generate;
pub mod send;
pub mod status;

use super::Network;
use crate::exit_with_error;

/// An amount or fee in MINA, as OCaml's `mina client send-payment` and
/// `delegate-stake` take it (`0.1`), in nanomina. Up to 9 decimals; more
/// precision than a nanomina, a sign, or a value over `u64::MAX` nanomina is
/// refused rather than cut, as `--snarker-fee` would cut it.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/currency/currency.ml#L98-L113>
pub fn parse_mina(value: &str) -> Result<u64, String> {
    if value.starts_with('-') {
        return Err("negative amounts are refused; expected MINA, like 0.1".to_owned());
    }
    if value
        .split_once('.')
        .is_some_and(|(_, decimals)| decimals.len() > 9)
    {
        return Err("more than 9 decimal places; the smallest unit is 0.000000001 MINA".to_owned());
    }
    mina_node::account::parse_snarker_fee(value).map_err(|error| {
        if error.contains("64 bits") {
            "too large: at most 18446744073.709551615 MINA".to_owned()
        } else {
            "expected MINA, like 0.1 or 2".to_owned()
        }
    })
}

#[derive(Debug, clap::Args)]
pub struct Wallet {
    #[command(subcommand)]
    pub command: WalletCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum WalletCommand {
    /// Print the public key of an encrypted key file.
    Address(address::Address),
    /// Print an account's balance and nonce, from a node's GraphQL API.
    Balance(balance::Balance),
    /// Delegate an account's stake, as OCaml's `mina client delegate-stake`.
    Delegate(delegate::Delegate),
    /// Generate an account key pair and save it as an encrypted key file.
    Generate(generate::Generate),
    /// Sign a payment and send it through a node.
    Send(send::Send),
    /// Print a transaction's status, from a node's GraphQL API, with a pooled
    /// transaction's amount and fee in MINA.
    Status(status::Status),
}

impl Wallet {
    pub fn run(self, network: Network) -> anyhow::Result<()> {
        let result = match self.command {
            WalletCommand::Address(cmd) => cmd.run(),
            WalletCommand::Balance(cmd) => cmd.run(),
            WalletCommand::Delegate(cmd) => cmd.run(network),
            WalletCommand::Generate(cmd) => cmd.run(),
            WalletCommand::Send(cmd) => cmd.run(network),
            WalletCommand::Status(cmd) => cmd.run(),
        };

        // Handle errors without backtraces for wallet commands
        if let Err(err) = result {
            exit_with_error(err);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::parse_mina;

    // CLI-REF: wallet amounts and fees are in MINA, as OCaml's.
    #[test]
    fn amounts_are_read_in_mina() {
        for (input, nanomina) in [
            ("0", 0),
            ("1", 1_000_000_000),
            ("0.1", 100_000_000),
            ("2.5", 2_500_000_000),
            ("0.000000001", 1),
            ("1.", 1_000_000_000),
            (".5", 500_000_000),
            ("18446744073.709551615", u64::MAX),
        ] {
            assert_eq!(parse_mina(input), Ok(nanomina), "{input}");
        }
        for (input, message) in [
            ("0.0000000001", "more than 9 decimal places"),
            ("1.1234567891", "more than 9 decimal places"),
            ("-1", "negative"),
            ("-0.1", "negative"),
            ("18446744073.709551616", "too large"),
            ("100000000000", "too large"),
            ("", "expected MINA"),
            (".", "expected MINA"),
            ("1e9", "expected MINA"),
            ("1,5", "expected MINA"),
            (" 1", "expected MINA"),
            ("+1", "expected MINA"),
            ("1.2.3", "expected MINA"),
        ] {
            let error = parse_mina(input).unwrap_err();
            assert!(error.contains(message), "{input:?}: {error}");
        }
    }
}
