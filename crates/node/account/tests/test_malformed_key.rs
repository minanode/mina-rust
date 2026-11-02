//! Decrypted account keys of the wrong length or version are errors, not
//! panics (R43e.2).

use mina_node_account::AccountSecretKey;

#[test]
fn malformed_decrypted_account_lengths_and_version_return_errors() {
    use mina_core::EncryptedSecretKey;
    struct TestKey;
    impl EncryptedSecretKey for TestKey {
        const PW_DIFF: (u32, u32) = (8192, 1);
    }
    for plaintext in [vec![], vec![1], vec![1; 32], vec![1; 34], vec![2; 33]] {
        let encrypted = TestKey::try_encrypt_raw(&plaintext, "test").unwrap();
        assert!(AccountSecretKey::from_encrypted(&encrypted, "test").is_err());
    }
}
