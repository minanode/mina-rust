//! Reads an OCaml ledger from a dump of its RocksDB database, as
//! `mina-rocksdb-scanner dump` writes it: one `<key hex> : <value hex>` line
//! per entry, sorted by key.
//!
//! OCaml publishes a network's genesis and epoch ledgers as tarballs of such
//! databases (`genesis_ledger_<hash>.tar.gz`, `epoch_ledger_<hash>.tar.gz`).
//! A database key is a location: a prefix byte, then a path in the tree
//! padded with zero bits to whole bytes, the first step in the most
//! significant bit. With the mainnet and devnet depth of 35, a path is 5
//! bytes, and account `i` is at path `i << 5`.
//!
//! - Accounts are under prefix `0xfe`, as bin_prot accounts.
//! - Hashes are under the prefix `depth - d` for a node at depth `d`, so the
//!   root is under `depth` with an all-zero path, as a bin_prot field.
//! - Generic entries (prefix `0xff`) map account and token ids to locations.
//!   The one this reader uses is `last_account_location`.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/merkle_ledger/location.ml#L5-L23>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/merkle_ledger/location.ml#L75-L85>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/merkle_address/merkle_address.ml#L220-L227>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/merkle_ledger/database.ml#L235-L236>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/rocksdb-scanner/rocksdb_scanner.ml#L28-L62>

use std::io::BufRead;

use anyhow::{bail, ensure, Context};
use mina_p2p_messages::{binprot::BinProtRead, v2};

const ACCOUNT_PREFIX: u8 = 0xfe;
const GENERIC_PREFIX: u8 = 0xff;
const LAST_ACCOUNT_LOCATION: &[u8] = b"last_account_location";

/// The accounts of one ledger, in ledger index order, and the root its
/// database stores.
pub struct LedgerDump {
    pub accounts: Vec<ledger::Account>,
    pub root: v2::LedgerHash,
}

impl LedgerDump {
    /// The total currency stake is measured against: the balances of the
    /// default token's accounts.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/genesis_data.ml#L3-L11>
    pub fn total_currency(&self) -> u64 {
        self.accounts
            .iter()
            .filter(|account| account.token_id.is_default())
            .map(|account| account.balance.as_u64())
            .try_fold(0u64, u64::checked_add)
            .expect("total currency overflows")
    }

    /// How many accounts hold another token than the default one.
    pub fn custom_token_accounts(&self) -> usize {
        self.accounts
            .iter()
            .filter(|account| !account.token_id.is_default())
            .count()
    }
}

/// The index of the account at this location (prefix and path), if it is an
/// account location.
fn account_index(location: &[u8], ledger_depth: usize) -> anyhow::Result<Option<u64>> {
    let path_len = ledger_depth.div_ceil(8);
    match location.split_first() {
        Some((&ACCOUNT_PREFIX, path)) => {
            ensure!(
                path.len() == path_len,
                "account location {} is not {path_len} bytes long",
                hex::encode(path)
            );
            let path = path
                .iter()
                .fold(0u64, |acc, byte| (acc << 8) | *byte as u64);
            let padding = 8 * path_len - ledger_depth;
            ensure!(
                path & ((1 << padding) - 1) == 0,
                "account location {path:#x} has bits in its padding"
            );
            Ok(Some(path >> padding))
        }
        _ => Ok(None),
    }
}

