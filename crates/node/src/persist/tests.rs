//! Tests of the frontier store (R20 D8): the ledger codec, the Merkle diff,
//! the manifest and its two slots, crash injection over the in-memory backend,
//! the version, chain and lock rules, garbage collection, the native backend,
//! and (ignored, it needs a mainnet dump) the I5b check of
//! `StagedLedger::from_persisted_unchecked`.
//!
//! Ledgers come from `Account::rand`, which gives zkApp accounts with and
//! without verification keys. The randomized tests draw their shapes (sizes,
//! changed indices, chunk sizes) from seeded generators, so a failure names
//! its seed; the accounts themselves differ from run to run.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read},
};

use ledger::{Account, AccountIndex, Address, BaseLedger, Mask};
use mina_core::{ChainId, DEVNET_CHAIN_ID, MAINNET_CHAIN_ID};
use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    v2,
};
use rand::{rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};

use super::{
    backend::Faults,
    diff::{address_of_linear, apply_diff, merkle_diff_bounded, DiffError},
    format::{
        block_name, decode_manifest, encode_manifest, is_data_file, ledger_diff_name,
        ledger_full_name, parts_name, staged_name, SlotError, ATTEMPTS_FILE, MANIFEST_SLOTS,
    },
    ledger_codec::{
        cached_hash, count_nonempty_nodes, read_diff, read_diff_as_child, read_full, read_header,
        read_ledger, verify_full, write_diff, write_full, CodecError, LEDGER_MAGIC,
    },
    *,
};

const DEPTH: usize = 35;

type Files = BTreeMap<String, Vec<u8>>;
type Op<'a> = &'a dyn Fn(&mut FrontierStore<MemBackend>) -> Result<(), PersistError>;

// Ledgers.

fn random_ledger_with(num_accounts: usize, token_owners: bool) -> Mask {
    let mut ledger = if token_owners {
        Mask::create_with_token_owners(DEPTH)
    } else {
        Mask::create(DEPTH)
    };
    for _ in 0..num_accounts {
        let account = Account::rand();
        ledger.get_or_create_account(account.id(), account).unwrap();
    }
    ledger
}

fn random_ledger(num_accounts: usize) -> Mask {
    random_ledger_with(num_accounts, false)
}

fn root(ledger: &Mask) -> Fp {
    ledger.clone().merkle_root()
}

fn hash_of(ledger: &Mask) -> v2::LedgerHash {
    v2::LedgerHash::from_fp(root(ledger))
}

/// A copy of the root ledger `base`, with the nonces of the accounts at
/// `changed` incremented and `appended` new accounts.
fn modified(base: &Mask, changed: &[u64], appended: usize) -> Mask {
    let mut ledger = base.copy();
    for &index in changed {
        let mut account = ledger.get_at_index(AccountIndex(index)).unwrap();
        account.nonce = account.nonce.incr();
        ledger.set_at_index(AccountIndex(index), account).unwrap();
    }
    for _ in 0..appended {
        let account = Account::rand();
        ledger.get_or_create_account(account.id(), account).unwrap();
    }
    ledger
}

fn linear(level: u32, index: u64) -> u64 {
    (1u64 << level) - 1 + index
}

fn node(level: u32, index: u64) -> Address {
    Address::from_index(AccountIndex(index), level as usize)
}

/// The non-empty nodes of a ledger, as `(level, index)`.
fn nonempty_nodes(num_accounts: u64, depth: u32) -> Vec<(u32, u64)> {
    let mut nodes = Vec::new();
    for level in 0..=depth {
        let count = match num_accounts {
            0 => 0,
            n => ((n - 1) >> (depth - level)) + 1,
        };
        nodes.extend((0..count).map(|index| (level, index)));
    }
    nodes
}

/// The linear indices of the nodes on the paths from `leaves` to the root.
fn nodes_on_paths(leaves: &[u64], depth: u32) -> Vec<u64> {
    let mut nodes = BTreeSet::new();
    for &leaf in leaves {
        for level in 0..=depth {
            nodes.insert(linear(level, leaf >> (depth - level)));
        }
    }
    nodes.into_iter().collect()
}

/// `loaded` has the accounts of `expected`, at the same indices, and holds the
/// hash of every non-empty node, equal to the one `expected` computes.
fn assert_same_ledger(loaded: &Mask, expected: &Mask) {
    let (mut loaded, mut expected) = (loaded.clone(), expected.clone());
    assert_eq!(loaded.depth(), expected.depth());
    assert_eq!(loaded.num_accounts(), expected.num_accounts());
    assert_eq!(loaded.has_token_owners(), expected.has_token_owners());
    let num_accounts = expected.num_accounts() as u64;
    for index in 0..num_accounts {
        let account = expected.get_at_index(AccountIndex(index)).unwrap();
        assert_eq!(
            loaded.get_at_index(AccountIndex(index)).as_ref(),
            Some(&account),
            "account {index}"
        );
        assert_eq!(
            loaded.index_of_account(account.id()),
            Some(AccountIndex(index))
        );
        if expected.has_token_owners() {
            let token = account.id().derive_token_id();
            assert_eq!(
                loaded.token_owner(token.clone()),
                expected.token_owner(token)
            );
        }
    }
    // A child mask's own hashes, or its parent's off its accounts' paths.
    for (level, index) in nonempty_nodes(num_accounts, expected.depth() as u32) {
        let want = expected.get_inner_hash_at_addr(node(level, index)).unwrap();
        assert_eq!(
            cached_hash(&mut loaded, &node(level, index)),
            Some(want),
            "node ({level}, {index})"
        );
    }
    assert_eq!(loaded.merkle_root(), expected.merkle_root());
}

/// A reader that returns at most `chunk` bytes per `read`.
struct Chunked<'a> {
    bytes: &'a [u8],
    chunk: usize,
}

impl Read for Chunked<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(self.chunk).min(self.bytes.len());
        buf[..n].copy_from_slice(&self.bytes[..n]);
        self.bytes = &self.bytes[n..];
        Ok(n)
    }
}

fn full_bytes(ledger: &Mask) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_full(&mut bytes, ledger).unwrap();
    bytes
}

fn diff_bytes(diff: &LedgerDiff) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_diff(&mut bytes, diff).unwrap();
    bytes
}

/// Reads a whole ledger file of either kind through `chunk`-byte reads, and
/// checks that nothing is left after it.
fn read_bytes(
    bytes: &[u8],
    chunk: usize,
    base: Option<&Mask>,
    verify: VerifyMode,
) -> Result<(Mask, LedgerFileHeader), CodecError> {
    let mut reader = Chunked { bytes, chunk };
    let result = read_ledger(&mut reader, |_| base.cloned(), verify)?;
    assert!(reader.bytes.is_empty(), "bytes left after the ledger");
    Ok(result)
}

