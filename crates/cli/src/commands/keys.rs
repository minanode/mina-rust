use libp2p_identity::PeerId;
use mina_node::{account::AccountSecretKey, p2p::identity::SecretKey};
use std::io::Write;

use crate::commands::wallet::generate::Generate;

#[derive(Debug, clap::Args)]
pub struct Keys {
    #[command(subcommand)]
    command: KeysCommand,
}

impl Keys {
    pub fn run(self) -> anyhow::Result<()> {
        match self.command {
            KeysCommand::MinaKeyPair(command) => command.run(),
            KeysCommand::P2PKeyPair(command) => command.run(),
            KeysCommand::MinaEncryptedKey(command) => command.run(),
            KeysCommand::FrontierStoreCheck(command) => command.run(),
        }
    }
}

#[derive(Debug, clap::Subcommand)]
pub enum KeysCommand {
    MinaKeyPair(MinaKeyPair),
    P2PKeyPair(P2PKeyPair),
    /// The older name of `mina wallet generate`, which it runs.
    #[command(hide = true)]
    MinaEncryptedKey(Generate),
    /// The older path of `mina node frontier-store-check`, which it runs.
    #[command(hide = true)]
    FrontierStoreCheck(FrontierStoreCheck),
}

/// The `--verify` values of `node frontier-store-check`.
const VERIFY_MODES: [&str; 3] = ["full", "sample", "root"];

/// Check a frontier store directory offline, with the node stopped.
///
/// Loads the newest valid generation as a restart would, rehashing every
/// ledger by default, and checks the checkpoint's root staged ledger against
/// its block, the parts' protocol states and the chain's blocks. Writes
/// nothing. Exits with an error if a check fails. A running node locks its
/// store: stop it first, or pass `--no-lock`.
///
/// The store is `<chain id>/frontier` in the work directory, `/data` in
/// Docker.
///
/// Example: `mina node frontier-store-check ~/.mina/<chain id>/frontier`
#[derive(Debug, Clone, clap::Args)]
pub struct FrontierStoreCheck {
    /// The store: a chain's `frontier` directory.
    dir: std::path::PathBuf,
    /// How much of each ledger to rehash: `full` (every node), `sample` (64
    /// leaf paths, as a restart does) or `root`.
    #[arg(long, default_value = "full", value_parser = VERIFY_MODES)]
    verify: String,
    /// Also rebuild the root staged ledger the slow way, by applying the scan
    /// state's transactions again (tens of seconds on mainnet).
    #[arg(long)]
    rebuild_staged: bool,
    /// Check without taking the store's lock, while a node writes it.
    #[arg(long)]
    no_lock: bool,
}

impl FrontierStoreCheck {
    pub fn run(self) -> anyhow::Result<()> {
        use mina_node::persist::{
            check::{check_store, CheckOptions},
            VerifyMode,
        };
        let verify = match self.verify.as_str() {
            "full" => VerifyMode::Full,
            "sample" => VerifyMode::default(),
            "root" => VerifyMode::RootOnly,
            other => anyhow::bail!("--verify {other}: expected full, sample or root"),
        };
        let options = CheckOptions {
            verify,
            rebuild_staged: self.rebuild_staged,
            lock: !self.no_lock,
        };
        let mut out = std::io::stdout().lock();
        let summary = check_store(&self.dir, &options, &mut out)?;
        for warning in &summary.warnings {
            writeln!(out, "warning: {warning}")?;
        }
        for problem in &summary.problems {
            writeln!(out, "FAILED: {problem}")?;
        }
        if !summary.passed() {
            anyhow::bail!("the store failed {} checks", summary.problems.len());
        }
        writeln!(
            out,
            "PASSED: generation {}, {}, {} ledgers, {} blocks, {} files, {} bytes, {} \
             unreferenced files",
            summary
                .generation
                .map_or("none".to_owned(), |generation| generation.to_string()),
            if summary.has_checkpoint {
                "with a checkpoint"
            } else {
                "no checkpoint"
            },
            summary.ledgers,
            summary.blocks,
            summary.files,
            summary.bytes,
            summary.unreferenced.len(),
        )?;
        Ok(())
    }
}

/// Generate a libp2p key pair for a node's peer identity, and print it.
///
/// Prints the secret key, the public key and the peer id. The secret key
/// goes to `mina node --p2p-secret-key`.
#[derive(Debug, Clone, clap::Args)]
pub struct P2PKeyPair {
    /// Print the key pair of this secret key instead of a new one.
    #[arg(
        long,
        short = 's',
        env = "MINA_P2P_SEC_KEY",
        hide_env_values = true,
        value_name = "KEY",
        value_parser = crate::commands::secret_value::p2p_key()
    )]
    p2p_secret_key: Option<SecretKey>,
}

impl P2PKeyPair {
    pub fn run(self) -> anyhow::Result<()> {
        let secret_key = self.p2p_secret_key.unwrap_or_else(SecretKey::rand);
        let public_key = secret_key.public_key();
        let peer_id = public_key.peer_id();
        let libp2p_peer_id = PeerId::try_from(peer_id)?;
        println!("secret key: {secret_key}");
        println!("public key: {public_key}");
        println!("peer_id:    {peer_id}");
        println!("libp2p_id:  {libp2p_peer_id}");

        Ok(())
    }
}

/// Generate an unencrypted Mina account key pair, and print it.
///
/// Prints the secret key and the public key. For a key file a block producer
/// can use, run `mina wallet generate` instead.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct MinaKeyPair {
    /// Print the key pair of this secret key instead of a new one.
    #[arg(
        long,
        short = 's',
        env = "MINA_SEC_KEY",
        hide_env_values = true,
        value_name = "KEY",
        value_parser = crate::commands::secret_value::account_key()
    )]
    secret_key: Option<AccountSecretKey>,
}

impl MinaKeyPair {
    pub fn run(self) -> anyhow::Result<()> {
        let private_key = self.secret_key.unwrap_or_else(AccountSecretKey::rand);
        let public_key = private_key.public_key();

        println!("secret key: {private_key}");
        println!("public key: {public_key}");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mina_key_pair_generates_random_key() {
        let cmd = MinaKeyPair::default();

        let result = cmd.run();
        assert!(result.is_ok());
    }

    #[test]
    fn test_mina_key_pair_with_provided_secret_key() {
        let secret_key = AccountSecretKey::rand();
        let cmd = MinaKeyPair {
            secret_key: Some(secret_key),
        };

        let result = cmd.run();
        assert!(result.is_ok());
    }

    #[test]
    fn test_p2p_key_pair_generates_random_key() {
        let cmd = P2PKeyPair {
            p2p_secret_key: None,
        };

        let result = cmd.run();
        assert!(result.is_ok());
    }

    #[test]
    fn test_p2p_key_pair_with_provided_secret_key() {
        let secret_key = SecretKey::rand();
        let cmd = P2PKeyPair {
            p2p_secret_key: Some(secret_key),
        };

        let result = cmd.run();
        assert!(result.is_ok());
    }
}
