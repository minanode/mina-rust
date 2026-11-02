//! A Mina encrypted key file as text, for a caller that holds the file's
//! contents rather than a path: the web node's key page
//! (MINAWEB-KEYFILE-IMPORT). The format is the one OCaml's `Secret_file`
//! and `mina wallet generate` write: [`mina_core::EncryptedSecretKeyFile`].

use mina_core::{EncryptedSecretKey, EncryptedSecretKeyFile, EncryptionError};
use zeroize::Zeroizing;

use crate::AccountSecretKey;

/// Why a key file gave no key. Each value names a cause a page can show
/// without repeating anything from the file or the password; `Display`
/// gives a stable code for the page to map to its own text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeyFileError {
    /// The authentication failed: the password is wrong, or the
    /// ciphertext was changed.
    #[error("wrong-password")]
    WrongPassword,
    /// Not the JSON of an encrypted key file, or a field of it doesn't
    /// decode.
    #[error("not-a-key-file")]
    NotAKeyFile,
    /// A key file, but encrypted with a box or password primitive other
    /// than xsalsa20poly1305 and argon2i.
    #[error("unsupported")]
    Unsupported,
    /// It decrypts, but to something other than an account key (a libp2p
    /// key file, for one).
    #[error("not-an-account-key")]
    NotAnAccountKey,
    /// The file asks for more Argon2i memory or iterations than the
    /// caller allows.
    #[error("too-expensive")]
    TooExpensive,
}

impl From<EncryptionError> for KeyFileError {
    fn from(error: EncryptionError) -> Self {
        match error {
            EncryptionError::SecretBox(_) => Self::WrongPassword,
            EncryptionError::UnsupportedPrimitive => Self::Unsupported,
            EncryptionError::TooExpensive { .. } => Self::TooExpensive,
            _ => Self::NotAKeyFile,
        }
    }
}

impl AccountSecretKey {
    /// The account key in an encrypted key file's text. A file that asks
    /// for more than `max_memory` bytes of Argon2i memory is refused before
    /// any derivation runs (the core's own cap, 1 GiB and 64 iterations,
    /// applies as well).
    pub fn from_key_file_text(
        text: &str,
        password: &str,
        max_memory: u32,
    ) -> Result<Self, KeyFileError> {
        let encrypted: EncryptedSecretKeyFile =
            serde_json::from_str(text).map_err(|_| KeyFileError::NotAKeyFile)?;
        if encrypted.pwdiff().0 > max_memory {
            return Err(KeyFileError::TooExpensive);
        }
        let plaintext = Zeroizing::new(Self::try_decrypt(&encrypted, password)?);
        // An account key's plaintext is a version byte, 1, and the 32-byte
        // scalar (secret_key.rs, `from_encrypted`).
        match plaintext.split_first() {
            Some((&Self::SECRET_KEY_PREFIX_BYTE, scalar)) if scalar.len() == 32 => {
                Self::from_bytes(scalar).map_err(|_| KeyFileError::NotAnAccountKey)
            }
            _ => Err(KeyFileError::NotAnAccountKey),
        }
    }

    /// The key as an encrypted key file's text, with OCaml's default cost
    /// (128 MiB, 6 iterations): what [`AccountSecretKey::to_encrypted_file`]
    /// writes, without the file.
    pub fn to_key_file_text(&self, password: &str) -> Result<String, EncryptionError> {
        let encrypted = Self::try_encrypt(&*Zeroizing::new(self.to_bytes()), password)?;
        Ok(serde_json::to_string(&encrypted)?)
    }
}
