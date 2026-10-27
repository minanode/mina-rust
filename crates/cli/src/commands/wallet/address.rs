use std::path::PathBuf;

use anyhow::{Context, Result};
use mina_node_account::AccountSecretKey;

#[derive(Debug, clap::Args)]
pub struct Address {
    /// The encrypted key file. Its password is read from the
    /// `MINA_PRIVKEY_PASS` environment variable.
    #[arg(long, env = "MINA_FROM", value_name = "FILE")]
    pub from: PathBuf,

    /// Refused: a password on the command line would be visible to other
    /// processes; it is read from the MINA_PRIVKEY_PASS environment
    /// variable instead.
    #[arg(hide = true, value_parser = crate::commands::secret_value::refused_privkey_password())]
    pub password: Option<String>,

    /// Refused as the positional is: `--password` and the other names an
    /// operator may try (RC7-FIXES).
    #[arg(
        long = "password",
        hide = true,
        aliases = ["pass", "privkey-pass", "privkey-password"],
        num_args = 0..=1,
        default_missing_value = "",
        value_parser = crate::commands::secret_value::refused_privkey_password()
    )]
    pub password_flag: Option<String>,
}

impl Address {
    pub fn run(self) -> Result<()> {
        debug_assert!(self.password.is_none() && self.password_flag.is_none());
        let password = crate::commands::secret_value::password_from_env("MINA_PRIVKEY_PASS")?;
        // Load and decrypt the key
        let secret_key = AccountSecretKey::from_encrypted_file(&self.from, &password)
            .with_context(|| format!("Failed to decrypt key file: {}", self.from.display()))?;

        // Get the public key
        let public_key = secret_key.public_key();

        // Display the address
        println!("{}", public_key);

        Ok(())
    }
}
