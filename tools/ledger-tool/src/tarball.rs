//! Where OCaml publishes the RocksDB tarball of a genesis or epoch ledger.

use blake2::{digest::consts::U32, Blake2b, Digest};
use mina_p2p_messages::{binprot::BinProtWrite, v2};

/// The default bucket of `Cache_dir.s3_keys_bucket_prefix`, which
/// `MINA_LEDGER_S3_BUCKET` overrides in OCaml.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/cache_dir/native/cache_dir.ml#L8-L11>
pub const S3_BUCKET: &str = "https://s3-us-west-2.amazonaws.com/snark-keys-ro.o1test.net";

/// The prefix of the genesis ledger's tarball name.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_ledger_helper/genesis_ledger_helper.ml#L580>
pub const GENESIS_LEDGER_PREFIX: &str = "genesis_ledger";

/// The prefix of the staking and next epoch ledgers' tarball names.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_ledger_helper/genesis_ledger_helper.ml#L626>
pub const EPOCH_LEDGER_PREFIX: &str = "epoch_ledger";

/// The name OCaml looks the tarball of a ledger up by, from the ledger's root:
/// `<prefix>_<hex Blake2b-256 of the root in base58 check followed by the
/// bin_prot empty account>.tar.gz`. The empty account is the current one, so
/// the name changes when the account layout or its default permissions do.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_ledger_helper/genesis_ledger_helper.ml#L92-L100>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/account.ml#L671-L672>
pub fn tarball_name(prefix: &str, ledger_hash: &v2::LedgerHash) -> String {
    let empty: v2::MinaBaseAccountBinableArgStableV2 = (&ledger::Account::empty()).into();
    let mut empty_bytes = Vec::new();
    empty
        .binprot_write(&mut empty_bytes)
        .expect("writing to a Vec doesn't fail");
    let mut hasher = Blake2b::<U32>::new();
    hasher.update(ledger_hash.to_string().as_bytes());
    hasher.update(&empty_bytes);
    format!("{prefix}_{}.tar.gz", hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(prefix: &str, hash: &str) -> String {
        tarball_name(prefix, &hash.parse().unwrap())
    }

    /// The tarballs the Mesa devnet config's ledgers are published as (R13):
    /// each one's SHA3-256 is the config's `s3_data_hash`.
    #[test]
    fn mesa_devnet_tarball_names() {
        assert_eq!(
            name(
                GENESIS_LEDGER_PREFIX,
                "jwrCSCRNpNx3ZBh6D4bUAnZtqPacKXXe6veGC9kQfHrxWqmEbJS"
            ),
            "genesis_ledger_e4b922d4b44b62973b5f75a18d2e32305fe81ed76ec4a49b250e1987e169cf00.tar.gz"
        );
        assert_eq!(
            name(
                EPOCH_LEDGER_PREFIX,
                "jwbFn9V1eyDa5AgLkHAwsVffHSjLLhfw8WpKYM6bJiKXb5Uhbti"
            ),
            "epoch_ledger_2051e48942a85d0a7862124aae7aae84899a1c38028bc7201c8f4fe16904f99b.tar.gz"
        );
        assert_eq!(
            name(
                EPOCH_LEDGER_PREFIX,
                "jxUYRdFcuDDMyEtRXkAEo74EFpFSAjcVf6pBX9j56siW7rFZFRq"
            ),
            "epoch_ledger_508eb7483a18650ccf5c9b31b443dc2877459e628279a30de3d490fb18cf5cd6.tar.gz"
        );
    }

    /// The tarballs of the Mesa mainnet config's ledgers, downloaded for the
    /// mainnet genesis on 2026-10-19.
    #[test]
    fn mesa_mainnet_tarball_names() {
        assert_eq!(
            name(
                GENESIS_LEDGER_PREFIX,
                "jwo3EM3zXkK5gpQYZHQP9KJVgH4ZCfv5bMgvyy3Gh2dAWU6AgyN"
            ),
            "genesis_ledger_8f90b767376bb0196dc19cad67793299d5d51691c446e512b246ca5d8259e00d.tar.gz"
        );
        assert_eq!(
            name(
                EPOCH_LEDGER_PREFIX,
                "jwydedrpUSNZr49ZgNsmZjMVX5BEZp5cwGN68Z7hhU9kp3eAtBa"
            ),
            "epoch_ledger_7ceaa1961cbb1630f98cbb17ac8fe04aeeadbe7b72b302d09789537b15474f3d.tar.gz"
        );
        assert_eq!(
            name(
                EPOCH_LEDGER_PREFIX,
                "jwCRaGZjgN6fe1ZQWGvti47ERfzdPMbffeL6BJPNj8sk6SLNX8Z"
            ),
            "epoch_ledger_cd423fcdf17f7aeb5db014e4782756ecd12b9e5ac31c3201f3023e8cc9721d71.tar.gz"
        );
    }
}
