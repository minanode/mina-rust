use mina_node_account::AccountSecretKey;
use std::{io::Write, path::PathBuf};

/// The secret key variable `mina-encrypted-key` read before CLI-REF. Read
/// when `--secret-key` and `MINA_SEC_KEY` are not set; not in the help.
const OLD_SECRET_KEY_ENV: &str = "MINA_ENC_KEY";

/// Generate a Mina account key pair and save it as an encrypted key file.
///
/// The file is in the format OCaml's `mina advanced generate-keypair` writes,
/// and may be used as `mina node --producer-key`. The public key goes to the
/// same path with `.pub` appended. The password, which must not be empty, is
/// read from the `MINA_PRIVKEY_PASS` environment variable.
///
/// The secret key is only written, encrypted, to `--output`, which only its
/// owner may read (mode 0600). The command prints the public key, never the
/// secret key.
#[derive(Debug, Clone, clap::Args)]
pub struct Generate {
    /// The encrypted key file to write. Neither it nor its `.pub` file may
    /// exist.
    #[arg(long, alias = "file", short_alias = 'f', value_name = "FILE")]
    pub output: PathBuf,

    /// Encrypt this secret key instead of a new one.
    #[arg(
        long,
        short = 's',
        env = "MINA_SEC_KEY",
        hide_env_values = true,
        value_name = "KEY",
        value_parser = crate::commands::secret_value::account_key()
    )]
    pub secret_key: Option<AccountSecretKey>,

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

impl Generate {
    pub fn run(self) -> anyhow::Result<()> {
        debug_assert!(self.password.is_none() && self.password_flag.is_none());
        let password = crate::commands::secret_value::password_from_env("MINA_PRIVKEY_PASS")?;
        // The older variable is read only when neither `--secret-key` nor
        // MINA_SEC_KEY gave a key, so a stale value can't stop a run that
        // doesn't use it.
        let old_secret_key = match self.secret_key {
            Some(_) => None,
            None => crate::commands::secret_value::account_key_from_env(OLD_SECRET_KEY_ENV)?,
        };
        self.run_to(&mut std::io::stdout().lock(), &password, old_secret_key)
    }

    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/cli_lib/commands.ml#L9-L23>
    fn run_to(
        self,
        out: &mut impl Write,
        password: &str,
        old_secret_key: Option<AccountSecretKey>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !password.is_empty(),
            "a password is required: set the MINA_PRIVKEY_PASS environment variable"
        );
        let file = self.output.display().to_string();
        let public_key_file = format!("{file}.pub");
        anyhow::ensure!(!self.output.exists(), "{file} already exists");
        anyhow::ensure!(
            !std::path::Path::new(&public_key_file).exists(),
            "{public_key_file} already exists"
        );

        let secret_key = self
            .secret_key
            .or(old_secret_key)
            .unwrap_or_else(AccountSecretKey::rand);
        let public_key = secret_key.public_key();

        // The error names the path, never the key.
        secret_key
            .to_encrypted_file(&self.output, password)
            .map_err(|e| anyhow::anyhow!("failed to write the encrypted key to {file}: {e}"))?;
        std::fs::write(&public_key_file, public_key.to_string())
            .map_err(|e| anyhow::anyhow!("failed to write {public_key_file}: {e}"))?;
        writeln!(out, "Keypair generated")?;
        writeln!(out, "Public key: {public_key}")?;
        writeln!(out, "Encrypted key file: {file}")?;
        writeln!(out, "Public key file: {public_key_file}")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const PASSWORD: &str = "test_password";

    fn generate(output: PathBuf, secret_key: Option<AccountSecretKey>) -> Generate {
        Generate {
            output,
            secret_key,
            password: None,
            password_flag: None,
        }
    }

