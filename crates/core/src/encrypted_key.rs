//! # Encrypted Secret Key Implementation
//!
//! This module provides a unified interface for encrypting and decrypting
//! cryptographic secret keys used throughout the Mina node. It implements
//! password-based encryption compatible with the Mina Protocol's key format.
//!
//! ## Usage
//!
//! This module is used by:
//! - Block producer keys ([`AccountSecretKey`]) for signing blocks and transactions
//! - P2P networking keys ([`SecretKey`]) for node identity and peer authentication
//!
//! [`AccountSecretKey`]: ../../../node/account/struct.AccountSecretKey.html
//! [`SecretKey`]: ../../../p2p/identity/struct.SecretKey.html
//!
//! ## Encryption Algorithms
//!
//! The implementation uses industry-standard cryptographic algorithms:
//!
//! ### Key Derivation
//! - **Argon2i**: Password-based key derivation function (PBKDF) with
//!   configurable memory cost and time cost parameters
//! - **Default parameters**: 128MB memory cost, 6 iterations
//! - **Salt**: 32-byte random salt generated using OS entropy
//!
//! ### Symmetric Encryption
//! - **XSalsa20Poly1305**: Authenticated encryption with associated data (AEAD)
//! - **Key size**: 256-bit derived from password via Argon2i
//! - **Nonce**: 192-bit random nonce generated per encryption
//! - **Authentication**: Poly1305 MAC for ciphertext integrity
//!
//! ### Encoding
//! - **Base58**: All encrypted data (nonce, salt, ciphertext) encoded in
//!   Base58 with version bytes for format compatibility with Mina Protocol
//! - **Version byte**: 2 for encryption data format compatibility
//!
//! ## File Format
//!
//! Encrypted keys are stored in JSON format with the following structure:
//! ```json
//! {
//!   "box_primitive": "xsalsa20poly1305",
//!   "pw_primitive": "argon2i",
//!   "nonce": "base58-encoded-nonce",
//!   "pwsalt": "base58-encoded-salt",
//!   "pwdiff": [memory_cost_bytes, time_cost_iterations],
//!   "ciphertext": "base58-encoded-encrypted-key"
//! }
//! ```
//!
//! This format ensures compatibility with existing Mina Protocol tooling and
//! wallet implementations.
//!
//! ## Reference Implementation
//!
//! The encryption format is based on the OCaml implementation in the Mina
//! repository:
//! [`src/lib/secret_box`](https://github.com/MinaProtocol/mina/tree/develop/src/lib/secret_box)

use std::{fs, path::Path};

use argon2::{password_hash::SaltString, Argon2, Block};
use base64::Engine;
use crypto_secretbox::{
    aead::{Aead, OsRng},
    AeadCore, KeyInit, XSalsa20Poly1305,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize, Debug)]
struct Base58String(String);

impl Base58String {
    pub fn new(raw: &[u8], version: u8) -> Self {
        Base58String(bs58::encode(raw).with_check_version(version).into_string())
    }