/// Reads a dump of a ledger database of depth `ledger_depth`.
///
/// The accounts must be at indexes 0 to n - 1, with no gaps, and
/// `last_account_location` must be the last of them. The dump is sorted by
/// key, so accounts come in index order. Every other entry, including the
/// inner hashes, is skipped: the caller rebuilds the tree and compares its
/// root with [`LedgerDump::root`].
pub fn read(reader: impl BufRead, ledger_depth: usize) -> anyhow::Result<LedgerDump> {
    ensure!(
        (1..=56).contains(&ledger_depth),
        "unsupported ledger depth {ledger_depth}"
    );
    let path_len = ledger_depth.div_ceil(8);
    let root_key = {
        let mut key = vec![ledger_depth as u8];
        key.resize(1 + path_len, 0);
        key
    };
    let last_account_key = [&[GENERIC_PREFIX], LAST_ACCOUNT_LOCATION].concat();

    let mut accounts = Vec::new();
    let mut root = None;
    let mut last_account = None;
    for (line_number, line) in reader.lines().enumerate() {
        let line_number = line_number + 1;
        let line = line.with_context(|| format!("line {line_number}"))?;
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(" : ") else {
            bail!("line {line_number} is not `<key> : <value>`");
        };
        let key = hex::decode(key).with_context(|| format!("key on line {line_number}"))?;
        let is_account = key.first() == Some(&ACCOUNT_PREFIX);
        if !is_account && key != root_key && key != last_account_key {
            continue;
        }
        let value = hex::decode(value).with_context(|| format!("value on line {line_number}"))?;
        let mut bytes = value.as_slice();

        if let Some(index) = account_index(&key, ledger_depth)? {
            ensure!(
                index == accounts.len() as u64,
                "line {line_number}: account {index} where account {} was expected",
                accounts.len()
            );
            let account = v2::MinaBaseAccountBinableArgStableV2::binprot_read(&mut bytes)
                .with_context(|| format!("account {index}"))?;
            ensure!(bytes.is_empty(), "trailing bytes after account {index}");
            let account: ledger::Account = (&account)
                .try_into()
                .with_context(|| format!("account {index}"))?;
            accounts.push(account);
        } else if key == root_key {
            let hash = v2::LedgerHash::binprot_read(&mut bytes).context("root hash")?;
            ensure!(bytes.is_empty(), "trailing bytes after the root hash");
            root = Some(hash);
        } else {
            last_account = Some(
                account_index(&value, ledger_depth)?
                    .context("last_account_location is not an account location")?,
            );
        }
    }

    let root = root.context("the dump has no root hash")?;
    match last_account {
        Some(last) => ensure!(
            accounts.len() as u64 == last + 1,
            "{} accounts, but the last account location is {last}",
            accounts.len()
        ),
        None => ensure!(
            accounts.is_empty(),
            "{} accounts but no last account location",
            accounts.len()
        ),
    }
    Ok(LedgerDump { accounts, root })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ledger::scan_state::currency::Balance;
    use mina_curves::pasta::Fp;
    use mina_p2p_messages::binprot::BinProtWrite;

    const DEPTH: usize = 35;

    fn account(i: u64, token_id: ledger::TokenId) -> ledger::Account {
        let public_key = mina_signer::CompressedPubKey {
            x: Fp::from(1000 + i),
            is_odd: false,
        };
        let id = ledger::AccountId::new(public_key, token_id);
        ledger::Account::create_with(id, Balance::from_u64(10 * (i + 1)))
    }

    fn hex_of(value: &impl BinProtWrite) -> String {
        let mut bytes = Vec::new();
        value.binprot_write(&mut bytes).unwrap();
        hex::encode_upper(bytes)
    }

    fn account_key(i: u64) -> String {
        format!("FE{:010X}", i << 5)
    }

    fn root_line(root: &v2::LedgerHash) -> String {
        format!("230000000000 : {}", hex_of(root))
    }

    /// A dump as `mina-rocksdb-scanner` writes it, sorted by key, with inner
    /// hashes and generic entries the reader must skip.
    fn dump(accounts: &[ledger::Account], root: &v2::LedgerHash) -> String {
        let mut lines = vec![
            // A leaf hash and an inner hash.
            format!("000000000000 : {}", "11".repeat(32)),
            format!("010000000000 : {}", "22".repeat(32)),
            root_line(root),
        ];
        for (i, account) in accounts.iter().enumerate() {
            let wire: v2::MinaBaseAccountBinableArgStableV2 = account.into();
            lines.push(format!("{} : {}", account_key(i as u64), hex_of(&wire)));
        }
        // An account id's location, then the last account location.
        lines.push(format!("FF2442363271 : {}", account_key(0)));
        lines.push(format!(
            "{} : {}",
            hex::encode_upper([&[0xff], LAST_ACCOUNT_LOCATION].concat()),
            account_key(accounts.len() as u64 - 1)
        ));
        lines.join("\n") + "\n"
    }

    fn accounts() -> Vec<ledger::Account> {
        vec![
            account(0, ledger::TokenId::default()),
            account(1, ledger::TokenId(Fp::from(7u64))),
            account(2, ledger::TokenId::default()),
        ]
    }

    #[test]
    fn reads_accounts_in_order_and_the_root() {
        let accounts = accounts();
        let root = v2::LedgerHash::from_fp(Fp::from(42u64));
        let dump = read(dump(&accounts, &root).as_bytes(), DEPTH).unwrap();
        assert_eq!(dump.accounts, accounts);
        assert_eq!(dump.root, root);
        assert_eq!(dump.total_currency(), 10 + 30);
        assert_eq!(dump.custom_token_accounts(), 1);
    }

    #[test]
    fn account_locations() {
        assert_eq!(
            account_index(&[0xfe, 0, 0, 0, 0, 0], DEPTH).unwrap(),
            Some(0)
        );
        assert_eq!(
            account_index(&[0xfe, 0, 0, 0, 0, 0x20], DEPTH).unwrap(),
            Some(1)
        );
        // The last of 2^35 leaves.
        assert_eq!(
            account_index(&[0xfe, 0xff, 0xff, 0xff, 0xff, 0xe0], DEPTH).unwrap(),
            Some((1 << 35) - 1)
        );
        assert_eq!(account_index(&[0x23, 0, 0, 0, 0, 0], DEPTH).unwrap(), None);
        assert!(account_index(&[0xfe, 0, 0, 0, 0, 1], DEPTH).is_err());
        assert!(account_index(&[0xfe, 0, 0, 0, 0], DEPTH).is_err());
    }

    #[test]
    fn rejects_gaps_and_missing_entries() {
        let accounts = accounts();
        let root = v2::LedgerHash::from_fp(Fp::from(42u64));
        let full = dump(&accounts, &root);

        // Account 1 missing.
        let gap: String = full
            .lines()
            .filter(|line| !line.starts_with(&account_key(1)))
            .map(|line| format!("{line}\n"))
            .collect();
        let error = read(gap.as_bytes(), DEPTH).err().unwrap();
        assert!(
            error.to_string().contains("account 2 where account 1"),
            "{error}"
        );

        // The last account missing: the count disagrees with the last location.
        let short: String = full
            .lines()
            .filter(|line| !line.starts_with(&account_key(2)))
            .map(|line| format!("{line}\n"))
            .collect();
        let error = read(short.as_bytes(), DEPTH).err().unwrap();
        assert!(
            error.to_string().contains("last account location is 2"),
            "{error}"
        );

        // No root.
        let no_root = full.replace(&(root_line(&root) + "\n"), "");
        let error = read(no_root.as_bytes(), DEPTH).err().unwrap();
        assert!(error.to_string().contains("no root hash"), "{error}");

        // Not a dump line.
        let error = read("not a dump\n".as_bytes(), DEPTH).err().unwrap();
        assert!(error.to_string().contains("line 1"), "{error}");
    }
}
