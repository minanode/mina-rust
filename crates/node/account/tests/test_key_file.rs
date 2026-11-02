//! MINAWEB-KEYFILE-IMPORT: an encrypted key file's text, as the web node's
//! key page reads it. The fixtures are throwaway keys made for these tests,
//! each with the password below: `keyfile-ocaml-4.0.2` by the OCaml 4.0.2
//! reference binary (`mina advanced generate-keypair --privkey-path`), and
//! `keyfile-native-0.20.1` by this node's `mina wallet generate` (0.20.1).
//! Each `.pub` is what that binary wrote next to it.

use std::fs;

use mina_core::EncryptedSecretKey;
use mina_node_account::{AccountSecretKey, KeyFileError};
use serde_json::Value;

const PASSWORD: &str = "keyfile-fixture-password";
/// What the web node allows: twice OCaml's default.
const MAX_MEMORY: u32 = 256 * 1024 * 1024;

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/../../../tests/files/accounts/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// A cheap derivation for the tests that build their own files: the cost
/// comes from the file, so decryption follows it.
struct Cheap;
impl EncryptedSecretKey for Cheap {
    const PW_DIFF: (u32, u32) = (8192, 1);
}

fn cheap_file(plaintext: &[u8]) -> Value {
    serde_json::to_value(Cheap::try_encrypt_raw(plaintext, PASSWORD).unwrap()).unwrap()
}

fn cheap_account_file(key: &AccountSecretKey) -> Value {
    let mut plaintext = vec![1];
    plaintext.extend(key.to_bytes());
    cheap_file(&plaintext)
}

fn import(file: &Value, password: &str) -> Result<AccountSecretKey, KeyFileError> {
    AccountSecretKey::from_key_file_text(&file.to_string(), password, MAX_MEMORY)
}

#[test]
fn ocaml_and_native_files_give_their_public_keys() {
    for name in ["keyfile-ocaml-4.0.2", "keyfile-native-0.20.1"] {
        let key =
            AccountSecretKey::from_key_file_text(&fixture(name), PASSWORD, MAX_MEMORY).unwrap();
        assert_eq!(
            key.public_key().to_string(),
            fixture(&format!("{name}.pub")).trim(),
            "{name}"
        );
    }
}

#[test]
fn a_wrong_password_is_named() {
    let result = AccountSecretKey::from_key_file_text(
        &fixture("keyfile-native-0.20.1"),
        "not-the-password",
        MAX_MEMORY,
    );
    assert_eq!(result.err(), Some(KeyFileError::WrongPassword));
}

#[test]
fn a_changed_ciphertext_fails_authentication() {
    let key = AccountSecretKey::rand();
    let mut file = cheap_account_file(&key);
    assert_eq!(
        import(&file, PASSWORD).unwrap().public_key(),
        key.public_key()
    );
    // Flip one ciphertext bit and encode it again with a valid checksum:
    // only the Poly1305 tag can catch it.
    let encoded = file["ciphertext"].as_str().unwrap().to_owned();
    let mut raw = bs58::decode(&encoded)
        .with_check(Some(2))
        .into_vec()
        .unwrap();
    raw[5] ^= 1;
    file["ciphertext"] = bs58::encode(&raw[1..])
        .with_check_version(2)
        .into_string()
        .into();
    assert_eq!(
        import(&file, PASSWORD).err(),
        Some(KeyFileError::WrongPassword)
    );
    // A changed character breaks the base58 checksum instead.
    let mut chars: Vec<char> = encoded.chars().collect();
    chars[10] = if chars[10] == 'a' { 'b' } else { 'a' };
    file["ciphertext"] = chars.into_iter().collect::<String>().into();
    assert_eq!(
        import(&file, PASSWORD).err(),
        Some(KeyFileError::NotAKeyFile)
    );
}

#[test]
fn other_primitives_are_unsupported() {
    let file = cheap_account_file(&AccountSecretKey::rand());
    for (field, value) in [
        ("box_primitive", "aes-256-gcm"),
        ("pw_primitive", "argon2id"),
        ("pw_primitive", "scrypt"),
    ] {
        let mut changed = file.clone();
        changed[field] = value.into();
        assert_eq!(
            import(&changed, PASSWORD).err(),
            Some(KeyFileError::Unsupported),
            "{field} {value}"
        );
    }
}

#[test]
fn what_isnt_a_key_file_is_named() {
    let pub_text = fixture("keyfile-ocaml-4.0.2.pub");
    let mut missing = cheap_account_file(&AccountSecretKey::rand());
    missing.as_object_mut().unwrap().remove("nonce");
    for text in [
        "",
        "   ",
        "not json",
        "{}",
        "[]",
        pub_text.as_str(),
        &AccountSecretKey::rand().to_string(),
        &missing.to_string(),
    ] {
        assert_eq!(
            AccountSecretKey::from_key_file_text(text, PASSWORD, MAX_MEMORY).err(),
            Some(KeyFileError::NotAKeyFile)
        );
    }
}

/// The same envelope holds libp2p keys (a different plaintext): it
/// decrypts, and is refused as an account key.
#[test]
fn other_plaintexts_arent_account_keys() {
    for plaintext in [&[1u8; 33][..1], &[1; 32], &[2; 33], &[1; 34], &[1; 65]] {
        assert_eq!(
            import(&cheap_file(plaintext), PASSWORD).err(),
            Some(KeyFileError::NotAnAccountKey),
            "{} bytes",
            plaintext.len()
        );
    }
}

#[test]
fn a_costlier_derivation_than_allowed_is_refused_before_it_runs() {
    let mut file = cheap_account_file(&AccountSecretKey::rand());
    for pwdiff in [
        (MAX_MEMORY + 1024, 6),
        (u32::MAX, 1),
        // Within the memory allowed, past the core's iteration cap.
        (8192, 65),
    ] {
        file["pwdiff"] = serde_json::json!([pwdiff.0, pwdiff.1]);
        assert_eq!(
            import(&file, PASSWORD).err(),
            Some(KeyFileError::TooExpensive),
            "{pwdiff:?}"
        );
    }
}

/// The export is the format OCaml and `mina wallet generate` write, at
/// OCaml's default cost, and reads back to the same key.
#[test]
fn an_exported_file_reads_back() {
    let key = AccountSecretKey::rand();
    let text = key.to_key_file_text(PASSWORD).unwrap();
    let json: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["box_primitive"], "xsalsa20poly1305");
    assert_eq!(json["pw_primitive"], "argon2i");
    assert_eq!(json["pwdiff"], serde_json::json!([134_217_728, 6]));
    assert!(!text.contains(&key.to_string()));
    let back = AccountSecretKey::from_key_file_text(&text, PASSWORD, MAX_MEMORY).unwrap();
    assert_eq!(back.public_key(), key.public_key());
    assert_eq!(
        AccountSecretKey::from_key_file_text(&text, "another password", MAX_MEMORY).err(),
        Some(KeyFileError::WrongPassword)
    );
}

/// The codes the page maps to its own text.
#[test]
fn error_codes_are_stable() {
    for (error, code) in [
        (KeyFileError::WrongPassword, "wrong-password"),
        (KeyFileError::NotAKeyFile, "not-a-key-file"),
        (KeyFileError::Unsupported, "unsupported"),
        (KeyFileError::NotAnAccountKey, "not-an-account-key"),
        (KeyFileError::TooExpensive, "too-expensive"),
    ] {
        assert_eq!(error.to_string(), code);
    }
}
