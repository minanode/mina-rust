use std::{env, fs};

use mina_node_account::AccountSecretKey;

#[test]
fn test_account_secret_key_bs58check_decode() {
    let parsed: AccountSecretKey = "EKFWgzXsoMYcP1Hnj7dBhsefxNucZ6wyz676Qg5uMFNzytXAi2Ww"
        .parse()
        .unwrap();
    // Test by comparing the public key
    assert_eq!(
        parsed.public_key().to_string(),
        "B62qjVQLxt9nYMWGn45mkgwYfcz8e8jvjNCBo11VKJb7vxDNwv5QLPS"
    );
}

#[test]
fn test_account_secret_key_display() {
    let parsed: AccountSecretKey = "EKFWgzXsoMYcP1Hnj7dBhsefxNucZ6wyz676Qg5uMFNzytXAi2Ww"
        .parse()
        .unwrap();
    assert_eq!(
        &parsed.to_string(),
        "EKFWgzXsoMYcP1Hnj7dBhsefxNucZ6wyz676Qg5uMFNzytXAi2Ww"
    );
}

#[test]
fn test_encrypt_decrypt() {
    let password = "not-very-secure-pass";

    let new_key = AccountSecretKey::rand();
    let tmp_dir = env::temp_dir();
    let tmp_path = format!("{}/{}-key", tmp_dir.display(), new_key.public_key());

    // dump encrypted file
    new_key
        .to_encrypted_file(&tmp_path, password)
        .expect("Failed to encrypt secret key");

    // load and decrypt
    let decrypted = AccountSecretKey::from_encrypted_file(&tmp_path, password)
        .unwrap_or_else(|_| panic!("Failed to decrypt secret key file: {}", tmp_path));

    assert_eq!(
        new_key.public_key(),
        decrypted.public_key(),
        "Encrypted and decrypted public keys do not match"
    );
}

/// A key file that other users could read is refused, as OCaml's
/// `Secret_file.read` refuses it; the file as our own writer leaves it is
/// still accepted.
#[cfg(unix)]
#[test]
fn test_key_file_readable_by_others_is_refused() {
    use std::os::unix::fs::PermissionsExt;

    let password = "not-very-secure-pass";
    let new_key = AccountSecretKey::rand();
    let tmp_dir = env::temp_dir();
    let tmp_path = format!("{}/{}-perm-key", tmp_dir.display(), new_key.public_key());
    new_key
        .to_encrypted_file(&tmp_path, password)
        .expect("Failed to encrypt secret key");

    // As written: private to its owner.
    AccountSecretKey::from_encrypted_file(&tmp_path, password)
        .unwrap_or_else(|_| panic!("Failed to decrypt private key file: {}", tmp_path));

    for mode in [0o644, 0o600 + 0o004, 0o666] {
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(mode)).unwrap();
        let error = match AccountSecretKey::from_encrypted_file(&tmp_path, password) {
            Err(error) => error,
            Ok(_) => panic!("Accepted a {mode:o} key file: {}", tmp_path),
        };
        assert!(
            error.to_string().contains("permissions"),
            "{mode:o}: {error}"
        );
    }

    fs::remove_file(&tmp_path).unwrap();
}

#[test]
fn test_block_producer_key_decrypt() {
    // Get the workspace root directory
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_root = format!("{}/../../..", manifest_dir);
    let key_path = format!(
        "{}/tests/files/accounts/test-block-producer",
        workspace_root
    );
    let pubkey_path = format!(
        "{}/tests/files/accounts/test-block-producer.pub",
        workspace_root
    );
    let password = "test-password";

    // Git checkouts give the committed fixture group-write permissions;
    // the loader requires a key file to be private to its owner.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    // Load and decrypt the key
    let secret_key =
        AccountSecretKey::from_encrypted_file(&key_path, password).unwrap_or_else(|e| {
            panic!(
                "Failed to decrypt secret key file: {} - Error: {}",
                key_path, e
            )
        });

    // Get the public key from the decrypted secret key
    let public_key_from_secret = secret_key.public_key();

    // Load the expected public key from file
    let expected_public_key = fs::read_to_string(&pubkey_path)
        .unwrap_or_else(|_| panic!("Failed to read public key file: {}", pubkey_path))
        .trim()
        .to_string();

    // Verify they match
    assert_eq!(
        public_key_from_secret.to_string(),
        expected_public_key,
        "Public key from decrypted secret key does not match expected public key"
    );
}