    #[test]
    fn a_new_key_is_written_encrypted() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("key.json");
        generate(file.clone(), None)
            .run_to(&mut Vec::new(), PASSWORD, None)
            .unwrap();
        let content = fs::read_to_string(&file).unwrap();
        assert!(content.starts_with('{') && content.ends_with('}'));
        let key = AccountSecretKey::from_encrypted_file(&file, PASSWORD).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("key.json.pub")).unwrap(),
            key.public_key().to_string()
        );
        assert!(AccountSecretKey::from_encrypted_file(&file, "wrong_password").is_err());
    }

    #[test]
    fn a_given_key_is_kept() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("key.json");
        let secret_key = AccountSecretKey::rand();
        generate(file.clone(), Some(secret_key.clone()))
            .run_to(&mut Vec::new(), PASSWORD, None)
            .unwrap();
        let loaded = AccountSecretKey::from_encrypted_file(&file, PASSWORD).unwrap();
        assert_eq!(loaded.to_string(), secret_key.to_string());
    }

    /// CLI-REF: `MINA_ENC_KEY` is read when neither `--secret-key` nor
    /// `MINA_SEC_KEY` is given, and loses to them.
    #[test]
    fn the_old_secret_key_variable_is_a_fallback() {
        let dir = TempDir::new().unwrap();
        let old = AccountSecretKey::rand();
        let file = dir.path().join("old");
        generate(file.clone(), None)
            .run_to(&mut Vec::new(), PASSWORD, Some(old.clone()))
            .unwrap();
        let loaded = AccountSecretKey::from_encrypted_file(&file, PASSWORD).unwrap();
        assert_eq!(loaded.public_key(), old.public_key());

        let new = AccountSecretKey::rand();
        let file = dir.path().join("new");
        generate(file.clone(), Some(new.clone()))
            .run_to(&mut Vec::new(), PASSWORD, Some(old))
            .unwrap();
        let loaded = AccountSecretKey::from_encrypted_file(&file, PASSWORD).unwrap();
        assert_eq!(loaded.public_key(), new.public_key());
    }

    #[test]
    fn an_empty_password_is_refused() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("key.json");
        let error = generate(file.clone(), None)
            .run_to(&mut Vec::new(), "", None)
            .unwrap_err();
        assert!(error.to_string().contains("MINA_PRIVKEY_PASS"), "{error}");
        assert!(!file.exists());
    }

    #[test]
    fn an_invalid_path_fails() {
        let file = PathBuf::from("/invalid/path/that/does/not/exist/key.json");
        assert!(generate(file, None)
            .run_to(&mut Vec::new(), PASSWORD, None)
            .is_err());
    }

    /// R35-key-output and P5.0a: the output names the public key and files,
    /// and holds no form of the secret key; the file is owner-only, as OCaml's
    /// `Secret_file.write` makes it.
    #[test]
    fn the_output_has_no_secret() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("producer-key");
        let file_str = file.to_str().unwrap().to_owned();
        let secret_key = AccountSecretKey::rand();
        let public_key = secret_key.public_key();
        let mut out = Vec::new();
        generate(file.clone(), Some(secret_key.clone()))
            .run_to(&mut out, PASSWORD, None)
            .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(
            out,
            format!(
                "Keypair generated\nPublic key: {public_key}\nEncrypted key file: \
                 {file_str}\nPublic key file: {file_str}.pub\n"
            )
        );
        assert!(!out.contains(&secret_key.to_string()));
        assert!(!out.contains(&hex::encode(secret_key.to_bytes())));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn existing_files_are_kept() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("producer-key");
        fs::write(&file, "existing").unwrap();
        let mut out = Vec::new();
        assert!(generate(file.clone(), None)
            .run_to(&mut out, PASSWORD, None)
            .is_err());
        assert!(out.is_empty());
        assert_eq!(fs::read_to_string(&file).unwrap(), "existing");

        let file = dir.path().join("other-key");
        fs::write(dir.path().join("other-key.pub"), "existing").unwrap();
        assert!(generate(file.clone(), None)
            .run_to(&mut out, PASSWORD, None)
            .is_err());
        assert!(!file.exists());
    }
}