    pub fn try_decode(&self, version: u8) -> Result<Vec<u8>, EncryptionError> {
        let decoded = bs58::decode(&self.0).with_check(Some(version)).into_vec()?;
        Ok(decoded[1..].to_vec())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EncryptionError {
    #[error(transparent)]
    SecretBox(#[from] crypto_secretbox::aead::Error),
    #[error(transparent)]
    ArgonError(#[from] argon2::Error),
    #[error(transparent)]
    PasswordHash(#[from] argon2::password_hash::Error),
    #[error(transparent)]
    Base58DecodeError(#[from] bs58::decode::Error),
    #[error(transparent)]
    CipherKeyInvalidLength(#[from] crypto_secretbox::cipher::InvalidLength),
    #[error("invalid encrypted key file: {0}")]
    InvalidFormat(&'static str),
    #[error("invalid encrypted key file: unsupported primitive")]
    UnsupportedPrimitive,
    #[error(
        "key file asks for an unreasonably expensive derivation \
         ({memory} bytes, {iterations} iterations)"
    )]
    TooExpensive { memory: u32, iterations: u32 },
    #[error("Password hash missing after hash_password")]
    HashMissing,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
    #[error("Other: {0}")]
    Other(String),
}

/// Represents the JSON structure of an encrypted secret key file.
///
/// This structure defines the format used to store encrypted secret keys on
/// disk, compatible with the Mina Protocol's key file format. The file
/// contains all necessary cryptographic parameters for decryption.
///
/// # JSON Format
/// When serialized, this structure produces a JSON file with the following
/// format:
/// ```json
/// {
///   "box_primitive": "xsalsa20poly1305",
///   "pw_primitive": "argon2i",
///   "nonce": "base58-encoded-nonce-with-version-byte",
///   "pwsalt": "base58-encoded-salt-with-version-byte",
///   "pwdiff": [memory_cost_in_bytes, time_cost_iterations],
///   "ciphertext": "base58-encoded-encrypted-key-with-version-byte"
/// }
/// ```
///
/// # Security Considerations
/// - The `nonce` must be unique for each encryption operation
/// - The `pwsalt` should be cryptographically random
/// - The `pwdiff` parameters determine the computational cost of key
///   derivation
/// - All Base58-encoded fields include version bytes for format validation
#[derive(Serialize, Deserialize, Debug)]
pub struct EncryptedSecretKeyFile {
    /// Symmetric encryption algorithm identifier.
    /// Always "xsalsa20poly1305" for compatibility.
    box_primitive: String,

    /// Password-based key derivation function identifier.
    /// Always "argon2i" for compatibility.
    pw_primitive: String,

    /// Encryption nonce encoded in Base58 with version byte.
    /// Used once per encryption to ensure semantic security.
    nonce: Base58String,

    /// Argon2 salt encoded in Base58 with version byte.
    /// Random value used in password-based key derivation.
    pwsalt: Base58String,

    /// Argon2 parameters as (memory_cost_bytes, time_cost_iterations).
    /// Determines computational difficulty of key derivation.
    pwdiff: (u32, u32),

    /// Encrypted secret key encoded in Base58 with version byte.
    /// Contains the actual encrypted key data with authentication tag.
    ciphertext: Base58String,
}

impl EncryptedSecretKeyFile {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, EncryptionError> {
        let file = fs::File::open(path)?;
        reject_public_key_file(&file)?;
        Ok(serde_json::from_reader(file)?)
    }

    /// The Argon2i cost the file asks for: (memory in bytes, iterations).
    pub fn pwdiff(&self) -> (u32, u32) {
        self.pwdiff
    }
}

/// The most expensive derivation a key file may ask for: 1 GiB of memory
/// and 64 iterations. Our own files use 128 MiB and 6 (`PW_DIFF`); the
/// bounds leave room for stronger future parameters while keeping a
/// tampered file from making every start pay an unbounded cost before
/// the password is even tried.
const MAX_PWDIFF: (u32, u32) = (1024 * 1024 * 1024, 64);

fn setup_argon(pwdiff: (u32, u32)) -> Result<Argon2<'static>, EncryptionError> {
    if pwdiff.0 > MAX_PWDIFF.0 || pwdiff.1 > MAX_PWDIFF.1 {
        return Err(EncryptionError::TooExpensive {
            memory: pwdiff.0,
            iterations: pwdiff.1,
        });
    }
    let params = argon2::Params::new(
        pwdiff.0 / 1024,
        pwdiff.1,
        argon2::Params::DEFAULT_P_COST,
        None,
    )?;

    Ok(Argon2::new(
        argon2::Algorithm::Argon2i,
        Default::default(),
        params,
    ))
}

/// The 32-byte key Argon2i derives from `password` and `salt`: what
/// `PasswordHasher::hash_password` computes with these parameters, but with
/// the key and the derivation's memory wiped when dropped.
fn derive_key(
    argon2: &Argon2,
    password: &[u8],
    salt: &SaltString,
) -> Result<Zeroizing<[u8; 32]>, EncryptionError> {
    let mut salt_buffer = [0u8; 64];
    let salt = salt.decode_b64(&mut salt_buffer)?;
    let mut memory = Zeroizing::new(vec![Block::default(); argon2.params().block_count()]);
    let mut key = Zeroizing::new([0u8; 32]);
    argon2
        .hash_password_into_with_memory(password, salt, &mut key[..], &mut memory[..])
        .map_err(argon2::password_hash::Error::from)?;
    Ok(key)
}

/// Refuses a key file that other users could read, as OCaml's Secret_file
/// does for the file itself (OCaml also demands a 0700 directory, which we
/// deliberately do not, to keep operators' existing key locations
/// working). Checked on the open file's metadata, not on the path, so the
/// file checked is the file read.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/crypto/secrets/secret_file.ml#L99-L106>
#[cfg(unix)]
pub fn reject_public_key_file(file: &fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let permissions = file.metadata()?.permissions();
    if permissions.mode() & 0o077 != 0 {
        return Err(std::io::Error::other(format!(
            "key file permissions are too open ({:o}); they should be 0600. \
             Hint: chmod 600 the file",
            permissions.mode() & 0o777
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn reject_public_key_file(_file: &fs::File) -> std::io::Result<()> {
    Ok(())
}

pub trait EncryptedSecretKey {
    const ENCRYPTION_DATA_VERSION_BYTE: u8 = 2;
    const SECRET_KEY_PREFIX_BYTE: u8 = 1;

    // Based on the OCaml implementation at:
    // https://github.com/MinaProtocol/mina/tree/develop/src/lib/secret_box
    const BOX_PRIMITIVE: &'static str = "xsalsa20poly1305";
    const PW_PRIMITIVE: &'static str = "argon2i";
    // Note: Only used for encryption, for decryption use the pwdiff from the
    // file
    const PW_DIFF: (u32, u32) = (134217728, 6);

    /// Decrypts an encrypted secret key file using the provided password.
    ///
    /// This method implements the decryption process compatible with Mina
    /// Protocol's key format:
    /// 1. Decodes Base58-encoded nonce, salt, and ciphertext from the file
    /// 2. Derives encryption key from password using Argon2i with file's
    ///    parameters
    /// 3. Decrypts the ciphertext using XSalsa20Poly1305 AEAD
    /// 4. Returns plaintext for the account or P2P decoder to validate
    ///
    /// # Parameters
    /// - `encrypted`: The encrypted key file structure containing all
    ///   encryption metadata
    /// - `password`: The password used to derive the decryption key
    ///
    /// # Returns
    /// - `Ok(Vec<u8>)`: The raw secret key bytes on successful decryption
    /// - `Err(EncryptionError)`: Various errors including wrong password,
    ///   corrupted data, or format incompatibility
    ///
    /// # Errors
    /// - `EncryptionError::SecretBox`: AEAD decryption failure (wrong
    ///   password)
    /// - `EncryptionError::Base58DecodeError`: Invalid Base58 encoding
    /// - `EncryptionError::ArgonError`: Key derivation failure
    fn try_decrypt(
        encrypted: &EncryptedSecretKeyFile,
        password: &str,
    ) -> Result<Vec<u8>, EncryptionError> {
        // Check the shared envelope before key derivation or infallible
        // fixed-size conversions. Account and P2P plaintext formats differ;
        // their decoders validate their own plaintext lengths below this layer.
        if encrypted.box_primitive != Self::BOX_PRIMITIVE
            || encrypted.pw_primitive != Self::PW_PRIMITIVE
        {
            return Err(EncryptionError::UnsupportedPrimitive);
        }
        // prepare inputs to cipher
        let password = password.as_bytes();
        let pwsalt = encrypted
            .pwsalt
            .try_decode(Self::ENCRYPTION_DATA_VERSION_BYTE)?;
        let nonce = encrypted
            .nonce
            .try_decode(Self::ENCRYPTION_DATA_VERSION_BYTE)?;
        let ciphertext = encrypted
            .ciphertext
            .try_decode(Self::ENCRYPTION_DATA_VERSION_BYTE)?;

        let nonce: &[u8; 24] = nonce
            .as_slice()
            .try_into()
            .map_err(|_| EncryptionError::InvalidFormat("invalid nonce length"))?;
        if ciphertext.len() < 16 {
            return Err(EncryptionError::InvalidFormat(
                "ciphertext shorter than authentication tag",
            ));
        }

        // The argon crate's SaltString can only be built from base64 string,
        // but the OCaml Mina node encodes the salt in base58. So we decode it
        // from base58 first, then convert to base64 and lastly to SaltString
        let pwsalt_encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(pwsalt);
        let salt = SaltString::from_b64(&pwsalt_encoded)?;

        let argon2 = setup_argon(encrypted.pwdiff)?;
        let password_key = derive_key(&argon2, password, &salt)?;

        // decrypt cipher
        let cipher = XSalsa20Poly1305::new_from_slice(&password_key[..])?;
        let decrypted = cipher.decrypt(nonce.into(), ciphertext.as_ref())?;

        // Plaintext formats differ for account and P2P keys. The caller
        // wipes it.
        Ok(decrypted)
    }

    /// Encrypts a secret key using password-based encryption.
    ///
    /// This method implements the encryption process compatible with Mina
    /// Protocol's key format:
    /// 1. Prefixes the key with a format version byte
    /// 2. Generates a random salt and derives encryption key using Argon2i
    /// 3. Encrypts the prefixed key using XSalsa20Poly1305 AEAD with a
    ///    random nonce
    /// 4. Encodes all components (nonce, salt, ciphertext) in Base58 format
    /// 5. Returns the complete encrypted file structure
    ///
    /// # Parameters
    /// - `key`: The raw secret key bytes to encrypt
    /// - `password`: The password used to derive the encryption key
    ///
    /// # Returns
    /// - `Ok(EncryptedSecretKeyFile)`: Complete encrypted file structure
    ///   ready for JSON serialization
    /// - `Err(EncryptionError)`: Encryption process failure
    ///
    /// # Errors
    /// - `EncryptionError::ArgonError`: Key derivation failure
    /// - `EncryptionError::SecretBox`: AEAD encryption failure
    ///
    /// # Security Notes
    /// - Uses cryptographically secure random number generation for salt
    ///   and nonce
    /// - Default Argon2i parameters: 128MB memory cost, 6 iterations
    /// - Each encryption produces unique salt and nonce for security
    fn try_encrypt(key: &[u8], password: &str) -> Result<EncryptedSecretKeyFile, EncryptionError> {
        // add the prefix byte to the key
        let mut key_prefixed = Zeroizing::new(vec![Self::SECRET_KEY_PREFIX_BYTE]);
        key_prefixed.extend(key);

        Self::try_encrypt_raw(&key_prefixed, password)
    }

    /// Encrypt raw bytes without prepending a version/prefix byte.
    /// Used for account/BP keys, use [`EncryptedSecretKey::try_encrypt`] which adds the
    /// standard 0x01 prefix byte.
    fn try_encrypt_raw(
        plaintext: &[u8],
        password: &str,
    ) -> Result<EncryptedSecretKeyFile, EncryptionError> {
        let argon2 = setup_argon(Self::PW_DIFF)?;

        let salt = SaltString::generate(&mut OsRng);
        let password_key = derive_key(&argon2, password.as_bytes(), &salt)?;

        let nonce = XSalsa20Poly1305::generate_nonce(&mut OsRng);
        let cipher = XSalsa20Poly1305::new_from_slice(&password_key[..])?;

        let ciphertext = cipher.encrypt(&nonce, plaintext)?;

        // Same reason as in decrypt, we need to decode the SaltString from
        // base64 then encode it to base58 below
        let mut salt_bytes = [0; 32];
        let salt_portion = salt.decode_b64(&mut salt_bytes)?;

        Ok(EncryptedSecretKeyFile {
            box_primitive: Self::BOX_PRIMITIVE.to_string(),
            pw_primitive: Self::PW_PRIMITIVE.to_string(),
            nonce: Base58String::new(&nonce, Self::ENCRYPTION_DATA_VERSION_BYTE),
            pwsalt: Base58String::new(salt_portion, Self::ENCRYPTION_DATA_VERSION_BYTE),
            pwdiff: (argon2.params().m_cost() * 1024, argon2.params().t_cost()),
            ciphertext: Base58String::new(&ciphertext, Self::ENCRYPTION_DATA_VERSION_BYTE),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestKey;
    impl EncryptedSecretKey for TestKey {
        const PW_DIFF: (u32, u32) = (8192, 1);
    }

    fn encrypted() -> EncryptedSecretKeyFile {
        TestKey::try_encrypt_raw(b"synthetic plaintext", "test password").unwrap()
    }

    #[test]
    fn derive_key_is_what_hash_password_computes() {
        use argon2::PasswordHasher;
        let argon2 = setup_argon(TestKey::PW_DIFF).unwrap();
        let salt = SaltString::generate(&mut OsRng);
        let expected = argon2
            .hash_password(b"test password", &salt)
            .unwrap()
            .hash
            .unwrap();
        let key = derive_key(&argon2, b"test password", &salt).unwrap();
        assert_eq!(&key[..], expected.as_bytes());
    }

    #[test]
    fn malformed_nonce_lengths_return_errors() {
        for len in [0, 1, 23, 25, 32] {
            let mut file = encrypted();
            file.nonce = Base58String::new(&vec![0; len], 2);
            assert!(TestKey::try_decrypt(&file, "test password").is_err());
        }
    }

    #[test]
    fn malformed_salt_lengths_return_errors() {
        for len in [0, 1, 7, 65] {
            let mut file = encrypted();
            file.pwsalt = Base58String::new(&vec![0; len], 2);
            assert!(TestKey::try_decrypt(&file, "test password").is_err());
        }
    }

    #[test]
    fn malformed_ciphertext_lengths_return_errors() {
        for len in [0, 1, 15, 16, 17, 49] {
            let mut file = encrypted();
            file.ciphertext = Base58String::new(&vec![0; len], 2);
            assert!(TestKey::try_decrypt(&file, "test password").is_err());
        }
    }

    #[test]
    fn malformed_base58_checksum_and_versions_return_errors() {
        for field in ["nonce", "pwsalt", "ciphertext"] {
            for bad in [
                "0-invalid-base58".to_owned(),
                "111111111".to_owned(),
                bs58::encode([0; 24]).with_check_version(3).into_string(),
            ] {
                let mut json = serde_json::to_value(encrypted()).unwrap();
                json[field] = bad.into();
                let file = serde_json::from_value(json).unwrap();
                assert!(TestKey::try_decrypt(&file, "test password").is_err());
            }
        }
    }

    #[test]
    fn unsupported_primitives_return_errors() {
        for field in ["box_primitive", "pw_primitive"] {
            let mut json = serde_json::to_value(encrypted()).unwrap();
            json[field] = "unsupported".into();
            let file = serde_json::from_value(json).unwrap();
            assert!(matches!(
                TestKey::try_decrypt(&file, "test password"),
                Err(EncryptionError::UnsupportedPrimitive)
            ));
        }
    }

    /// The cost parameters come from the key file, so they are not trusted
    /// without limit: a file that asks for an unreasonably expensive
    /// derivation is refused before any of it runs. The caps leave room
    /// above our own parameters (128 MiB, 6 iterations).
    #[test]
    fn unreasonably_expensive_derivation_is_refused() {
        for pwdiff in [
            (u32::MAX, u32::MAX),
            (1024 * 1024 * 1024 + 1024, 6),
            (134_217_728, 65),
        ] {
            let error = match setup_argon(pwdiff) {
                Err(error) => error,
                Ok(_) => panic!("accepted {pwdiff:?}"),
            };
            let message = error.to_string();
            assert!(message.contains("expensive"), "{pwdiff:?}: {message}");
        }
        // Our own files' parameters still build the derivation.
        assert!(setup_argon((134_217_728, 6)).is_ok());
    }
}