/// A full ledger file, re-encoded with its accounts and hashes changed by
/// `edit`: the header is re-encoded from the new counts.
fn edit_full_file(
    bytes: &[u8],
    edit: impl FnOnce(
        &mut LedgerFileHeader,
        &mut Vec<v2::MinaBaseAccountBinableArgStableV2>,
        &mut Vec<(u64, v2::LedgerHash)>,
    ),
) -> Vec<u8> {
    let mut reader = bytes;
    let mut header = read_header(&mut reader).unwrap();
    let mut accounts = (0..header.num_entries)
        .map(|_| v2::MinaBaseAccountBinableArgStableV2::binprot_read(&mut reader).unwrap())
        .collect::<Vec<_>>();
    let mut hashes = (0..header.num_hashes)
        .map(|_| {
            (
                u64::binprot_read(&mut reader).unwrap(),
                v2::LedgerHash::binprot_read(&mut reader).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert!(reader.is_empty());
    edit(&mut header, &mut accounts, &mut hashes);
    let mut out = Vec::new();
    out.extend_from_slice(&LEDGER_MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    header.binprot_write(&mut out).unwrap();
    for account in &accounts {
        account.binprot_write(&mut out).unwrap();
    }
    for (index, hash) in &hashes {
        index.binprot_write(&mut out).unwrap();
        hash.binprot_write(&mut out).unwrap();
    }
    out
}

// Stores.

fn genesis() -> v2::StateHash {
    v2::StateHash::from_fp(Fp::from(7u64))
}

fn state_hash(n: u64) -> v2::StateHash {
    v2::StateHash::from_fp(Fp::from(1_000 + n))
}

fn config_for(chain_id: ChainId, genesis_state_hash: v2::StateHash) -> StoreConfig {
    StoreConfig {
        chain_id,
        genesis_state_hash,
        node_commit: "i5a-tests".to_owned(),
        now_ms: Box::new(|| 1_760_000_000_000),
    }
}

fn config() -> StoreConfig {
    config_for(MAINNET_CHAIN_ID, genesis())
}

fn entry(hash: v2::StateHash, height: u64) -> ChainEntry {
    ChainEntry {
        hash,
        height: height as u32,
        global_slot: height as u32 * 3,
        just_emitted_a_proof: height % 4 == 0,
    }
}

/// The chain of blocks `from` to `to`, both included.
fn chain(from: u64, to: u64) -> Vec<ChainEntry> {
    (from..=to).map(|n| entry(state_hash(n), n)).collect()
}

fn block_bytes(hash: &v2::StateHash) -> Vec<u8> {
    format!("block {hash};").repeat(8).into_bytes()
}

fn blocks(chain: &[ChainEntry]) -> Vec<(v2::StateHash, Blob)> {
    chain
        .iter()
        .map(|entry| (entry.hash.clone(), blob(block_bytes(&entry.hash))))
        .collect()
}

fn staged_ledger_hash(staged: &Mask) -> v2::MinaBaseStagedLedgerHashStableV1 {
    v2::MinaBaseStagedLedgerHashStableV1::zero(
        hash_of(staged),
        v2::PendingCoinbaseHash::from_fp(Fp::from(3u64)),
    )
}

/// The ledgers of a checkpoint: the staking ledger, the root snarked ledger
/// over it and the root staged ledger over that.
#[derive(Clone)]
struct Fixture {
    staking: Mask,
    snarked: Mask,
    staged: Mask,
}

impl Fixture {
    /// At least 5 accounts, for the changes below.
    fn new(num_accounts: usize) -> Self {
        assert!(num_accounts >= 5);
        let staking = random_ledger(num_accounts);
        let snarked = modified(&staking, &[1, 4], 2);
        let staged = modified(&snarked, &[0, 3], 1);
        Self {
            staking,
            snarked,
            staged,
        }
    }

    /// The next root: the snarked ledger moves on, the staking ledger stays.
    fn next(&self) -> Self {
        let snarked = modified(&self.snarked, &[2, 5], 1);
        let staged = modified(&snarked, &[6], 0);
        Self {
            staking: self.staking.clone(),
            snarked,
            staged,
        }
    }

    /// A checkpoint at root `root`, with the chain `root` to `tip`.
    fn write(&self, root: u64, tip: u64) -> CheckpointWrite {
        let chain = chain(root, tip);
        CheckpointWrite {
            checkpoint: Checkpoint {
                root: state_hash(root),
                root_height: root as u32,
                root_snarked_ledger: hash_of(&self.snarked),
                root_staged_ledger_hash: staged_ledger_hash(&self.staged),
            },
            ledgers: vec![
                (
                    hash_of(&self.staking),
                    Some(LedgerWrite::Full(self.staking.clone())),
                ),
                (
                    hash_of(&self.snarked),
                    Some(choose_ledger_write(
                        &self.snarked,
                        std::slice::from_ref(&self.staking),
                    )),
                ),
            ],
            staged: merkle_diff(&self.staged, &self.snarked).unwrap(),
            parts: blob(format!("parts {root};").repeat(16).into_bytes()),
            blocks: blocks(&chain),
            chain,
        }
    }
}

fn open(mem: &MemBackend) -> (FrontierStore<MemBackend>, OpenReport) {
    FrontierStore::open(mem.second_handle(), config()).unwrap()
}

/// What a load reads, checked: every listed ledger with its bases, the
/// checkpoint's staged ledger and parts, and every block of the chain.
struct Loaded {
    ledgers: BTreeMap<v2::LedgerHash, Mask>,
    staged: Option<Mask>,
    blocks: usize,
}

fn load_all<B: StoreBackend>(
    store: &mut FrontierStore<B>,
    verify: VerifyMode,
) -> Result<Loaded, PersistError> {
    let Some(manifest) = store.manifest().cloned() else {
        return Ok(Loaded {
            ledgers: BTreeMap::new(),
            staged: None,
            blocks: 0,
        });
    };
    let ledgers = store.read_ledgers(&manifest, verify)?;
    let missing = |name: String| PersistError::MissingFile(name);
    let staged = match &manifest.checkpoint {
        Some(checkpoint) => {
            let snarked = ledgers
                .get(&checkpoint.root_snarked_ledger)
                .ok_or_else(|| missing(ledger_full_name(&checkpoint.root_snarked_ledger)))?;
            let staged = store.read_staged(&manifest, snarked, verify)?;
            let parts = manifest
                .file(&parts_name(&checkpoint.root))
                .ok_or_else(|| missing(parts_name(&checkpoint.root)))?;
            store.read_blob(parts)?;
            Some(staged)
        }
        None => None,
    };
    for entry in &manifest.chain {
        let name = block_name(&entry.hash);
        let file = manifest.file(&name).ok_or_else(|| missing(name.clone()))?;
        if store.read_blob(file)? != block_bytes(&entry.hash) {
            return Err(PersistError::corrupt(name, "another block"));
        }
    }
    Ok(Loaded {
        ledgers,
        staged,
        blocks: manifest.chain.len(),
    })
}

fn assert_fixture_loads(loaded: &Loaded, fixture: &Fixture) {
    assert_same_ledger(
        &loaded.ledgers[&hash_of(&fixture.staking)],
        &fixture.staking,
    );
    assert_same_ledger(
        &loaded.ledgers[&hash_of(&fixture.snarked)],
        &fixture.snarked,
    );
    assert_same_ledger(loaded.staged.as_ref().unwrap(), &fixture.staged);
}

fn heights(chain: &[ChainEntry]) -> Vec<u32> {
    chain.iter().map(|entry| entry.height).collect()
}

fn flip(mem: &MemBackend, name: &str, at: usize) {
    let mut bytes = mem.file(name).unwrap();
    bytes[at] ^= 0x10;
    mem.put_file(name, bytes);
}

fn data_files(mem: &MemBackend) -> BTreeSet<String> {
    mem.files()
        .into_keys()
        .filter(|name| is_data_file(name))
        .collect()
}

// The ledger codec.

#[test]
fn full_files_round_trip_through_any_chunk_size() {
    for (num_accounts, token_owners) in [(0, false), (1, false), (37, false), (64, true)] {
        let ledger = random_ledger_with(num_accounts, token_owners);
        if num_accounts >= 37 {
            let accounts = ledger.to_list();
            let zkapps = accounts.iter().filter(|a| a.zkapp.is_some()).count();
            let with_keys = accounts
                .iter()
                .filter(|a| {
                    a.zkapp
                        .as_ref()
                        .is_some_and(|z| z.verification_key.is_some())
                })
                .count();
            assert!(zkapps > 0 && with_keys > 0, "no zkApp accounts to test");
        }
        let bytes = full_bytes(&ledger);
        for chunk in [1, 7, 4 << 20] {
            for verify in [
                VerifyMode::RootOnly,
                VerifyMode::default(),
                VerifyMode::Full,
            ] {
                let (loaded, header) = read_bytes(&bytes, chunk, None, verify).unwrap();
                assert_eq!(header.kind, LedgerFileKind::Full);
                assert_eq!(header.ledger_hash, hash_of(&ledger));
                assert_eq!(header.depth, DEPTH as u32);
                assert_eq!(header.num_accounts, num_accounts as u64);
                assert_eq!(header.num_entries, num_accounts as u64);
                assert_eq!(
                    header.num_hashes,
                    count_nonempty_nodes(num_accounts as u64, DEPTH as u32)
                );
                assert_eq!(header.token_owners, token_owners);
                assert!(loaded.is_root());
                assert_same_ledger(&loaded, &ledger);
            }
        }
    }
}

#[test]
fn diff_files_round_trip_and_leave_the_base_alone() {
    let base = random_ledger(40);
    let base_root = root(&base);
    let first = base.clone().get_at_index(AccountIndex(0)).unwrap();
    let target = modified(&base, &[0, 7, 39], 3);
    let diff = merkle_diff(&target, &base).unwrap();
    let bytes = diff_bytes(&diff);
    for chunk in [1, 7, 4 << 20] {
        let (loaded, header) =
            read_bytes(&bytes, chunk, Some(&base), VerifyMode::default()).unwrap();
        assert_eq!(
            header.kind,
            LedgerFileKind::Diff {
                base: hash_of(&base)
            }
        );
        assert_eq!(header.num_accounts, 43);
        assert_eq!(header.num_entries, 6);
        assert!(loaded.is_root());
        assert_same_ledger(&loaded, &target);
    }
    assert_eq!(root(&base), base_root);
    assert_eq!(base.num_accounts(), 40);
    assert_eq!(
        base.clone().get_at_index(AccountIndex(0)).as_ref(),
        Some(&first)
    );
    // A diff over a diff.
    let loaded = read_bytes(&bytes, 4 << 20, Some(&base), VerifyMode::Full)
        .unwrap()
        .0;
    let next = modified(&loaded, &[42], 1);
    let again = diff_bytes(&merkle_diff(&next, &loaded).unwrap());
    let (reloaded, _) = read_bytes(&again, 7, Some(&loaded), VerifyMode::Full).unwrap();
    assert_same_ledger(&reloaded, &next);
}

#[test]
fn a_diff_needs_its_own_base() {
    let base = random_ledger(12);
    let target = modified(&base, &[3], 0);
    let bytes = diff_bytes(&merkle_diff(&target, &base).unwrap());
    let base_hash = hash_of(&base);
    let mut reader = bytes.as_slice();
    let (loaded, _) = read_ledger(
        &mut reader,
        |hash| (hash == &base_hash).then(|| base.clone()),
        VerifyMode::default(),
    )
    .unwrap();
    assert_same_ledger(&loaded, &target);

    let mut reader = bytes.as_slice();
    let missing = read_ledger(&mut reader, |_| None, VerifyMode::default());
    assert!(matches!(missing, Err(CodecError::WrongBase)));

    let other = random_ledger(12);
    let wrong = read_bytes(&bytes, 1 << 20, Some(&other), VerifyMode::default());
    assert!(matches!(wrong, Err(CodecError::WrongBase)));

    // A child mask is not a base: the diff is applied to a copy of a root.
    let mut reader = bytes.as_slice();
    let header = read_header(&mut reader).unwrap();
    let child = base.make_child();
    assert!(matches!(
        read_diff(&mut reader, &header, &child, VerifyMode::default()),
        Err(CodecError::Invalid(_))
    ));
}

/// The root staged ledger's diff read as a child mask of its base (I5d5): the
/// mask holds only the diff's accounts and the hashes on their paths, reads
/// everything else from the base, and leaves the base as it was. Its
/// verification reads the hashes off the changed paths from the base, so a
/// changed account under its old hashes, or a hash replaced on a changed
/// path, is still caught; a hash off the changed paths is refused, as for a
/// copy.
#[test]
fn a_diff_read_as_a_child_mask_holds_only_the_diff() {
    let read = |bytes: &[u8], base: &Mask, verify| {
        let mut reader = Chunked { bytes, chunk: 7 };
        let header = read_header(&mut reader)?;
        let ledger = read_diff_as_child(&mut reader, &header, base, verify)?;
        assert!(reader.bytes.is_empty(), "bytes left after the ledger");
        Ok::<_, CodecError>(ledger)
    };
    for token_owners in [false, true] {
        let base = random_ledger_with(40, token_owners);
        let base_root = root(&base);
        let target = modified(&base, &[0, 7, 39], 3);
        let diff = merkle_diff(&target, &base).unwrap();
        let bytes = diff_bytes(&diff);
        for verify in [
            VerifyMode::RootOnly,
            VerifyMode::default(),
            VerifyMode::Full,
        ] {
            let mut loaded = read(&bytes, &base, verify).unwrap();
            assert!(loaded.is_attached());
            assert_eq!(loaded.get_parent().unwrap().get_uuid(), base.get_uuid());
            assert_same_ledger(&loaded, &target);
            verify_full(&mut loaded).unwrap();
            // Only the diff's own accounts, hashes on their paths only.
            assert_eq!(loaded.validate_child_account_indexes(), Ok(6));
            assert_eq!(loaded.get_raw_inner_hashes().len(), diff.hashes.len());
            assert_eq!(root(&base), base_root);
            assert_eq!(base.num_accounts(), 40);
        }
    }

    let base = random_ledger(40);
    let target = modified(&base, &[0, 7, 39], 3);
    let diff = merkle_diff(&target, &base).unwrap();
    // A changed account under its old hashes.
    let mut tampered = diff.clone();
    tampered.accounts[1].1.nonce = tampered.accounts[1].1.nonce.incr();
    let bytes = diff_bytes(&tampered);
    assert!(read(&bytes, &base, VerifyMode::RootOnly).is_ok());
    assert!(matches!(
        read(&bytes, &base, VerifyMode::Full),
        Err(CodecError::HashMismatch(_))
    ));
    // A changed path's hash replaced, the root's kept.
    let mut tampered = diff.clone();
    let last = tampered.hashes.len() - 1;
    tampered.hashes[last].1 += Fp::from(1u64);
    assert!(matches!(
        read(&diff_bytes(&tampered), &base, VerifyMode::Full),
        Err(CodecError::HashMismatch(_))
    ));
    // A hash off the changed paths, in place of one on them.
    let mut tampered = diff.clone();
    let (on_path, hash) = tampered.hashes.pop().unwrap();
    let off_path = (on_path + 1..)
        .find(|linear| diff.hashes.iter().all(|(index, _)| index != linear))
        .unwrap();
    tampered.hashes.push((off_path, hash));
    let error = read(&diff_bytes(&tampered), &base, VerifyMode::Full).unwrap_err();
    assert!(
        matches!(&error, CodecError::Invalid(reason) if reason.contains("not on a changed path")),
        "{error:?}"
    );
    // A staged ledger the same as its snarked ledger: an empty diff, the
    // root read from the base.
    let same = merkle_diff(&base, &base).unwrap();
    assert!(same.accounts.is_empty());
    let loaded = read(&diff_bytes(&same), &base, VerifyMode::Full).unwrap();
    assert_same_ledger(&loaded, &base);
    assert_eq!(loaded.validate_child_account_indexes(), Ok(0));
    // A child mask is not a base.
    let child = base.make_child();
    assert!(matches!(
        read(&diff_bytes(&diff), &child, VerifyMode::default()),
        Err(CodecError::Invalid(_))
    ));
}

#[test]
fn bad_ledger_files_are_errors() {
    let ledger = random_ledger(10);
    let bytes = full_bytes(&ledger);
    let read = |bytes: &[u8], verify| read_bytes(bytes, 1 << 20, None, verify);

    let mut bad = bytes.clone();
    bad[0] ^= 1;
    assert!(matches!(
        read(&bad, VerifyMode::default()),
        Err(CodecError::BadMagic)
    ));

    let mut bad = bytes.clone();
    bad[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    assert!(matches!(
        read(&bad, VerifyMode::default()),
        Err(CodecError::Version(v)) if v == FORMAT_VERSION + 1
    ));

    // Truncated anywhere.
    for len in 0..bytes.len() {
        assert!(
            read(&bytes[..len], VerifyMode::RootOnly).is_err(),
            "truncated to {len}"
        );
    }

    // Stored under another hash.
    let bad = edit_full_file(&bytes, |header, _, _| {
        header.ledger_hash = v2::LedgerHash::from_fp(Fp::from(5u64))
    });
    assert!(matches!(
        read(&bad, VerifyMode::RootOnly),
        Err(CodecError::RootMismatch)
    ));

    // A wrong leaf hash: only hashing finds it.
    let leaf = linear(DEPTH as u32, 0);
    let bad = edit_full_file(&bytes, |_, _, hashes| {
        let entry = hashes.iter_mut().find(|(index, _)| *index == leaf).unwrap();
        entry.1 = v2::LedgerHash::from_fp(Fp::from(5u64));
    });
    assert!(read(&bad, VerifyMode::RootOnly).is_ok());
    assert!(matches!(
        read(&bad, VerifyMode::Full),
        Err(CodecError::HashMismatch(index)) if index == leaf
    ));
    // 64 random draws over 10 leaves pass by leaf 0, or by its parent.
    assert!(matches!(
        read(&bad, VerifyMode::Sample(64)),
        Err(CodecError::HashMismatch(_))
    ));

    // A wrong account: its path no longer hashes to the stored hashes.
    let bad = edit_full_file(&bytes, |_, accounts, _| {
        let mut account = Account::try_from(&accounts[0]).unwrap();
        account.nonce = account.nonce.incr();
        accounts[0] = (&account).into();
    });
    assert!(matches!(
        read(&bad, VerifyMode::Full),
        Err(CodecError::HashMismatch(index)) if index == leaf
    ));

    // Hashes missing, out of order or past the ledger.
    let bad = edit_full_file(&bytes, |header, _, hashes| {
        hashes.pop();
        header.num_hashes -= 1;
    });
    assert!(matches!(
        read(&bad, VerifyMode::RootOnly),
        Err(CodecError::Invalid(_))
    ));
    let bad = edit_full_file(&bytes, |_, _, hashes| hashes.swap(3, 4));
    assert!(matches!(
        read(&bad, VerifyMode::RootOnly),
        Err(CodecError::Invalid(_))
    ));
    let bad = edit_full_file(&bytes, |_, _, hashes| {
        hashes.last_mut().unwrap().0 += 1;
    });
    assert!(matches!(
        read(&bad, VerifyMode::RootOnly),
        Err(CodecError::Invalid(_))
    ));

    // An account twice, and counts that don't agree.
    let bad = edit_full_file(&bytes, |_, accounts, _| accounts[1] = accounts[0].clone());
    assert!(matches!(
        read(&bad, VerifyMode::RootOnly),
        Err(CodecError::Invalid(_))
    ));
    let bad = edit_full_file(&bytes, |header, accounts, _| {
        accounts.pop();
        header.num_entries -= 1;
    });
    assert!(matches!(
        read(&bad, VerifyMode::RootOnly),
        Err(CodecError::Invalid(_))
    ));
    for depth in [0, 41] {
        let bad = edit_full_file(&bytes, |header, _, _| header.depth = depth);
        assert!(matches!(
            read(&bad, VerifyMode::RootOnly),
            Err(CodecError::Invalid(_))
        ));
    }

    // A full file is not a diff, nor a diff a full file.
    let mut reader = bytes.as_slice();
    let header = read_header(&mut reader).unwrap();
    assert!(matches!(
        read_diff(&mut reader, &header, &ledger, VerifyMode::default()),
        Err(CodecError::Invalid(_))
    ));
    let diff = diff_bytes(&merkle_diff(&modified(&ledger, &[1], 0), &ledger).unwrap());
    let mut reader = diff.as_slice();
    let header = read_header(&mut reader).unwrap();
    assert!(matches!(
        read_full(&mut reader, &header, VerifyMode::default()),
        Err(CodecError::Invalid(_))
    ));
}

thread_local! {
    static CATCHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PANIC_AT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f`, and returns the file a panic in it came from instead of
/// printing it. Panics on other threads are reported as usual.
fn catching_panics<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if CATCHING.with(|catching| catching.get()) {
                let file = info.location().map(|location| location.file().to_owned());
                PANIC_AT.with(|at| *at.borrow_mut() = file);
            } else {
                previous(info);
            }
        }));
    });
    CATCHING.with(|catching| catching.set(true));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    CATCHING.with(|catching| catching.set(false));
    result.map_err(|_| {
        PANIC_AT
            .with(|at| at.borrow_mut().take())
            .unwrap_or_default()
    })
}

/// Where the ledger crate's account conversion panics on a damaged account
/// on this branch: `Affine::new` asserting that a verification key point is
/// on the curve, and ark-ff's `From<BigInt>` unwrapping a value past the
/// modulus. P6a (3e878968) makes both conversion errors; the store never
/// gets here with a damaged file, since it checks a ledger file's checksum
/// before decoding it.
fn is_account_conversion_panic(file: &str) -> bool {
    (file.contains("ark-ec-") && file.ends_with("short_weierstrass/affine.rs"))
        || (file.contains("ark-ff-") && file.ends_with("fields/models/fp/mod.rs"))
}

/// Without the store's checksum, a flipped byte gives an error or a ledger,
/// and the codec itself never panics.
#[test]
fn no_flipped_byte_panics_the_codec() {
    let ledger = random_ledger(4);
    let full = full_bytes(&ledger);
    let base = random_ledger(6);
    let diff = diff_bytes(&merkle_diff(&modified(&base, &[2], 1), &base).unwrap());
    let (mut errors, mut conversion_panics) = (0, 0);
    for (bytes, base) in [(&full, None), (&diff, Some(&base))] {
        for at in 0..bytes.len() {
            let mut bad = bytes.clone();
            bad[at] ^= 0x10;
            let read = catching_panics(|| {
                let mut reader = bad.as_slice();
                read_ledger(&mut reader, |_| base.cloned(), VerifyMode::Sample(2)).is_err()
            });
            match read {
                Ok(error) => errors += error as usize,
                Err(file) if is_account_conversion_panic(&file) => conversion_panics += 1,
                Err(file) => panic!("byte {at} flipped: a panic at {file}"),
            }
        }
    }
    eprintln!(
        "{} flipped bytes: {errors} errors, {conversion_panics} panics in the account conversion",
        full.len() + diff.len()
    );
}

// The Merkle diff.

#[test]
fn identical_ledgers_have_an_empty_diff() {
    let ledger = random_ledger(20);
    let diff = merkle_diff(&ledger, &ledger.copy()).unwrap();
    assert!(diff.accounts.is_empty() && diff.hashes.is_empty());
    assert_eq!(diff.base, diff.target);
    let applied = apply_diff(&ledger, &diff).unwrap();
    assert_same_ledger(&applied, &ledger);
}

#[test]
fn the_diff_holds_exactly_the_changed_accounts_and_paths() {
    let base = random_ledger(40);
    let mut target = modified(&base, &[0, 7, 39], 3);
    let diff = merkle_diff(&target, &base).unwrap();
    let leaves = [0, 7, 39, 40, 41, 42];
    assert_eq!(
        diff.accounts.iter().map(|(i, _)| i.0).collect::<Vec<_>>(),
        leaves
    );
    for (index, account) in &diff.accounts {
        assert_eq!(target.get_at_index(*index).as_ref(), Some(account));
    }
    assert_eq!(
        diff.hashes.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        nodes_on_paths(&leaves, DEPTH as u32)
    );
    for (index, hash) in &diff.hashes {
        let addr = address_of_linear(*index, DEPTH as u8).unwrap();
        assert_eq!(target.get_inner_hash_at_addr(addr).unwrap(), *hash);
    }
    assert_eq!(diff.num_accounts, 43);
    assert_eq!(diff.base, root(&base));
    assert_eq!(diff.target, root(&target));
}

#[test]
fn the_diff_rejects_ledgers_it_cannot_describe() {
    let base = random_ledger(10);
    let mut shorter = Mask::create(DEPTH);
    for account in base.to_list().into_iter().take(8) {
        shorter
            .get_or_create_account(account.id(), account)
            .unwrap();
    }
    assert_eq!(
        merkle_diff(&shorter, &base).unwrap_err(),
        DiffError::Removed(8)
    );
    assert!(matches!(
        choose_ledger_write(&shorter, std::slice::from_ref(&base)),
        LedgerWrite::Full(_)
    ));

    let other_depth = Mask::create(20);
    assert_eq!(
        merkle_diff(&other_depth, &base).unwrap_err(),
        DiffError::Depth(20, DEPTH as u8)
    );
    let owners = Mask::create_with_token_owners(DEPTH);
    assert_eq!(
        merkle_diff(&owners, &base).unwrap_err(),
        DiffError::TokenOwners
    );
}

#[test]
fn the_writer_takes_the_smallest_diff_under_the_limit() {
    let base = random_ledger(10);
    // 40% of 10 accounts: four changes are a diff, five a full file.
    let four = modified(&base, &[0, 1, 2, 3], 0);
    assert!(matches!(
        choose_ledger_write(&four, std::slice::from_ref(&base)),
        LedgerWrite::Diff(diff) if diff.accounts.len() == 4
    ));
    let five = modified(&base, &[0, 1, 2, 3, 4], 0);
    assert!(matches!(
        choose_ledger_write(&five, std::slice::from_ref(&base)),
        LedgerWrite::Full(_)
    ));
    assert!(merkle_diff_bounded(&five, &base, 4).unwrap().is_none());
    assert!(merkle_diff_bounded(&five, &base, 5).unwrap().is_some());

    // Of two bases, the closer one.
    let near = modified(&base, &[0, 1, 2], 0);
    let target = modified(&near, &[5], 0);
    let LedgerWrite::Diff(diff) = choose_ledger_write(&target, &[base.clone(), near.clone()])
    else {
        panic!("a full file");
    };
    assert_eq!(diff.base, root(&near));
    assert_eq!(diff.accounts.len(), 1);
}

#[test]
fn bad_diffs_are_errors() {
    let base = random_ledger(12);
    let target = modified(&base, &[2, 9], 1);
    let diff = merkle_diff(&target, &base).unwrap();
    let is_invalid =
        |diff: &LedgerDiff| matches!(apply_diff(&base, diff), Err(CodecError::Invalid(_)));

    let mut bad = diff.clone();
    bad.hashes.remove(5);
    assert!(is_invalid(&bad), "a hash missing");

    // A hash of an unchanged node: leaf 3's.
    let mut bad = diff.clone();
    let sibling = linear(DEPTH as u32, 3);
    let hash = root(&base); // any value
    let at = bad.hashes.partition_point(|(index, _)| *index < sibling);
    bad.hashes.insert(at, (sibling, hash));
    assert!(is_invalid(&bad), "a hash off the changed paths");

    let mut bad = diff.clone();
    bad.accounts.swap(0, 1);
    assert!(is_invalid(&bad), "accounts out of order");

    let mut bad = diff.clone();
    bad.hashes.swap(0, 1);
    assert!(is_invalid(&bad), "hashes out of order");

    let mut bad = diff.clone();
    bad.base = Fp::from(5u64);
    assert!(matches!(
        apply_diff(&base, &bad),
        Err(CodecError::WrongBase)
    ));

    let mut bad = diff.clone();
    bad.target = Fp::from(5u64);
    assert!(matches!(
        apply_diff(&base, &bad),
        Err(CodecError::RootMismatch)
    ));

    // A wrong leaf hash with the right root: the changed paths are hashed on
    // load, so a sampled read finds it.
    let mut bad = diff.clone();
    let leaf = linear(DEPTH as u32, 2);
    bad.hashes
        .iter_mut()
        .find(|(index, _)| *index == leaf)
        .unwrap()
        .1 = Fp::from(5u64);
    let bytes = diff_bytes(&bad);
    assert!(read_bytes(&bytes, 1 << 20, Some(&base), VerifyMode::RootOnly).is_ok());
    assert!(matches!(
        read_bytes(&bytes, 1 << 20, Some(&base), VerifyMode::default()),
        Err(CodecError::HashMismatch(index)) if index == leaf
    ));
}

/// Base plus diff gives the target, for seeded random shapes: sizes, changed
/// accounts, appended accounts, chains of diffs and read chunk sizes.
#[test]
fn seeded_random_diffs_rebuild_the_target() {
    for seed in 0..24u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let num_accounts = rng.gen_range(1..48usize);
        let mut base = random_ledger(num_accounts);
        for step in 0..3 {
            let len = base.num_accounts() as u64;
            let mut indices = (0..len).collect::<Vec<_>>();
            indices.shuffle(&mut rng);
            let changed = rng.gen_range(0..=indices.len().min(12));
            let mut changed = indices[..changed].to_vec();
            changed.sort_unstable();
            let appended = rng.gen_range(0..4);
            let target = modified(&base, &changed, appended);

            let diff = merkle_diff(&target, &base).unwrap();
            let mut leaves = changed.clone();
            leaves.extend(len..len + appended as u64);
            assert_eq!(
                diff.accounts.iter().map(|(i, _)| i.0).collect::<Vec<_>>(),
                leaves,
                "seed {seed} step {step}"
            );
            assert_same_ledger(&apply_diff(&base, &diff).unwrap(), &target);

            let chunk = [1, 3, 64, 4 << 20][rng.gen_range(0..4)];
            let (loaded, _) = read_bytes(
                &diff_bytes(&diff),
                chunk,
                Some(&base),
                VerifyMode::default(),
            )
            .unwrap();
            assert_same_ledger(&loaded, &target);
            base = loaded;
        }
    }
}

// The manifest and the attempts record.

fn sample_manifest() -> Manifest {
    Manifest {
        generation: 7,
        chain_id: MAINNET_CHAIN_ID,
        genesis_state_hash: genesis(),
        written_at_ms: 1_760_000_000_000,
        node_commit: mina_p2p_messages::binprot::SmallString1k("abc".to_owned()),
        ledgers: vec![v2::LedgerHash::from_fp(Fp::from(1u64))],
        checkpoint: Some(Checkpoint {
            root: state_hash(1),
            root_height: 1,
            root_snarked_ledger: v2::LedgerHash::from_fp(Fp::from(1u64)),
            root_staged_ledger_hash: v2::MinaBaseStagedLedgerHashStableV1::zero(
                v2::LedgerHash::from_fp(Fp::from(2u64)),
                v2::PendingCoinbaseHash::from_fp(Fp::from(3u64)),
            ),
        }),
        chain: chain(1, 3),
        files: vec![FileRef::new("block-x.bin", 10, 0xdead_beef)],
    }
}

#[test]
fn manifests_round_trip_and_any_damage_is_detected() {
    let manifest = sample_manifest();
    let bytes = encode_manifest(&manifest);
    assert_eq!(decode_manifest(&bytes).unwrap(), manifest);
    for len in 0..bytes.len() {
        assert!(
            decode_manifest(&bytes[..len]).is_err(),
            "truncated to {len}"
        );
    }
    for at in 0..bytes.len() {
        let mut bad = bytes.clone();
        bad[at] ^= 0x10;
        assert!(decode_manifest(&bad).is_err(), "byte {at} flipped");
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(matches!(
        decode_manifest(&longer),
        Err(SlotError::Decode(_))
    ));
    let mut newer = bytes.clone();
    newer[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    assert_eq!(
        decode_manifest(&newer).unwrap_err(),
        SlotError::NewerVersion(FORMAT_VERSION + 1)
    );
    let mut older = bytes;
    older[8..12].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(
        decode_manifest(&older).unwrap_err(),
        SlotError::OlderVersion(0)
    );
}

#[test]
fn attempts_records_round_trip_and_damage_reads_as_none() {
    let attempts = Attempts {
        generation: 12,
        count: 2,
    };
    let bytes = attempts.encode();
    assert_eq!(Attempts::decode(&bytes), Some(attempts));
    for at in 0..bytes.len() {
        let mut bad = bytes.clone();
        bad[at] ^= 0x10;
        assert_eq!(Attempts::decode(&bad), None, "byte {at} flipped");
    }
    assert_eq!(Attempts::decode(&bytes[..27]), None);

    let mem = MemBackend::new();
    let (mut store, _) = open(&mem);
    assert_eq!(store.attempts().unwrap(), None);
    store.set_attempts(attempts).unwrap();
    assert_eq!(store.attempts().unwrap(), Some(attempts));
    flip(&mem, ATTEMPTS_FILE, 20);
    assert_eq!(store.attempts().unwrap(), None);
    store.set_attempts(attempts).unwrap();
    store.clear_attempts().unwrap();
    assert_eq!(store.attempts().unwrap(), None);
    store.clear_attempts().unwrap();
}

// The store.

#[test]
fn an_empty_store_opens_empty() {
    let mem = MemBackend::new();
    let (store, report) = open(&mem);
    assert_eq!(report.outcome, OpenOutcome::Empty);
    assert_eq!(report.slots, [SlotStatus::Absent, SlotStatus::Absent]);
    assert!(report.deleted.is_empty());
    assert_eq!(store.generation(), 0);
    assert!(store.manifest().is_none());
}

#[test]
fn a_checkpoint_loads_back_after_a_restart() {
    let mem = MemBackend::new();
    let fixture = Fixture::new(20);
    let (mut store, _) = open(&mem);
    assert_eq!(store.checkpoint(fixture.write(10, 13)).unwrap(), 1);
    drop(store);

    let (mut store, report) = FrontierStore::open(mem.reopen(), config()).unwrap();
    assert_eq!(report.outcome, OpenOutcome::Opened { generation: 1 });
    assert_eq!(
        report.slots,
        [SlotStatus::Valid { generation: 1 }, SlotStatus::Absent]
    );
    let manifest = store.manifest().unwrap().clone();
    assert_eq!(manifest.chain_id, MAINNET_CHAIN_ID);
    assert_eq!(manifest.genesis_state_hash, genesis());
    assert_eq!(manifest.written_at_ms, 1_760_000_000_000);
    assert_eq!(manifest.node_commit.0, "i5a-tests");
    assert_eq!(
        manifest.ledgers,
        [hash_of(&fixture.staking), hash_of(&fixture.snarked)]
    );
    assert_eq!(heights(&manifest.chain), [10, 11, 12, 13]);
    // The snarked ledger differs from the staking ledger in 4 of 22 accounts,
    // so it is stored as a diff.
    assert!(manifest.references(&ledger_full_name(&hash_of(&fixture.staking))));
    assert!(manifest.references(&ledger_diff_name(&hash_of(&fixture.snarked))));
    assert!(manifest.references(&staged_name(&state_hash(10))));
    assert!(manifest.references(&parts_name(&state_hash(10))));
    assert!(store.is_ledger_stored(&hash_of(&fixture.snarked)));
    assert!(store.is_block_stored(&state_hash(13)));
    assert!(!store.is_block_stored(&state_hash(14)));
    assert_eq!(
        store.ledger_base(&hash_of(&fixture.snarked)).unwrap(),
        Some(hash_of(&fixture.staking))
    );
    assert_eq!(store.ledger_base(&hash_of(&fixture.staking)).unwrap(), None);
    // Every file is listed with its length and checksum.
    assert_eq!(
        manifest
            .files
            .iter()
            .map(|file| file.name().to_owned())
            .collect::<BTreeSet<_>>(),
        data_files(&mem)
    );
    for file in &manifest.files {
        let bytes = mem.file(file.name()).unwrap();
        assert_eq!(file.len, bytes.len() as u64);
        assert_eq!(file.crc32, crc32fast::hash(&bytes));
    }

    let loaded = load_all(&mut store, VerifyMode::Full).unwrap();
    assert_fixture_loads(&loaded, &fixture);
    assert_eq!(loaded.blocks, 4);
}

#[test]
fn the_newest_valid_slot_wins_and_a_damaged_one_is_skipped() {
    let mem = MemBackend::new();
    let fixture = Fixture::new(12);
    let (mut store, _) = open(&mem);
    store.checkpoint(fixture.write(10, 11)).unwrap();
    store
        .chain_put(chain(10, 13), blocks(&chain(12, 13)))
        .unwrap();
    drop(store);

    let (store, report) = FrontierStore::open(mem.reopen(), config()).unwrap();
    assert_eq!(report.outcome, OpenOutcome::Opened { generation: 2 });
    assert_eq!(store.previous_manifest().unwrap().generation, 1);
    drop(store);

    // Slot B, generation 2, damaged: generation 1, and the blocks only
    // generation 2 had are deleted.
    let slot_b = mem.file(MANIFEST_SLOTS[1]).unwrap();
    flip(&mem, MANIFEST_SLOTS[1], slot_b.len() / 2);
    let (mut store, report) = FrontierStore::open(mem.reopen(), config()).unwrap();
    assert_eq!(report.outcome, OpenOutcome::Opened { generation: 1 });
    assert_eq!(report.slots[0], SlotStatus::Valid { generation: 1 });
    assert_eq!(
        report.slots[1],
        SlotStatus::Invalid("checksum mismatch".into())
    );
    assert_eq!(
        report.deleted.iter().collect::<BTreeSet<_>>(),
        [block_name(&state_hash(12)), block_name(&state_hash(13))]
            .iter()
            .collect()
    );
    assert_eq!(heights(&store.manifest().unwrap().chain), [10, 11]);
    assert_fixture_loads(&load_all(&mut store, VerifyMode::Full).unwrap(), &fixture);

    // The next write goes to the damaged slot.
    store
        .chain_put(chain(10, 12), blocks(&chain(12, 12)))
        .unwrap();
    assert_eq!(store.generation(), 2);
    assert!(decode_manifest(&mem.file(MANIFEST_SLOTS[1]).unwrap()).is_ok());
    drop(store);

    // Both slots truncated: nothing is loadable, and every data file goes.
    for slot in MANIFEST_SLOTS {
        let bytes = mem.file(slot).unwrap();
        mem.put_file(slot, bytes[..bytes.len() - 1].to_vec());
    }
    let (store, report) = FrontierStore::open(mem.reopen(), config()).unwrap();
    assert_eq!(report.outcome, OpenOutcome::Empty);
    assert!(matches!(report.slots[0], SlotStatus::Invalid(_)));
    assert!(matches!(report.slots[1], SlotStatus::Invalid(_)));
    assert!(store.manifest().is_none());
    assert!(data_files(&mem).is_empty());
}

#[test]
fn a_slot_whose_files_are_missing_or_short_is_skipped() {
    for damage in ["missing", "short"] {
        let mem = MemBackend::new();
        let fixture = Fixture::new(12);
        let (mut store, _) = open(&mem);
        store.checkpoint(fixture.write(10, 11)).unwrap();
        store
            .chain_put(chain(10, 12), blocks(&chain(12, 12)))
            .unwrap();
        drop(store);
        let name = block_name(&state_hash(12));
        match damage {
            "missing" => {
                let mut files = mem.files();
                files.remove(&name);
                mem.set_files(files);
            }
            _ => {
                let bytes = mem.file(&name).unwrap();
                mem.put_file(&name, bytes[1..].to_vec());
            }
        }
        let (mut store, report) = FrontierStore::open(mem.reopen(), config()).unwrap();
        assert_eq!(
            report.outcome,
            OpenOutcome::Opened { generation: 1 },
            "{damage}"
        );
        assert!(matches!(&report.slots[1], SlotStatus::Invalid(reason) if reason.contains(&name)));
        assert_fixture_loads(
            &load_all(&mut store, VerifyMode::default()).unwrap(),
            &fixture,
        );
    }
}

#[test]
fn data_files_are_checked_against_their_checksums_when_read() {
    let mem = MemBackend::new();
    let fixture = Fixture::new(5);
    let (mut store, _) = open(&mem);
    store.checkpoint(fixture.write(10, 11)).unwrap();
    let manifest = store.manifest().unwrap().clone();

    // A block: the same length, so the store opens; the read fails.
    let block = manifest.file(&block_name(&state_hash(11))).unwrap().clone();
    flip(&mem, block.name(), 3);
    let (mut store, report) = FrontierStore::open(mem.reopen(), config()).unwrap();
    assert_eq!(report.outcome, OpenOutcome::Opened { generation: 1 });
    assert!(matches!(
        store.read_blob(&block),
        Err(PersistError::Corrupt { .. })
    ));
    flip(&mem, block.name(), 3);
    store.read_blob(&block).unwrap();

    // Every byte of every ledger file, one at a time.
    let ledger_files = manifest
        .files
        .iter()
        .filter(|file| file.name().starts_with("ledger-") || file.name().starts_with("staged-"))
        .collect::<Vec<_>>();
    assert_eq!(ledger_files.len(), 3);
    for file in ledger_files {
        let original = mem.file(file.name()).unwrap();
        for at in 0..original.len() {
            let mut bad = original.clone();
            bad[at] ^= 0x10;
            mem.put_file(file.name(), bad);
            assert!(
                load_all(&mut store, VerifyMode::RootOnly).is_err(),
                "{} byte {at}",
                file.name()
            );
        }
        mem.put_file(file.name(), original);
    }
    assert_fixture_loads(&load_all(&mut store, VerifyMode::Full).unwrap(), &fixture);
}

#[test]
fn falling_back_gives_up_the_newest_generation() {
    let mem = MemBackend::new();
    let fixture = Fixture::new(8);
    let (mut store, _) = open(&mem);
    store.checkpoint(fixture.write(10, 11)).unwrap();
    let next = fixture.next();
    store.checkpoint(next.write(11, 12)).unwrap();
    assert_eq!(store.generation(), 2);

    assert!(store.fall_back().unwrap());
    assert_eq!(store.generation(), 1);
    assert!(mem.file(MANIFEST_SLOTS[1]).is_none());
    assert!(!data_files(&mem).contains(&staged_name(&state_hash(11))));
    assert_fixture_loads(
        &load_all(&mut store, VerifyMode::default()).unwrap(),
        &fixture,
    );

    assert!(!store.fall_back().unwrap());
    assert!(store.manifest().is_none());
    assert!(data_files(&mem).is_empty());
    assert!(!store.fall_back().unwrap());

    // The store is usable again.
    assert_eq!(store.checkpoint(next.write(11, 12)).unwrap(), 1);
    let (mut store, _) = FrontierStore::open(mem.reopen(), config()).unwrap();
    assert_fixture_loads(&load_all(&mut store, VerifyMode::default()).unwrap(), &next);
}

#[test]
fn chain_puts_extend_fork_and_invalidate() {
    let mem = MemBackend::new();
    let fixture = Fixture::new(12);
    let (mut store, _) = open(&mem);
    store.checkpoint(fixture.write(10, 13)).unwrap();
    let not_invalidated = |generation| ChainPutResult {
        generation,
        checkpoint_invalidated: false,
    };

    // The root moves along the stored chain: the chain is kept from the
    // checkpoint's root.
    let result = store.chain_put(chain(11, 16), blocks(&chain(11, 16)));
    assert_eq!(result.unwrap(), not_invalidated(2));
    let manifest = store.manifest().unwrap();
    assert_eq!(heights(&manifest.chain), (10..=16).collect::<Vec<_>>());
    assert!(manifest.checkpoint.is_some());

    // Stored blocks need not be given again.
    let result = store.chain_put(chain(12, 17), blocks(&chain(17, 17)));
    assert_eq!(result.unwrap(), not_invalidated(3));

    // The same chain again writes nothing.
    let before = mem.files();
    let result = store.chain_put(chain(12, 17), Vec::new());
    assert_eq!(result.unwrap(), not_invalidated(3));
    assert_eq!(mem.files(), before);

    // A fork after 14.
    let fork_hash = |n: u64| state_hash(100 + n);
    let mut fork = chain(13, 14);
    fork.extend((15..=18).map(|n| entry(fork_hash(n), n)));
    let result = store.chain_put(fork.clone(), blocks(&fork[2..]));
    assert_eq!(result.unwrap(), not_invalidated(4));
    let manifest = store.manifest().unwrap();
    assert_eq!(heights(&manifest.chain), (10..=18).collect::<Vec<_>>());
    assert_eq!(manifest.chain[5].hash, fork_hash(15));
    // The old branch is still in the other slot.
    assert!(data_files(&mem).contains(&block_name(&state_hash(16))));
    fork.push(entry(fork_hash(19), 19));
    let result = store.chain_put(fork.clone(), blocks(&fork[6..]));
    assert_eq!(result.unwrap(), not_invalidated(5));
    assert!(!data_files(&mem).contains(&block_name(&state_hash(16))));

    // A block that is neither stored nor given: nothing is committed.
    let mut missing = fork.clone();
    missing.push(entry(fork_hash(20), 20));
    let before = mem.files();
    assert!(matches!(
        store.chain_put(missing, Vec::new()),
        Err(PersistError::Invalid(_))
    ));
    assert_eq!(store.generation(), 5);
    assert_eq!(mem.files(), before);
    assert!(matches!(
        store.chain_put(Vec::new(), Vec::new()),
        Err(PersistError::Invalid(_))
    ));

    // A root that isn't on the stored chain, as after a bootstrap: the
    // checkpoint is dropped and the ledgers are kept.
    let result = store.chain_put(chain(50, 52), blocks(&chain(50, 52)));
    assert_eq!(
        result.unwrap(),
        ChainPutResult {
            generation: 6,
            checkpoint_invalidated: true
        }
    );
    let manifest = store.manifest().unwrap();
    assert!(manifest.checkpoint.is_none());
    assert_eq!(heights(&manifest.chain), [50, 51, 52]);
    assert!(store.is_ledger_stored(&hash_of(&fixture.staking)));
    assert!(store.is_ledger_stored(&hash_of(&fixture.snarked)));
    assert!(!store
        .manifest()
        .unwrap()
        .references(&staged_name(&state_hash(10))));
    // Once neither slot has the checkpoint, its files go.
    let result = store.chain_put(chain(50, 53), blocks(&chain(53, 53)));
    assert_eq!(result.unwrap(), not_invalidated(7));
    let files = data_files(&mem);
    assert!(!files.contains(&staged_name(&state_hash(10))));
    assert!(!files.contains(&parts_name(&state_hash(10))));
    assert!(!files.contains(&block_name(&state_hash(10))));
    drop(store);

    let (mut store, _) = FrontierStore::open(mem.reopen(), config()).unwrap();
    let loaded = load_all(&mut store, VerifyMode::default()).unwrap();
    assert!(loaded.staged.is_none());
    assert_eq!(loaded.blocks, 4);
    assert_same_ledger(
        &loaded.ledgers[&hash_of(&fixture.snarked)],
        &fixture.snarked,
    );

    // A new checkpoint on the new chain reuses its blocks.
    let next = fixture.next();
    let mut write = next.write(51, 53);
    write.blocks.clear();
    assert_eq!(store.checkpoint(write).unwrap(), 8);
    assert_fixture_loads(&load_all(&mut store, VerifyMode::default()).unwrap(), &next);
}

#[test]
fn a_chain_put_on_an_empty_store_keeps_the_chain_for_the_next_checkpoint() {
    let mem = MemBackend::new();
    let (mut store, _) = open(&mem);
    let result = store.chain_put(chain(5, 7), blocks(&chain(5, 7))).unwrap();
    assert_eq!(
        result,
        ChainPutResult {
            generation: 1,
            checkpoint_invalidated: false
        }
    );
    assert!(store.manifest().unwrap().ledgers.is_empty());
    let fixture = Fixture::new(6);
    let mut write = fixture.write(6, 7);
    write.blocks.clear();
    // Its blocks were stored by the chain put.
    store.checkpoint(write).unwrap();
    assert_fixture_loads(
        &load_all(&mut store, VerifyMode::default()).unwrap(),
        &fixture,
    );
}

#[test]
fn checkpoints_are_validated_before_anything_is_written() {
    let mem = MemBackend::new();
    let fixture = Fixture::new(6);
    let (mut store, _) = open(&mem);
    let invalid = |store: &mut FrontierStore<MemBackend>, write| {
        matches!(store.checkpoint(write), Err(PersistError::Invalid(_)))
    };

    let mut write = fixture.write(10, 11);
    write.chain.remove(0);
    assert!(
        invalid(&mut store, write),
        "the chain doesn't start at the root"
    );

    let mut write = fixture.write(10, 11);
    write.ledgers.pop();
    assert!(
        invalid(&mut store, write),
        "the root snarked ledger isn't kept"
    );

    let mut write = fixture.write(10, 11);
    write.staged = merkle_diff(&fixture.staged, &fixture.staking).unwrap();
    assert!(invalid(&mut store, write), "the staged diff's base");

    let mut write = fixture.write(10, 11);
    write.checkpoint.root_staged_ledger_hash = staged_ledger_hash(&fixture.snarked);
    assert!(invalid(&mut store, write), "the staged diff's target");

    let mut write = fixture.write(10, 11);
    write.ledgers[1].1 = None;
    assert!(
        invalid(&mut store, write),
        "a ledger neither stored nor given"
    );

    let mut write = fixture.write(10, 11);
    write.ledgers[0].1 = Some(LedgerWrite::Full(fixture.snarked.clone()));
    assert!(
        invalid(&mut store, write),
        "a ledger given for another hash"
    );

    let mut write = fixture.write(10, 11);
    write.ledgers[1].1 = Some(LedgerWrite::Diff(
        merkle_diff(&fixture.staged, &fixture.snarked).unwrap(),
    ));
    assert!(invalid(&mut store, write), "a diff for another hash");

    assert_eq!(store.generation(), 0);
    assert!(data_files(&mem).is_empty());
}

#[test]
fn stored_ledgers_are_written_once_and_a_full_file_cuts_a_diff_chain() {
    let mem = MemBackend::new();
    let fixture = Fixture::new(16);
    let (mut store, _) = open(&mem);
    store.checkpoint(fixture.write(10, 11)).unwrap();
    let staking = hash_of(&fixture.staking);
    let snarked = hash_of(&fixture.snarked);
    let files = mem.files();

    // Given again or not at all, a stored ledger costs the same bytes: none.
    let bytes_written = |ledgers: Vec<(v2::LedgerHash, Option<LedgerWrite>)>| {
        let mem = MemBackend::new();
        mem.set_files(files.clone());
        let (mut store, _) = open(&mem);
        mem.set_faults(Faults::default());
        let mut write = fixture.write(11, 12);
        write.ledgers = ledgers;
        store.checkpoint(write).unwrap();
        mem.counters().1
    };
    let none = bytes_written(vec![(staking.clone(), None), (snarked.clone(), None)]);
    let given = bytes_written(fixture.write(11, 12).ledgers);
    let bogus_diff = LedgerWrite::Diff(merkle_diff(&fixture.staged, &fixture.snarked).unwrap());
    let ignored = bytes_written(vec![
        (
            staking.clone(),
            Some(LedgerWrite::Full(fixture.staking.clone())),
        ),
        (snarked.clone(), Some(bogus_diff)),
    ]);
    assert_eq!(none, given);
    assert_eq!(none, ignored);

    // A full file replaces the stored diff, so the staking ledger can go.
    let mut write = fixture.write(11, 12);
    write.ledgers = vec![(
        snarked.clone(),
        Some(LedgerWrite::Full(fixture.snarked.clone())),
    )];
    store.checkpoint(write).unwrap();
    let manifest = store.manifest().unwrap();
    assert_eq!(manifest.ledgers, [snarked.clone()]);
    assert!(manifest.references(&ledger_full_name(&snarked)));
    assert!(!manifest.references(&ledger_diff_name(&snarked)));
    assert!(!manifest.references(&ledger_full_name(&staking)));
    // The previous generation still needs them.
    assert!(data_files(&mem).contains(&ledger_full_name(&staking)));
    let mut write = fixture.write(12, 13);
    write.ledgers = vec![(snarked.clone(), None)];
    store.checkpoint(write).unwrap();
    let files = data_files(&mem);
    assert!(!files.contains(&ledger_full_name(&staking)));
    assert!(!files.contains(&ledger_diff_name(&snarked)));
    assert!(!store.is_ledger_stored(&staking));

    let (mut store, _) = FrontierStore::open(mem.reopen(), config()).unwrap();
    let loaded = load_all(&mut store, VerifyMode::Full).unwrap();
    assert_same_ledger(&loaded.ledgers[&snarked], &fixture.snarked);
    assert_same_ledger(loaded.staged.as_ref().unwrap(), &fixture.staged);
}

#[test]
fn a_chain_of_diffs_loads_and_keeps_its_bases() {
    let staking = random_ledger(16);
    let next = modified(&staking, &[2], 1);
    let snarked = modified(&next, &[5], 0);
    let staged = modified(&snarked, &[1], 0);
    let fixture = Fixture {
        staking: staking.clone(),
        snarked: snarked.clone(),
        staged,
    };
    let mut write = fixture.write(10, 11);
    write.ledgers = vec![
        (hash_of(&staking), Some(LedgerWrite::Full(staking.clone()))),
        (
            hash_of(&next),
            Some(LedgerWrite::Diff(merkle_diff(&next, &staking).unwrap())),
        ),
        (
            hash_of(&snarked),
            Some(LedgerWrite::Diff(merkle_diff(&snarked, &next).unwrap())),
        ),
    ];
    let mem = MemBackend::new();
    let (mut store, _) = open(&mem);
    store.checkpoint(write).unwrap();
    let loaded = load_all(&mut store, VerifyMode::Full).unwrap();
    assert_same_ledger(&loaded.ledgers[&hash_of(&next)], &next);
    assert_fixture_loads(&loaded, &fixture);

    // Keeping only the root snarked ledger keeps the files of its bases,
    // which no longer count as stored.
    for (generation, root) in [(2, 11), (3, 12)] {
        let mut write = fixture.write(root, root + 1);
        write.ledgers = vec![(hash_of(&snarked), None)];
        assert_eq!(store.checkpoint(write).unwrap(), generation);
    }
    let manifest = store.manifest().unwrap();
    assert_eq!(manifest.ledgers, [hash_of(&snarked)]);
    assert!(manifest.references(&ledger_full_name(&hash_of(&staking))));
    assert!(manifest.references(&ledger_diff_name(&hash_of(&next))));
    assert!(!store.is_ledger_stored(&hash_of(&staking)));
    assert!(!store.is_ledger_stored(&hash_of(&next)));
    let (mut store, _) = FrontierStore::open(mem.reopen(), config()).unwrap();
    let loaded = load_all(&mut store, VerifyMode::Full).unwrap();
    assert_same_ledger(&loaded.ledgers[&hash_of(&snarked)], &snarked);
}

/// I5d4: the ledgers are decoded by rounds, those of a round in parallel:
/// two diffs over one base, each over its own copy of it, and a chain of
/// diffs, each over the ledger the round before loaded. As loaded one at a
/// time, a ledger more than `MAX_DIFF_CHAIN` diffs above a full file is
/// refused, and so is one whose base isn't there.
#[test]
fn ledgers_decode_by_rounds_and_long_chains_are_refused() {
    use super::{
        format::MAX_DIFF_CHAIN,
        store::{decode_ledgers, LedgerFiles},
    };

    let staking = random_ledger(16);
    let mut chain = vec![staking.clone()];
    for i in 0..=MAX_DIFF_CHAIN as u64 {
        chain.push(modified(chain.last().unwrap(), &[i % 16], 1));
    }
    let sibling = modified(&staking, &[3, 7], 2);
    let files = |upto: usize, sibling_too: bool| {
        let mut files = vec![full_bytes(&staking)];
        for pair in chain[..=upto].windows(2) {
            files.push(diff_bytes(&merkle_diff(&pair[1], &pair[0]).unwrap()));
        }
        if sibling_too {
            files.push(diff_bytes(&merkle_diff(&sibling, &staking).unwrap()));
        }
        LedgerFiles::from_bytes(files)
    };

    // Up to MAX_DIFF_CHAIN diffs above the full file, and a sibling over the
    // same base, all load as they were.
    let loaded = decode_ledgers(files(MAX_DIFF_CHAIN, true), VerifyMode::Full).unwrap();
    assert_eq!(loaded.len(), MAX_DIFF_CHAIN + 2);
    for ledger in chain[..=MAX_DIFF_CHAIN].iter().chain([&sibling]) {
        assert_same_ledger(&loaded[&hash_of(ledger)], ledger);
    }
    // The base is left as it is by both diffs over it.
    assert_same_ledger(&loaded[&hash_of(&staking)], &staking);

    // One more is refused.
    let error = decode_ledgers(files(MAX_DIFF_CHAIN + 1, false), VerifyMode::Full).unwrap_err();
    assert!(error.to_string().contains("too long"), "{error}");

    // A diff whose base is missing never loads.
    let orphan = LedgerFiles::from_bytes(vec![diff_bytes(
        &merkle_diff(&chain[2], &chain[1]).unwrap(),
    )]);
    let error = decode_ledgers(orphan, VerifyMode::Full).unwrap_err();
    assert!(error.to_string().contains("too long"), "{error}");

    // A changed account in one of two diffs over one base fails the load.
    let mut bad = diff_bytes(&merkle_diff(&sibling, &staking).unwrap());
    let at = bad.len() - 200;
    bad[at] ^= 1;
    let damaged = LedgerFiles::from_bytes(vec![
        full_bytes(&staking),
        diff_bytes(&merkle_diff(&chain[1], &staking).unwrap()),
        bad,
    ]);
    assert!(decode_ledgers(damaged, VerifyMode::Full).is_err());
}

#[test]
fn version_and_chain_rules() {
    let stored = || {
        let mem = MemBackend::new();
        let (mut store, _) = open(&mem);
        store.checkpoint(Fixture::new(5).write(10, 11)).unwrap();
        store
            .set_attempts(Attempts {
                generation: 1,
                count: 1,
            })
            .unwrap();
        mem.put_file("README", b"not ours".to_vec());
        drop(store);
        mem
    };
    let set_version = |mem: &MemBackend, version: u32| {
        let mut slot = mem.file(MANIFEST_SLOTS[0]).unwrap();
        slot[8..12].copy_from_slice(&version.to_le_bytes());
        mem.put_file(MANIFEST_SLOTS[0], slot);
    };
    let assert_reset = |mem: &MemBackend, config: StoreConfig| {
        let (store, report) = FrontierStore::open(mem.reopen(), config).unwrap();
        assert!(matches!(report.outcome, OpenOutcome::Reset { .. }));
        assert!(store.manifest().is_none());
        assert_eq!(
            mem.files().into_keys().collect::<Vec<_>>(),
            ["README"],
            "only files that aren't the store's are left"
        );
    };

    // A newer format version: left as it is, and the store doesn't open.
    let mem = stored();
    set_version(&mem, FORMAT_VERSION + 1);
    let before = mem.files();
    assert!(matches!(
        FrontierStore::open(mem.reopen(), config()),
        Err(PersistError::NewerFormat { found, ours }) if found == FORMAT_VERSION + 1 && ours == FORMAT_VERSION
    ));
    assert_eq!(mem.files(), before);

    // An older one, another chain id or another genesis: deleted.
    let mem = stored();
    set_version(&mem, FORMAT_VERSION - 1);
    assert_reset(&mem, config());
    assert_reset(&stored(), config_for(DEVNET_CHAIN_ID, genesis()));
    assert_reset(&stored(), config_for(MAINNET_CHAIN_ID, state_hash(99)));
}

#[test]
fn a_second_process_cannot_open_a_locked_store() {
    let mem = MemBackend::new();
    let (_store, _) = open(&mem);
    assert!(matches!(
        FrontierStore::open(mem.second_handle(), config()),
        Err(PersistError::Locked)
    ));
    // Once its holder is gone, a restarted process takes it.
    assert!(FrontierStore::open(mem.reopen(), config()).is_ok());
}

#[test]
fn opening_deletes_only_unreferenced_data_files() {
    let mem = MemBackend::new();
    let fixture = Fixture::new(5);
    let (mut store, _) = open(&mem);
    store.checkpoint(fixture.write(10, 11)).unwrap();
    drop(store);
    let referenced = data_files(&mem);
    let junk = [
        "ledger-torn.full",
        "ledger-torn.diff",
        "staged-torn.diff",
        "parts-torn.bin",
        "block-torn.bin",
    ];
    for name in junk {
        mem.put_file(name, vec![1, 2, 3]);
    }
    for name in ["README", "MANIFEST-C", "LOCK", "ledger"] {
        mem.put_file(name, vec![4]);
    }
    let (mut store, report) = FrontierStore::open(mem.reopen(), config()).unwrap();
    assert_eq!(
        report
            .deleted
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        junk.into_iter().collect()
    );
    assert_eq!(data_files(&mem), referenced);
    for name in ["README", "MANIFEST-C", "LOCK", "ledger"] {
        assert!(mem.file(name).is_some(), "{name}");
    }
    assert_fixture_loads(
        &load_all(&mut store, VerifyMode::default()).unwrap(),
        &fixture,
    );
}

// Crash injection (D4, D8): a failure at every call of an operation, then at
// a spread of byte offsets, then a restart.

/// Runs `op` on a store opened over a copy of `files`, failing as `faults`
/// says, and returns the backend, whether the operation succeeded, and the
/// calls and bytes it made.
fn run_op(files: &Files, faults: Faults, op: Op<'_>) -> (MemBackend, bool, (u64, u64)) {
    let mem = MemBackend::new();
    mem.set_files(files.clone());
    let (mut store, _) = open(&mem);
    mem.set_faults(faults);
    let ok = op(&mut store).is_ok();
    let counters = mem.counters();
    (mem, ok, counters)
}

fn manifest_of(files: &Files) -> Option<Manifest> {
    let mem = MemBackend::new();
    mem.set_files(files.clone());
    open(&mem).0.manifest().cloned()
}

/// A failure at each call, then at a spread of byte offsets: the start, a
/// few small offsets, every sixteenth of the bytes, and around and inside the
/// manifest, which is written last.
fn fault_plan(calls: u64, bytes: u64, manifest_len: u64) -> Vec<Faults> {
    let mut plan = (0..calls)
        .map(|call| Faults {
            fail_at_call: Some(call),
            fail_at_byte: None,
        })
        .collect::<Vec<_>>();
    let manifest_start = bytes.saturating_sub(manifest_len);
    let mut offsets = BTreeSet::from([0, 1, 2, 100, 1000]);
    offsets.extend((1..16).map(|k| bytes * k / 16));
    offsets.extend([
        manifest_start.saturating_sub(1),
        manifest_start,
        manifest_start + 1,
        manifest_start + manifest_len / 2,
        bytes.saturating_sub(1),
    ]);
    plan.extend(
        offsets
            .into_iter()
            .filter(|&offset| offset < bytes)
            .map(|offset| Faults {
                fail_at_call: None,
                fail_at_byte: Some(offset),
            }),
    );
    plan
}

/// After a crash, the restarted store opens at the old or the new generation,
/// everything it references loads, no data file is left that no slot
/// references, and the operation then succeeds. Returns whether the store is
/// at the new generation.
fn check_recovered(
    mem: &MemBackend,
    old: Option<&Manifest>,
    new: &Manifest,
    op: Op<'_>,
    what: &str,
) -> bool {
    let (mut store, _) = FrontierStore::open(mem.reopen(), config())
        .unwrap_or_else(|error| panic!("{what}: the restart failed: {error}"));
    let manifest = store.manifest().cloned();
    let is_new = manifest.as_ref() == Some(new);
    assert!(
        is_new || manifest.as_ref() == old,
        "{what}: neither the old nor the new generation: {:?}",
        manifest.map(|manifest| manifest.generation)
    );
    load_all(&mut store, VerifyMode::RootOnly)
        .unwrap_or_else(|error| panic!("{what}: the store doesn't load: {error}"));
    for name in data_files(mem) {
        let referenced = store.manifest().is_some_and(|m| m.references(&name))
            || store
                .previous_manifest()
                .is_some_and(|m| m.references(&name));
        assert!(referenced, "{what}: {name} was left behind");
    }
    op(&mut store).unwrap_or_else(|error| panic!("{what}: the store isn't writable: {error}"));
    load_all(&mut store, VerifyMode::RootOnly)
        .unwrap_or_else(|error| panic!("{what}: the store doesn't load after a write: {error}"));
    is_new
}

fn crash_everywhere(what: &str, before: &Files, op: Op<'_>) {
    let old = manifest_of(before);
    let (clean, ok, (calls, bytes)) = run_op(before, Faults::default(), op);
    assert!(ok, "{what}: the operation fails without faults");
    let new = manifest_of(&clean.files()).unwrap();
    assert_ne!(Some(&new), old.as_ref());
    let manifest_len = encode_manifest(&new).len() as u64;
    let plan = fault_plan(calls, bytes, manifest_len);
    let mut outcomes = [0; 2];
    for faults in &plan {
        let (mem, _, _) = run_op(before, *faults, op);
        let is_new = check_recovered(&mem, old.as_ref(), &new, op, &format!("{what}, {faults:?}"));
        outcomes[is_new as usize] += 1;
    }
    eprintln!(
        "{what}: {calls} calls, {bytes} bytes, {} crashes: {} at the old generation, {} at the new",
        plan.len(),
        outcomes[0],
        outcomes[1]
    );
    assert!(outcomes[0] > 0 && outcomes[1] > 0, "{what}: {outcomes:?}");
}

/// A store with a checkpoint in slot A and a chain put in slot B.
fn two_generations(fixture: &Fixture) -> Files {
    let mem = MemBackend::new();
    let (mut store, _) = open(&mem);
    store.checkpoint(fixture.write(10, 12)).unwrap();
    store
        .chain_put(chain(10, 13), blocks(&chain(13, 13)))
        .unwrap();
    mem.files()
}

#[test]
fn a_crash_anywhere_in_a_checkpoint_leaves_a_loadable_store() {
    let fixture = Fixture::new(6);
    let next = fixture.next();
    let before = two_generations(&fixture);
    crash_everywhere("checkpoint", &before, &|store| {
        store.checkpoint(next.write(11, 14)).map(|_| ())
    });
    // The first checkpoint, on an empty store.
    crash_everywhere("first checkpoint", &Files::new(), &|store| {
        store.checkpoint(fixture.write(10, 12)).map(|_| ())
    });
}

#[test]
fn a_crash_anywhere_in_a_chain_put_leaves_a_loadable_store() {
    let fixture = Fixture::new(6);
    let before = two_generations(&fixture);
    crash_everywhere("chain put", &before, &|store| {
        store
            .chain_put(chain(11, 16), blocks(&chain(14, 16)))
            .map(|_| ())
    });
    crash_everywhere("chain put off the chain", &before, &|store| {
        store
            .chain_put(chain(40, 42), blocks(&chain(40, 42)))
            .map(|_| ())
    });
}

#[test]
fn a_crash_anywhere_while_opening_leaves_a_loadable_store() {
    let fixture = Fixture::new(6);
    let mut before = two_generations(&fixture);
    for name in ["ledger-torn.full", "staged-torn.diff", "block-torn.bin"] {
        before.insert(name.to_owned(), vec![0; 10]);
    }
    let expected = manifest_of(&before);
    let calls = {
        let mem = MemBackend::new();
        mem.set_files(before.clone());
        mem.set_faults(Faults::default());
        FrontierStore::open(mem.second_handle(), config()).unwrap();
        mem.counters().0
    };
    for call in 0..calls {
        let mem = MemBackend::new();
        mem.set_files(before.clone());
        mem.set_faults(Faults {
            fail_at_call: Some(call),
            fail_at_byte: None,
        });
        assert!(FrontierStore::open(mem.second_handle(), config()).is_err());
        let (mut store, _) = FrontierStore::open(mem.reopen(), config()).unwrap();
        assert_eq!(
            store.manifest().map(|m| m.generation),
            expected.as_ref().map(|m| m.generation)
        );
        assert!(data_files(&mem).iter().all(|name| !name.contains("torn")));
        load_all(&mut store, VerifyMode::RootOnly).unwrap();
    }
}

// The native backend.

#[cfg(not(target_family = "wasm"))]
#[test]
fn the_native_backend_stores_locks_and_collects() {
    // Keep this store private even when several test binaries run concurrently.
    let dir = tempfile::tempdir().unwrap();
    let store_dir = fs_backend::store_dir(dir.path(), &MAINNET_CHAIN_ID);
    assert!(store_dir.ends_with(format!("{}/frontier", MAINNET_CHAIN_ID.to_hex())));
    let fixture = Fixture::new(10);
    let next = fixture.next();

    let (mut store, report) =
        FrontierStore::open(FsBackend::new(&store_dir).unwrap(), config()).unwrap();
    assert_eq!(report.outcome, OpenOutcome::Empty);
    store.checkpoint(fixture.write(10, 12)).unwrap();
    store
        .chain_put(chain(11, 14), blocks(&chain(13, 14)))
        .unwrap();
    store.checkpoint(next.write(12, 14)).unwrap();

    // Locked while the store is open, in this process too.
    assert!(matches!(
        FrontierStore::open(FsBackend::new(&store_dir).unwrap(), config()),
        Err(PersistError::Locked)
    ));
    drop(store);

    std::fs::write(store_dir.join("ledger-torn.full"), [1, 2, 3]).unwrap();
    std::fs::write(store_dir.join("README"), "not ours").unwrap();
    std::fs::create_dir(store_dir.join("block-dir.bin")).unwrap();
    let (mut store, report) =
        FrontierStore::open(FsBackend::new(&store_dir).unwrap(), config()).unwrap();
    assert_eq!(report.outcome, OpenOutcome::Opened { generation: 3 });
    assert_eq!(report.deleted, ["ledger-torn.full"]);
    assert!(store_dir.join("README").exists());
    let loaded = load_all(&mut store, VerifyMode::Full).unwrap();
    assert_fixture_loads(&loaded, &next);
    assert_eq!(loaded.blocks, 3);
    let manifest = store.manifest().unwrap().clone();
    for file in &manifest.files {
        let bytes = std::fs::read(store_dir.join(file.name())).unwrap();
        assert_eq!(
            (file.len, file.crc32),
            (bytes.len() as u64, crc32fast::hash(&bytes))
        );
    }
    store
        .set_attempts(Attempts {
            generation: 3,
            count: 1,
        })
        .unwrap();
    assert_eq!(store.attempts().unwrap().unwrap().count, 1);
    // Every data file (both generations'), both slots and the attempts file.
    let data = std::fs::read_dir(&store_dir)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_file())
        .filter(|entry| is_data_file(&entry.file_name().to_string_lossy()))
        .count();
    assert_eq!(store.reset().unwrap().len(), data + 3);
    drop(store);

    let (store, report) =
        FrontierStore::open(FsBackend::new(&store_dir).unwrap(), config()).unwrap();
    assert_eq!(report.outcome, OpenOutcome::Empty);
    assert!(store.manifest().is_none());
    drop(store);
    assert!(FsBackend::new(&store_dir)
        .unwrap()
        .create("../escape")
        .is_err());
}

// I5b: the root staged ledger without a rebuild.

/// I5b (R20 D8): a mainnet root staged ledger, rebuilt the slow way from a
/// reconstruct dump, then stored and loaded as a restart would: the snarked
/// ledger as a full file, the staged ledger as a diff over it, and the parts
/// as the wire type a node serves for a root. `from_persisted_unchecked` on
/// what is loaded must give the rebuilt staged ledger's hash. Logs the times
/// of both ways.
///
/// ```text
/// MINA_RECONSTRUCT_DUMP=$ROOT/.work/reconstruct-current.binprot \
///     target/release/deps/mina_node-<hash> --ignored --nocapture \
///     persisted_root_staged_ledger_matches_reconstruct
/// ```
///
/// The store goes to `I5B_STORE_DIR` (default: a new directory in the
/// temporary directory), which is deleted afterwards.
#[cfg(not(target_family = "wasm"))]
#[test]
#[ignore = "needs MINA_RECONSTRUCT_DUMP, a staged ledger dump; run in release"]
fn persisted_root_staged_ledger_matches_reconstruct() {
    use std::{collections::HashMap, path::PathBuf, time::Instant};

    use ledger::{
        scan_state::{
            pending_coinbase::PendingCoinbase, scan_state::ScanState,
            transaction_logic::local_state::LocalState,
        },
        staged_ledger::staged_ledger::StagedLedger,
        verifier::Verifier,
    };
    use rayon::prelude::*;

    use crate::p2p::channels::rpc::StagedLedgerAuxAndPendingCoinbases;

    let dump = std::env::var_os("MINA_RECONSTRUCT_DUMP").expect(
        "MINA_RECONSTRUCT_DUMP: a staged ledger dump, such as .work/reconstruct-current.binprot",
    );
    let _ = mina_core::NetworkConfig::init("mainnet");
    let constants = mina_core::constants::constraint_constants();
    let depth = constants.ledger_depth as usize;

    // The dump (`ReconstructContext` in `ledger_service.rs`): the snarked
    // ledger's accounts, then the parts a node serves for a root
    // (`get_staged_ledger_aux_and_pending_coinbases_at_hash`), in another
    // order.
    let started = Instant::now();
    let bytes = std::fs::read(&dump).unwrap();
    let mut reader = bytes.as_slice();
    let accounts = Vec::<v2::MinaBaseAccountBinableArgStableV2>::binprot_read(&mut reader).unwrap();
    let scan_state = v2::TransactionSnarkScanStateStableV2::binprot_read(&mut reader).unwrap();
    let pending_coinbase = v2::MinaBasePendingCoinbaseStableV2::binprot_read(&mut reader).unwrap();
    let staged_ledger_hash = v2::LedgerHash::binprot_read(&mut reader).unwrap();
    let needed_blocks =
        mina_p2p_messages::list::List::<v2::MinaStateProtocolStateValueStableV2>::binprot_read(
            &mut reader,
        )
        .unwrap();
    assert!(reader.is_empty());
    let parts = StagedLedgerAuxAndPendingCoinbases {
        scan_state,
        staged_ledger_hash,
        pending_coinbase,
        needed_blocks,
    };
    drop(bytes);
    eprintln!(
        "dump: {} accounts, {} protocol states, read in {:.2?}",
        accounts.len(),
        parts.needed_blocks.len(),
        started.elapsed()
    );

    // The slow way, as after a root ledger sync: the snarked ledger from its
    // accounts, hashed in parallel as `mina-block-replay` does, then the scan
    // state's transactions applied again.
    let started = Instant::now();
    let accounts = accounts
        .par_iter()
        .map(|account| Account::try_from(account).unwrap())
        .collect::<Vec<_>>();
    let leaf_hashes: Vec<Fp> = accounts.par_iter().map(Account::hash).collect();
    let mut snarked = Mask::create_with_token_owners(depth);
    for account in accounts {
        snarked
            .get_or_create_account(account.id(), account)
            .unwrap();
    }
    for (index, hash) in leaf_hashes.into_iter().enumerate() {
        let leaf = Address::from_index(AccountIndex(index as u64), depth);
        snarked.set_cached_hash_unchecked(&leaf, hash);
    }
    let snarked_hash = snarked.merkle_root();
    let build_time = started.elapsed();

    let started = Instant::now();
    let states = parts
        .needed_blocks
        .iter()
        .map(|state| {
            let hash = state.try_hash().unwrap().to_field::<Fp>().unwrap();
            (hash, state.clone())
        })
        .collect::<HashMap<_, _>>();
    let mut rebuilt = StagedLedger::of_scan_state_pending_coinbases_and_snarked_ledger(
        (),
        constants,
        Verifier,
        (&parts.scan_state).try_into().unwrap(),
        snarked.make_child(),
        LocalState::empty(),
        parts.staged_ledger_hash.to_field().unwrap(),
        (&parts.pending_coinbase).try_into().unwrap(),
        |hash| states[&hash].clone(),
    )
    .unwrap();
    let expected = rebuilt.hash();
    let reconstruct_time = started.elapsed();
    eprintln!(
        "slow way: snarked ledger {} built in {build_time:.2?}, staged ledger rebuilt in \
         {reconstruct_time:.2?}",
        v2::LedgerHash::from_fp(snarked_hash)
    );

    // Stored as a checkpoint stores it. The dump doesn't say which block it
    // is the root of, so the root's state hash and block are stand-ins.
    let dir = match std::env::var_os("I5B_STORE_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join(format!("mina-i5b-{}", std::process::id())),
    };
    let root = state_hash(0);
    let started = Instant::now();
    let staged_diff = merkle_diff(&rebuilt.ledger(), &snarked).unwrap();
    let diff_time = started.elapsed();
    let (diff_accounts, diff_hashes) = (staged_diff.accounts.len(), staged_diff.hashes.len());
    let mut parts_bytes = Vec::new();
    parts.binprot_write(&mut parts_bytes).unwrap();
    let write = CheckpointWrite {
        checkpoint: Checkpoint {
            root: root.clone(),
            root_height: 0,
            root_snarked_ledger: v2::LedgerHash::from_fp(snarked_hash),
            root_staged_ledger_hash: (&expected).into(),
        },
        ledgers: vec![(
            v2::LedgerHash::from_fp(snarked_hash),
            Some(LedgerWrite::Full(snarked.clone())),
        )],
        staged: staged_diff,
        parts: blob(parts_bytes),
        chain: vec![entry(root.clone(), 0)],
        blocks: vec![(root.clone(), blob(block_bytes(&root)))],
    };
    let started = Instant::now();
    let (mut store, _) = FrontierStore::open(FsBackend::new(&dir).unwrap(), config()).unwrap();
    store.checkpoint(write).unwrap();
    drop(store);
    let write_time = started.elapsed();
    eprintln!(
        "stored: staged diff of {diff_accounts} accounts and {diff_hashes} hashes taken in \
         {diff_time:.2?}, checkpoint written in {write_time:.2?} to {}",
        dir.display()
    );

    // Loaded as a restart would load it.
    let started = Instant::now();
    let (mut store, report) = FrontierStore::open(FsBackend::new(&dir).unwrap(), config()).unwrap();
    assert_eq!(report.outcome, OpenOutcome::Opened { generation: 1 });
    let manifest = store.manifest().unwrap().clone();
    for file in &manifest.files {
        eprintln!("  {}: {} bytes", file.name(), file.len);
    }
    let checkpoint = manifest.checkpoint.clone().unwrap();
    let ledgers = store
        .read_ledgers(&manifest, VerifyMode::default())
        .unwrap();
    let snarked_loaded = &ledgers[&checkpoint.root_snarked_ledger];
    let snarked_time = started.elapsed();
    let started = Instant::now();
    let staged_ledger = store
        .read_staged(&manifest, snarked_loaded, VerifyMode::default())
        .unwrap();
    let staged_time = started.elapsed();
    let started = Instant::now();
    let parts_file = manifest.file(&parts_name(&root)).unwrap().clone();
    let parts_bytes = store.read_blob(&parts_file).unwrap();
    let loaded_parts =
        StagedLedgerAuxAndPendingCoinbases::binprot_read(&mut parts_bytes.as_slice()).unwrap();
    let scan_state: ScanState = (&loaded_parts.scan_state).try_into().unwrap();
    let pending_coinbase: PendingCoinbase = (&loaded_parts.pending_coinbase).try_into().unwrap();
    let parts_time = started.elapsed();

    // I5d5: the loaded staged ledger is a mask over the loaded snarked
    // ledger, holding the diff's accounts. The loader makes it hold every
    // account the scan state references too; then it holds every account the
    // rebuilt one does (each account the transactions set), and reads the
    // same.
    let started = Instant::now();
    assert!(staged_ledger.is_attached());
    assert_eq!(
        staged_ledger.get_parent().unwrap().get_uuid(),
        snarked_loaded.get_uuid()
    );
    assert_eq!(staged_ledger.own_account_indexes().len(), diff_accounts);
    let referenced = scan_state.staged_accounts_referenced().len();
    let held = super::load::hold_referenced_accounts(&staged_ledger, &scan_state);
    let hold_time = started.elapsed();
    let restored_own = staged_ledger
        .own_account_indexes()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let rebuilt_own = rebuilt.ledger().own_account_indexes();
    let changed_back = rebuilt_own
        .iter()
        .filter(|index| rebuilt.ledger().get_at_index(**index) == snarked.get_at_index(**index))
        .count();
    assert!(rebuilt_own.iter().all(|index| restored_own.contains(index)));
    assert_eq!(
        staged_ledger.validate_child_account_indexes(),
        Ok(restored_own.len())
    );
    eprintln!(
        "I5d5: the rebuilt mask holds {} accounts ({changed_back} the same as the snarked \
         ledger's), the diff {diff_accounts}; the scan state references {referenced}, of \
         which {held} more are now held, {} in all, in {hold_time:.2?}",
        rebuilt_own.len(),
        restored_own.len()
    );

    let started = Instant::now();
    let mut persisted = StagedLedger::from_persisted_unchecked(
        constants,
        scan_state,
        staged_ledger,
        pending_coinbase,
    );
    let hash = persisted.hash();
    let hash_time = started.elapsed();
    eprintln!(
        "persisted way: open and snarked ledger {snarked_time:.2?}, staged ledger \
         {staged_time:.2?}, parts {parts_time:.2?}, from_persisted_unchecked and hash \
         {hash_time:.2?}; total {:.2?}",
        snarked_time + staged_time + parts_time + hash_time
    );

    assert_eq!(hash, expected);
    assert_eq!(
        v2::LedgerHash::from_fp(persisted.ledger().merkle_root()),
        parts.staged_ledger_hash
    );
    assert_eq!(
        v2::MinaBaseStagedLedgerHashStableV1::from(&hash),
        checkpoint.root_staged_ledger_hash
    );
    drop(store);
    std::fs::remove_dir_all(&dir).unwrap();
}

// Regression tests for I5a's malformed-file review findings.

#[test]
fn manifest_collection_lengths_are_bounded_before_allocation() {
    use super::format::{MANIFEST_MAGIC, MAX_CHAIN_ENTRIES, MAX_FILES, MAX_LEDGERS};
    use mina_p2p_messages::binprot::{Error, Nat0, OCAML_MAX_INT};

    let manifest = sample_manifest();
    let mut prefix = Vec::new();
    manifest.generation.binprot_write(&mut prefix).unwrap();
    manifest.chain_id.binprot_write(&mut prefix).unwrap();
    manifest
        .genesis_state_hash
        .binprot_write(&mut prefix)
        .unwrap();
    manifest.written_at_ms.binprot_write(&mut prefix).unwrap();
    manifest.node_commit.binprot_write(&mut prefix).unwrap();
    for (field, limit) in [MAX_LEDGERS, MAX_CHAIN_ENTRIES, MAX_FILES]
        .into_iter()
        .enumerate()
    {
        for count in [
            u64::MAX,
            OCAML_MAX_INT as u64,
            limit as u64 + 1,
            limit as u64,
        ] {
            let mut body = prefix.clone();
            if field > 0 {
                Nat0(0).binprot_write(&mut body).unwrap(); // ledgers
                Option::<Checkpoint>::None.binprot_write(&mut body).unwrap();
            }
            if field > 1 {
                Nat0(0).binprot_write(&mut body).unwrap(); // chain
            }
            Nat0(count).binprot_write(&mut body).unwrap();
            // Enough bytes for the count, but no entries. Even an in-range
            // count must fail against the remaining body before allocation.
            let mut bytes = MANIFEST_MAGIC.to_vec();
            bytes.extend(FORMAT_VERSION.to_le_bytes());
            bytes.extend((body.len() as u32).to_le_bytes());
            bytes.extend(body);
            bytes.extend(crc32fast::hash(&bytes).to_le_bytes());
            // OCaml bin_prot's Nat0 reader rejects values above max_int
            // before the store sees the collection length. The largest
            // representable Nat0 must still hit the store's own bound.
            // OCaml reference: libs/binprot-rs/src/int.rs, read_nat0.
            let expected = if count > OCAML_MAX_INT as u64 {
                Error::IoError(io::Error::new(io::ErrorKind::InvalidData, "Nat0_overflow"))
            } else {
                Error::TooMuchData(count)
            };
            assert_eq!(
                decode_manifest(&bytes),
                Err(SlotError::Decode(expected.to_string()))
            );
        }
    }
}

#[test]
fn manifest_collection_and_file_limits_accept_the_boundary() {
    use super::format::*;
    let mut manifest = sample_manifest();
    manifest.ledgers = vec![manifest.ledgers[0].clone(); MAX_LEDGERS];
    manifest.chain = vec![manifest.chain[0].clone(); MAX_CHAIN_ENTRIES];
    manifest.files = vec![FileRef::new("block-x.bin", MAX_BLOCK_FILE, 0); MAX_FILES];
    assert_eq!(
        decode_manifest(&encode_manifest(&manifest)).unwrap(),
        manifest
    );
    for (name, limit) in [
        ("ledger-x.full", MAX_LEDGER_FILE),
        ("ledger-x.diff", MAX_LEDGER_FILE),
        ("staged-x.diff", MAX_LEDGER_FILE),
        ("parts-x.bin", MAX_PARTS_FILE),
        ("block-x.bin", MAX_BLOCK_FILE),
    ] {
        manifest.files = vec![FileRef::new(name, limit, 0)];
        assert!(decode_manifest(&encode_manifest(&manifest)).is_ok());
        manifest.files[0].len += 1;
        assert!(decode_manifest(&encode_manifest(&manifest)).is_err());
    }
}

/// Metadata-only files: any payload read fails the test. This verifies that
/// physical size checks precede reading, buffering and decoded allocations.
struct MetadataBackend {
    name: &'static str,
    len: u64,
}

struct UnreadableFile(u64);
impl Read for UnreadableFile {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("oversized file payload was read")
    }
}
impl StoreReader for UnreadableFile {
    fn len(&self) -> u64 {
        self.0
    }
}
impl StoreBackend for MetadataBackend {
    fn lock(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn list(&mut self) -> io::Result<Vec<String>> {
        Ok(if MANIFEST_SLOTS.contains(&self.name) {
            vec![self.name.into()]
        } else {
            vec![]
        })
    }
    fn open(&mut self, name: &str) -> io::Result<Box<dyn StoreReader>> {
        if name == self.name {
            Ok(Box::new(UnreadableFile(self.len)))
        } else {
            Err(io::ErrorKind::NotFound.into())
        }
    }
    fn create(&mut self, _: &str) -> io::Result<Box<dyn StoreWriter>> {
        unreachable!()
    }
    fn delete(&mut self, _: &str) -> io::Result<()> {
        panic!("must not delete an unread manifest")
    }
    fn write_slot(&mut self, _: &str, _: &[u8]) -> io::Result<()> {
        unreachable!()
    }
}

#[test]
fn oversized_files_are_rejected_without_payload_reads() {
    use super::format::*;
    for name in MANIFEST_SLOTS {
        assert!(FrontierStore::open(
            MetadataBackend {
                name,
                len: MAX_MANIFEST_FILE + 1
            },
            config()
        )
        .is_err());
    }
    let (mut store, _) = FrontierStore::open(
        MetadataBackend {
            name: ATTEMPTS_FILE,
            len: ATTEMPTS_LEN + 1,
        },
        config(),
    )
    .unwrap();
    assert_eq!(store.attempts().unwrap(), None);
    for (name, limit) in [
        ("ledger-x.full", MAX_LEDGER_FILE),
        ("ledger-x.diff", MAX_LEDGER_FILE),
        ("staged-x.diff", MAX_LEDGER_FILE),
        ("parts-x.bin", MAX_PARTS_FILE),
        ("block-x.bin", MAX_BLOCK_FILE),
    ] {
        let (mut store, _) = FrontierStore::open(
            MetadataBackend {
                name,
                len: limit + 1,
            },
            config(),
        )
        .unwrap();
        for stated_len in [limit, limit + 1, u64::MAX] {
            let file = FileRef::new(name, stated_len, 0);
            assert!(matches!(
                store.read_blob(&file),
                Err(PersistError::Corrupt { .. })
            ));
            assert!(matches!(
                store.verify_file(&file),
                Err(PersistError::Corrupt { .. })
            ));
        }
    }
}

#[test]
fn ledger_depth_boundary_is_checked_at_every_entry_point() {
    use super::{
        diff::DiffApplier,
        ledger_codec::{verify_full, verify_paths, MAX_DEPTH},
    };
    // Source of the boundary is Mesa's constraint profile, not codec output.
    assert_eq!(MAX_DEPTH, 35);
    assert_eq!(
        mina_core::network::devnet::CONSTRAINT_CONSTANTS.ledger_depth,
        35
    );
    let base = Mask::create(35);
    let bytes = full_bytes(&base);
    let mut header = read_header(&mut bytes.as_slice()).unwrap();
    assert!(read_full(&mut &[][..], &header, VerifyMode::RootOnly).is_ok());
    let mut diff = merkle_diff(&base, &base).unwrap();
    assert!(apply_diff(&base, &diff).is_ok());
    assert!(read_bytes(&diff_bytes(&diff), 1, Some(&base), VerifyMode::RootOnly).is_ok());
    for depth in [0, 36, 40, u32::MAX] {
        header.depth = depth;
        let mut wire = LEDGER_MAGIC.to_vec();
        wire.extend(FORMAT_VERSION.to_le_bytes());
        header.binprot_write(&mut wire).unwrap();
        assert!(read_header(&mut wire.as_slice()).is_err());
        assert!(read_full(&mut UnreadableFile(0), &header, VerifyMode::RootOnly).is_err());
        header.kind = LedgerFileKind::Diff {
            base: hash_of(&base),
        };
        assert!(read_diff(&mut UnreadableFile(0), &header, &base, VerifyMode::RootOnly).is_err());
        header.kind = LedgerFileKind::Full;
    }
    let mut too_deep = Mask::create(36);
    assert!(write_full(&mut Vec::new(), &too_deep).is_err());
    assert!(merkle_diff(&too_deep, &too_deep).is_err());
    assert!(DiffApplier::new(&too_deep, false, 0).is_err());
    assert!(verify_full(&mut too_deep).is_err());
    assert!(verify_paths(&mut too_deep, 36, 0, &[]).is_err());
    diff.depth = 36;
    assert!(write_diff(&mut Vec::new(), &diff).is_err());
    assert!(apply_diff(&base, &diff).is_err());
    diff.depth = 34;
    assert!(apply_diff(&base, &diff).is_err());
}

#[test]
fn ledger_counts_are_bounded_without_reading_entries() {
    use super::diff::DiffApplier;
    let base = Mask::create(35);
    let bytes = full_bytes(&base);
    let header = read_header(&mut bytes.as_slice()).unwrap();
    for (accounts, entries, hashes) in [
        ((1u64 << 35) + 1, 0, 0),
        (0, 1, 0),
        (0, 0, 1),
        (1u64 << 35, 0, 1u64 << 36),
    ] {
        let bad = LedgerFileHeader {
            num_accounts: accounts,
            num_entries: entries,
            num_hashes: hashes,
            ..header.clone()
        };
        assert!(read_full(&mut UnreadableFile(0), &bad, VerifyMode::RootOnly).is_err());
        let bad = LedgerFileHeader {
            kind: LedgerFileKind::Diff {
                base: hash_of(&base),
            },
            ..bad
        };
        assert!(read_diff(&mut UnreadableFile(0), &bad, &base, VerifyMode::RootOnly).is_err());
    }
    assert!(DiffApplier::new(&base, false, (1u64 << 35) + 1).is_err());
    // An in-range, enormous account count does not reserve storage: it only
    // tries to decode the first account, and EOF is an ordinary error.
    let full = LedgerFileHeader {
        num_accounts: 1u64 << 35,
        num_entries: 1u64 << 35,
        num_hashes: (1u64 << 36) - 1,
        ..header
    };
    assert!(matches!(
        read_full(&mut &[][..], &full, VerifyMode::RootOnly),
        Err(CodecError::Decode(_))
    ));
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn native_sparse_oversized_files_are_rejected() {
    use super::format::*;
    let dir = std::env::temp_dir().join(format!(
        "mina-i5a-size-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let (mut store, _) = FrontierStore::open(FsBackend::new(&dir).unwrap(), config()).unwrap();
    for (name, limit) in [
        ("ledger-x.full", MAX_LEDGER_FILE),
        ("ledger-x.diff", MAX_LEDGER_FILE),
        ("staged-x.diff", MAX_LEDGER_FILE),
        ("parts-x.bin", MAX_PARTS_FILE),
        ("block-x.bin", MAX_BLOCK_FILE),
    ] {
        std::fs::File::create(dir.join(name))
            .unwrap()
            .set_len(limit + 1)
            .unwrap();
        assert!(store.read_blob(&FileRef::new(name, limit + 1, 0)).is_err());
    }
    std::fs::File::create(dir.join(ATTEMPTS_FILE))
        .unwrap()
        .set_len(ATTEMPTS_LEN + 1)
        .unwrap();
    assert_eq!(store.attempts().unwrap(), None);
    drop(store);
    for slot in MANIFEST_SLOTS {
        std::fs::File::create(dir.join(slot))
            .unwrap()
            .set_len(MAX_MANIFEST_FILE + 1)
            .unwrap();
        assert!(FrontierStore::open(FsBackend::new(&dir).unwrap(), config()).is_err());
        std::fs::remove_file(dir.join(slot)).unwrap();
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn account_token_symbol_length_is_checked_before_payload() {
    use mina_p2p_messages::binprot::Nat0;
    let base = random_ledger(1);
    let bytes = full_bytes(&base);
    let mut input = bytes.as_slice();
    let header = read_header(&mut input).unwrap();
    let account = v2::MinaBaseAccountBinableArgStableV2::binprot_read(&mut input).unwrap();
    for count in [7, u64::MAX] {
        let mut body = Vec::new();
        account.public_key.binprot_write(&mut body).unwrap();
        account.token_id.binprot_write(&mut body).unwrap();
        Nat0(count).binprot_write(&mut body).unwrap();
        assert!(matches!(
            read_full(&mut body.as_slice(), &header, VerifyMode::RootOnly),
            Err(CodecError::Decode(_))
        ));
    }
}

#[path = "writer_tests.rs"]
mod writer_tests;

#[path = "load_tests.rs"]
mod load_tests;

#[path = "proxy_tests.rs"]
mod proxy_tests;
