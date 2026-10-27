//! Ledger files (R20 D2): a header, then a body read in chunks.
//!
//! ```text
//! LEDGER_MAGIC (8 bytes) | FORMAT_VERSION (u32 LE) | LedgerFileHeader (binprot)
//! Full body: num_entries × account (binprot MinaBaseAccountBinableArgStableV2)
//!            in index order, then num_hashes × (u64 linear index, LedgerHash):
//!            the hash of every non-empty node, by linear index.
//! Diff body: num_entries × (u64 index, account) by index, then the hashes of
//!            every node on the changed paths, by linear index.
//! ```
//!
//! Loading never hashes the ledger: an account hash costs about 1 ms in the
//! browser, so 290,000 of them would take minutes. The hashes are read from the
//! file, and the ledger is checked by its root against the hash it is stored
//! under and by hashing a sample of leaf paths ([`VerifyMode`]). Accounts are
//! decoded one at a time, so a whole file is never in memory.

use std::io::{Read, Write};

use ledger::{
    Account, AccountIndex, Address, BaseLedger, Database, GetOrCreated, Mask, TreeVersion, V2,
};
use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    binprot::{
        self,
        macros::{BinProtRead, BinProtWrite},
        BinProtRead, BinProtWrite,
    },
    v2,
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use rayon::prelude::*;
use redux::Instant;

use super::{
    diff::{empty_hash, first_leaf, DiffApplier, LedgerDiff},
    format::FORMAT_VERSION,
    load_control::LoadControl,
};

pub const LEDGER_MAGIC: [u8; 8] = *b"MINALDG\0";

/// Leaf paths hashed when a ledger is loaded: about 2,300 permutations, 0.2 s
/// in the browser at R8's 94 µs each.
pub const SAMPLE_PATHS: usize = 64;

/// Mesa's mainnet and devnet constraint constants both specify depth 35:
/// `mina_core::network::{mainnet,devnet}::CONSTRAINT_CONSTANTS.ledger_depth`.
/// Smaller depths remain useful for test ledgers; empty hashes stop here.
pub const MAX_DEPTH: u32 = mina_core::network::mainnet::CONSTRAINT_CONSTANTS.ledger_depth as u32;

pub(crate) fn check_depth(depth: u32) -> Result<(), CodecError> {
    if depth == 0 || depth > MAX_DEPTH {
        return Err(CodecError::Invalid(format!("depth {depth}")));
    }
    Ok(())
}

/// Hashes handed to the ledger at a time when loading a full file.
const HASH_CHUNK: usize = 1 << 16;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("the frontier store load was cancelled")]
    Cancelled,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("decoding: {0}")]
    Decode(String),
    #[error("not a ledger file")]
    BadMagic,
    #[error("format version {0}, ours is {FORMAT_VERSION}")]
    Version(u32),
    #[error("invalid ledger file: {0}")]
    Invalid(String),
    #[error("the diff's base is not the ledger it was written over")]
    WrongBase,
    #[error("the ledger's root is not the hash it was stored under")]
    RootMismatch,
    #[error("the stored hash of node {0} is wrong")]
    HashMismatch(u64),
    #[error("the hash of node {0} is missing")]
    MissingHash(u64),
    #[error("account {0} is missing: the ledger has a gap")]
    NotDense(u64),
    /// What a streamed ledger is read from has gone for good (the ledger
    /// thread stopped): persistence is turned off rather than retried.
    #[error("the ledger being streamed is gone: {0}")]
    SourceGone(String),
}

impl From<binprot::Error> for CodecError {
    fn from(error: binprot::Error) -> Self {
        Self::Decode(error.to_string())
    }
}

impl From<mina_p2p_messages::bigint::InvalidBigInt> for CodecError {
    fn from(_: mina_p2p_messages::bigint::InvalidBigInt) -> Self {
        Self::Decode("a hash is not a field element".into())
    }
}

#[derive(Debug, Clone, PartialEq, BinProtRead, BinProtWrite)]
pub enum LedgerFileKind {
    Full,
    Diff { base: v2::LedgerHash },
}

/// What follows the magic and the format version.
#[derive(Debug, Clone, PartialEq, BinProtRead, BinProtWrite)]
pub struct LedgerFileHeader {
    pub kind: LedgerFileKind,
    /// What the hash matrix's root must equal.
    pub ledger_hash: v2::LedgerHash,
    pub depth: u32,
    pub num_accounts: u64,
    /// Accounts in the body.
    pub num_entries: u64,
    pub num_hashes: u64,
    /// Whether the ledger keeps a token owner index, as genesis ledgers do;
    /// ledgers synced from peers don't.
    pub token_owners: bool,
}

/// How much of a loaded ledger is hashed to check the stored hashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    /// Only the root against the hash the ledger is stored under.
    RootOnly,
    /// The root, and this many leaf paths: random ones for a full file, the
    /// changed ones for a diff. Each costs an account hash and a hash per
    /// level.
    Sample(usize),
    /// Every account and node, as the offline checker does
    /// (`PERSIST_VERIFY=full`) and a restore must (I5d1): no stored hash is
    /// trusted. For a diff, the accounts it sets and every node above them
    /// ([`verify_changed`]); its other nodes are its base's, which a full load
    /// verifies first.
    Full,
}

impl Default for VerifyMode {
    fn default() -> Self {
        Self::Sample(SAMPLE_PATHS)
    }
}

/// Non-empty nodes at `level` of a ledger of `depth` with `num_accounts`
/// accounts, all at the start.
fn nodes_at_level(num_accounts: u64, depth: u32, level: u32) -> u64 {
    if num_accounts == 0 {
        return 0;
    }
    ((num_accounts - 1) >> (depth - level)) + 1
}

/// How many hashes a full file holds: one per non-empty node.
pub fn count_nonempty_nodes(num_accounts: u64, depth: u32) -> u64 {
    (0..=depth)
        .map(|level| nodes_at_level(num_accounts, depth, level))
        .sum()
}

fn node(level: u32, index: u64) -> Address {
    Address::from_index(AccountIndex(index), level as usize)
}

fn linear(level: u32, index: u64) -> u64 {
    (1u64 << level) - 1 + index
}

fn write_prefix<W: Write>(w: &mut W, header: &LedgerFileHeader) -> std::io::Result<()> {
    w.write_all(&LEDGER_MAGIC)?;
    w.write_all(&FORMAT_VERSION.to_le_bytes())?;
    header.binprot_write(w)
}

pub fn read_header<R: Read>(r: &mut R) -> Result<LedgerFileHeader, CodecError> {
    let mut prefix = [0u8; 12];
    r.read_exact(&mut prefix)?;
    if prefix[..8] != LEDGER_MAGIC {
        return Err(CodecError::BadMagic);
    }
    let version = u32::from_le_bytes(prefix[8..].try_into().unwrap());
    if version != FORMAT_VERSION {
        return Err(CodecError::Version(version));
    }
    let header = LedgerFileHeader::binprot_read(r)?;
    validate_header(&header)?;
    Ok(header)
}

fn validate_header(header: &LedgerFileHeader) -> Result<(), CodecError> {
    check_depth(header.depth)?;
    if header.num_accounts > 1u64 << header.depth
        || header.num_entries > header.num_accounts
        || header.num_hashes > count_nonempty_nodes(header.num_accounts, header.depth)
    {
        return Err(CodecError::Invalid("counts out of range".into()));
    }
    Ok(())
}

/// Writes a whole ledger: its accounts, which must have no gap, and the
/// hashes of its non-empty nodes, which it computes if needed.
pub fn write_full<W: Write>(w: &mut W, ledger: &Mask) -> Result<LedgerFileHeader, CodecError> {
    let mut ledger = ledger.clone();
    let depth = ledger.depth() as u32;
    check_depth(depth)?;
    let root = ledger.merkle_root();
    let num_accounts = ledger.num_accounts() as u64;
    let header = LedgerFileHeader {
        kind: LedgerFileKind::Full,
        ledger_hash: v2::LedgerHash::from_fp(root),
        depth,
        num_accounts,
        num_entries: num_accounts,
        num_hashes: count_nonempty_nodes(num_accounts, depth),
        token_owners: ledger.has_token_owners(),
    };
    validate_header(&header)?;
    write_prefix(w, &header)?;

    let mut written = 0u64;
    let mut error = None;
    ledger.iter(|account| {
        if error.is_none() {
            match v2::MinaBaseAccountBinableArgStableV2::from(account).binprot_write(w) {
                Ok(()) => written += 1,
                Err(e) => error = Some(e),
            }
        }
    });
    if let Some(error) = error {
        return Err(error.into());
    }
    if written != num_accounts {
        return Err(CodecError::NotDense(written));
    }

    for level in 0..=depth {
        for index in 0..nodes_at_level(num_accounts, depth, level) {
            let hash = ledger
                .get_inner_hash_at_addr(node(level, index))
                .map_err(CodecError::Invalid)?;
            linear(level, index).binprot_write(w)?;
            v2::LedgerHash::from_fp(hash).binprot_write(w)?;
        }
    }
    Ok(header)
}

/// The header of a full file of `ledger`: a full file's counts follow from
/// its account count, so it is known before any of its body is written.
pub fn full_header(ledger: &Mask) -> Result<LedgerFileHeader, CodecError> {
    let mut ledger = ledger.clone();
    let depth = ledger.depth() as u32;
    check_depth(depth)?;
    let root = ledger.merkle_root();
    let num_accounts = ledger.num_accounts() as u64;
    let header = LedgerFileHeader {
        kind: LedgerFileKind::Full,
        ledger_hash: v2::LedgerHash::from_fp(root),
        depth,
        num_accounts,
        num_entries: num_accounts,
        num_hashes: count_nonempty_nodes(num_accounts, depth),
        token_owners: ledger.has_token_owners(),
    };
    validate_header(&header)?;
    Ok(header)
}

/// A piece of a full file's body, so that it can be encoded a piece at a time
/// by the thread that owns the ledger (see [`FullLedgerSource`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FullPart {
    /// The accounts at indices `start..end`.
    Accounts { start: u64, end: u64 },
    /// The hashes `start..end`, counted in the file's order: from the root
    /// down, level by level, and by index within a level.
    Hashes { start: u64, end: u64 },
}

/// Where a streamed full file's body comes from: pieces of a ledger that
/// another thread owns.
///
/// A ledger is never read from two threads: `Mask` takes its lock with
/// `try_lock().expect(..)`, so a read on the store thread while the ledger
/// thread touches the same mask, or any of its ancestors, panics one of them.
/// The ledger thread encodes each piece ([`encode_full_part`]) and the store
/// thread only writes bytes.
pub trait FullLedgerSource: Send + Sync + std::fmt::Debug {
    /// The file's header, as [`full_header`] gives it.
    fn header(&self) -> &LedgerFileHeader;
    /// The bytes of a piece, as [`encode_full_part`] gives them.
    fn read_part(&self, part: FullPart) -> Result<Vec<u8>, CodecError>;
    /// The accounts and the hashes asked for per piece.
    fn part_sizes(&self) -> (u64, u64) {
        (STREAM_ACCOUNTS, STREAM_HASHES)
    }
}

/// Accounts and hashes per piece of a streamed full file: about 3.5 MB and
/// 2.7 MB on mainnet, each encoded in tens of milliseconds.
pub const STREAM_ACCOUNTS: u64 = 1 << 14;
pub const STREAM_HASHES: u64 = 1 << 16;

/// Appends the bytes of a piece of `ledger`'s full file to `out`: the same
/// bytes [`write_full`] writes for those accounts or hashes. The ledger must
/// still be the one `header` was taken from.
pub fn encode_full_part(
    ledger: &Mask,
    header: &LedgerFileHeader,
    part: FullPart,
    out: &mut Vec<u8>,
) -> Result<(), CodecError> {
    let mut ledger = ledger.clone();
    if header.kind != LedgerFileKind::Full
        || ledger.num_accounts() as u64 != header.num_accounts
        || v2::LedgerHash::from_fp(ledger.merkle_root()) != header.ledger_hash
    {
        return Err(CodecError::Invalid(
            "the ledger changed while it was being written".into(),
        ));
    }
    let depth = header.depth;
    match part {
        FullPart::Accounts { start, end } => {
            if start > end || end > header.num_accounts {
                return Err(CodecError::Invalid("accounts out of range".into()));
            }
            for index in start..end {
                let account = ledger
                    .get_at_index(AccountIndex(index))
                    .ok_or(CodecError::NotDense(index))?;
                v2::MinaBaseAccountBinableArgStableV2::from(account.as_ref()).binprot_write(out)?;
            }
        }
        FullPart::Hashes { start, end } => {
            if start > end || end > header.num_hashes {
                return Err(CodecError::Invalid("hashes out of range".into()));
            }
            // The level the first hash is on, and its index there.
            let mut level = 0;
            let mut skipped = 0;
            while level <= depth {
                let here = nodes_at_level(header.num_accounts, depth, level);
                if start < skipped + here {
                    break;
                }
                skipped += here;
                level += 1;
            }
            let mut index = start - skipped;
            for _ in start..end {
                while index >= nodes_at_level(header.num_accounts, depth, level) {
                    level += 1;
                    index = 0;
                }
                let hash = ledger
                    .get_inner_hash_at_addr(node(level, index))
                    .map_err(CodecError::Invalid)?;
                linear(level, index).binprot_write(out)?;
                v2::LedgerHash::from_fp(hash).binprot_write(out)?;
                index += 1;
            }
        }
    }
    Ok(())
}

/// Writes a full file from a [`FullLedgerSource`], a piece at a time: the
/// same bytes as [`write_full`] of the source's ledger.
pub fn write_full_from<W: Write>(
    w: &mut W,
    source: &dyn FullLedgerSource,
) -> Result<LedgerFileHeader, CodecError> {
    let header = source.header().clone();
    validate_header(&header)?;
    if header.kind != LedgerFileKind::Full
        || header.num_entries != header.num_accounts
        || header.num_hashes != count_nonempty_nodes(header.num_accounts, header.depth)
    {
        return Err(CodecError::Invalid("not a full file's header".into()));
    }
    let (accounts_per_part, hashes_per_part) = source.part_sizes();
    if accounts_per_part == 0 || hashes_per_part == 0 {
        return Err(CodecError::Invalid("empty pieces".into()));
    }
    write_prefix(w, &header)?;
    let mut start = 0;
    while start < header.num_accounts {
        let end = (start + accounts_per_part).min(header.num_accounts);
        w.write_all(&source.read_part(FullPart::Accounts { start, end })?)?;
        start = end;
    }
    let mut start = 0;
    while start < header.num_hashes {
        let end = (start + hashes_per_part).min(header.num_hashes);
        w.write_all(&source.read_part(FullPart::Hashes { start, end })?)?;
        start = end;
    }
    Ok(header)
}

/// Writes a ledger as a diff over another one.
pub fn write_diff<W: Write>(w: &mut W, diff: &LedgerDiff) -> Result<LedgerFileHeader, CodecError> {
    let header = LedgerFileHeader {
        kind: LedgerFileKind::Diff {
            base: v2::LedgerHash::from_fp(diff.base),
        },
        ledger_hash: v2::LedgerHash::from_fp(diff.target),
        depth: diff.depth as u32,
        num_accounts: diff.num_accounts,
        num_entries: diff.accounts.len() as u64,
        num_hashes: diff.hashes.len() as u64,
        token_owners: diff.token_owners,
    };
    validate_header(&header)?;
    write_prefix(w, &header)?;
    for (index, account) in &diff.accounts {
        index.0.binprot_write(w)?;
        v2::MinaBaseAccountBinableArgStableV2::from(account.as_ref()).binprot_write(w)?;
    }
    for (linear, hash) in &diff.hashes {
        linear.binprot_write(w)?;
        v2::LedgerHash::from_fp(*hash).binprot_write(w)?;
    }
    Ok(header)
}

fn read_account<R: Read>(r: &mut R) -> Result<Account, CodecError> {
    // The general wire account uses ByteString (100 MB) for token_symbol.
    // A persisted account uses Mina's six-byte token-symbol bound instead.
    // The remaining collections are fixed PaddedSeqs or ZkAppUri (255 bytes).
    use mina_p2p_messages::string::{BoundedByteString, TOKEN_SYMBOL_MAX_LENGTH};
    let account = v2::MinaBaseAccountBinableArgStableV2 {
        public_key: BinProtRead::binprot_read(r)?,
        token_id: BinProtRead::binprot_read(r)?,
        token_symbol: BoundedByteString::<TOKEN_SYMBOL_MAX_LENGTH>::binprot_read(r)?
            .0
            .into(),
        balance: BinProtRead::binprot_read(r)?,
        nonce: BinProtRead::binprot_read(r)?,
        receipt_chain_hash: BinProtRead::binprot_read(r)?,
        delegate: BinProtRead::binprot_read(r)?,
        voting_for: BinProtRead::binprot_read(r)?,
        timing: BinProtRead::binprot_read(r)?,
        permissions: BinProtRead::binprot_read(r)?,
        zkapp: BinProtRead::binprot_read(r)?,
    };
    Ok((&account).try_into()?)
}

fn read_hash<R: Read>(r: &mut R) -> Result<(u64, Fp), CodecError> {
    let linear = u64::binprot_read(r)?;
    let hash = v2::LedgerHash::binprot_read(r)?.to_field::<Fp>()?;
    Ok((linear, hash))
}

/// Reads the body of a full file, given its header.
pub fn read_full<R: Read>(
    r: &mut R,
    header: &LedgerFileHeader,
    verify: VerifyMode,
) -> Result<Mask, CodecError> {
    read_full_timed(r, header, verify, &mut 0.0, &LoadControl::default())
}

pub(crate) fn read_full_timed<R: Read>(
    r: &mut R,
    header: &LedgerFileHeader,
    verify: VerifyMode,
    verify_ms: &mut f64,
    control: &LoadControl,
) -> Result<Mask, CodecError> {
    control.check()?;
    validate_header(header)?;
    if header.kind != LedgerFileKind::Full {
        return Err(CodecError::Invalid("not a full ledger file".into()));
    }
    let depth = header.depth;
    let num_accounts = header.num_accounts;
    if header.num_entries != num_accounts
        || header.num_hashes != count_nonempty_nodes(num_accounts, depth)
    {
        return Err(CodecError::Invalid("counts don't match".into()));
    }
    let database = if header.token_owners {
        Database::create_with_token_owners(depth as u8)
    } else {
        Database::create(depth as u8)
    };
    let mut ledger = Mask::new_root(database);
    #[cfg(test)]
    control.observe_mask(&ledger);

    for index in 0..num_accounts {
        control.check()?;
        let account = read_account(r)?;
        if index % 256 == 255 {
            control.completed(256)?;
        }
        match ledger.get_or_create_account(account.id(), account) {
            Ok(GetOrCreated::Added(addr)) if addr.to_index().0 == index => {}
            _ => {
                return Err(CodecError::Invalid(format!(
                    "account {index} is a duplicate"
                )))
            }
        }
    }

    control.completed(num_accounts.max(1))?;

    // Exactly the non-empty nodes, in order, so that no hash is set twice.
    let mut expected = (0..=depth).flat_map(|level| {
        (0..nodes_at_level(num_accounts, depth, level)).map(move |index| linear(level, index))
    });
    let mut chunk = Vec::with_capacity(HASH_CHUNK.min(header.num_hashes as usize));
    for item in 0..header.num_hashes {
        if item % 256 == 255 {
            control.completed(256)?;
        }
        control.check()?;
        let (linear, hash) = read_hash(r)?;
        if expected.next() != Some(linear) {
            return Err(CodecError::Invalid(format!(
                "unexpected hash index {linear}"
            )));
        }
        chunk.push((linear, hash));
        if chunk.len() == HASH_CHUNK {
            ledger.set_raw_inner_hashes(std::mem::take(&mut chunk));
        }
    }
    if !chunk.is_empty() {
        ledger.set_raw_inner_hashes(chunk);
    }

    check_root(&mut ledger, header)?;
    match verify {
        VerifyMode::RootOnly => {}
        VerifyMode::Sample(paths) => {
            // Every leaf of a ledger no larger than the sample. The seed comes
            // from the file, so a file that loads once loads every time.
            let leaves = if num_accounts <= paths as u64 {
                (0..num_accounts).collect::<Vec<_>>()
            } else {
                let mut rng = StdRng::seed_from_u64(num_accounts ^ header.num_hashes);
                (0..paths).map(|_| rng.gen_range(0..num_accounts)).collect()
            };
            verify_paths(&mut ledger, depth, num_accounts, &leaves)?;
        }
        VerifyMode::Full => {
            let started = Instant::now();
            let result = verify_full_controlled(&mut ledger, control);
            *verify_ms += started.elapsed().as_secs_f64() * 1000.0;
            result?;
        }
    }
    Ok(ledger)
}

/// Reads the body of a diff file over `base`, given its header. The result is
/// a new root ledger; `base` is left as it is.
pub fn read_diff<R: Read>(
    r: &mut R,
    header: &LedgerFileHeader,
    base: &Mask,
    verify: VerifyMode,
) -> Result<Mask, CodecError> {
    let copy = diff_base_copy(header, base)?;
    read_diff_onto(r, header, copy, verify)
}

/// As [`read_diff`], but the result is a child mask of `base` holding only
/// the diff's accounts and hashes (the root staged ledger over the root
/// snarked ledger, I5d5), not a copy of `base`. `base` keeps the mask as its
/// child, and must have all its hashes checked: the verification reads the
/// hashes off the changed paths from it.
pub fn read_diff_as_child<R: Read>(
    r: &mut R,
    header: &LedgerFileHeader,
    base: &Mask,
    verify: VerifyMode,
) -> Result<Mask, CodecError> {
    let child = diff_base_child(header, base)?;
    read_diff_onto(r, header, child, verify)
}

/// Checks that `base` is the base of the diff `header` describes, and copies
/// it, for [`read_diff_onto`].
pub fn diff_base_copy(header: &LedgerFileHeader, base: &Mask) -> Result<Mask, CodecError> {
    check_diff_base(header, base)?;
    // A root mask's copy is a copy of its database.
    Ok(base.copy())
}

/// As [`diff_base_copy`], but a child mask of `base`, which `base` keeps as
/// its child (I5d5).
pub fn diff_base_child(header: &LedgerFileHeader, base: &Mask) -> Result<Mask, CodecError> {
    check_diff_base(header, base)?;
    Ok(base.make_child())
}

fn check_diff_base(header: &LedgerFileHeader, base: &Mask) -> Result<(), CodecError> {
    validate_header(header)?;
    let LedgerFileKind::Diff { base: base_hash } = &header.kind else {
        return Err(CodecError::Invalid("not a diff file".into()));
    };
    if base.depth() as u32 != header.depth {
        return Err(CodecError::Invalid("the base has another depth".into()));
    }
    if base.clone().merkle_root() != base_hash.to_field::<Fp>()? {
        return Err(CodecError::WrongBase);
    }
    DiffApplier::check_base(base, header.token_owners, header.num_accounts)
}

/// As [`read_diff`], over `base_ledger`, the copy or child mask of its base
/// that [`diff_base_copy`] or [`diff_base_child`] made, which becomes the
/// result.
pub fn read_diff_onto<R: Read>(
    r: &mut R,
    header: &LedgerFileHeader,
    base_ledger: Mask,
    verify: VerifyMode,
) -> Result<Mask, CodecError> {
    read_diff_onto_timed(
        r,
        header,
        base_ledger,
        verify,
        &mut 0.0,
        &LoadControl::default(),
    )
}

pub(crate) fn read_diff_onto_timed<R: Read>(
    r: &mut R,
    header: &LedgerFileHeader,
    base_ledger: Mask,
    verify: VerifyMode,
    verify_ms: &mut f64,
    control: &LoadControl,
) -> Result<Mask, CodecError> {
    control.check()?;
    let mut applier = DiffApplier::over_copy(base_ledger, header.num_accounts);
    for item in 0..header.num_entries {
        if item % 256 == 255 {
            control.completed(256)?;
        }
        control.check()?;
        let index = u64::binprot_read(r)?;
        let account = read_account(r)?;
        applier.account(index, Box::new(account))?;
    }
    for item in 0..header.num_hashes {
        if item % 256 == 255 {
            control.completed(256)?;
        }
        control.check()?;
        let (linear, hash) = read_hash(r)?;
        applier.hash(linear, hash)?;
    }
    let (mut ledger, changed) = applier.finish()?;
    if ledger.num_accounts() as u64 != header.num_accounts {
        return Err(CodecError::Invalid("account count doesn't match".into()));
    }

    check_root(&mut ledger, header)?;
    match verify {
        VerifyMode::RootOnly => {}
        VerifyMode::Sample(paths) => {
            let step = changed.len().div_ceil(paths.max(1)).max(1);
            let leaves = changed.iter().step_by(step).copied().collect::<Vec<_>>();
            verify_paths(&mut ledger, header.depth, header.num_accounts, &leaves)?;
        }
        VerifyMode::Full => {
            let started = Instant::now();
            let result = verify_changed_controlled(&mut ledger, &changed, control);
            *verify_ms += started.elapsed().as_secs_f64() * 1000.0;
            result?;
        }
    }
    Ok(ledger)
}

/// Reads a ledger file of either kind; `base` gives a diff's base by hash.
pub fn read_ledger<R: Read>(
    r: &mut R,
    base: impl FnOnce(&v2::LedgerHash) -> Option<Mask>,
    verify: VerifyMode,
) -> Result<(Mask, LedgerFileHeader), CodecError> {
    let header = read_header(r)?;
    let ledger = match &header.kind {
        LedgerFileKind::Full => read_full(r, &header, verify)?,
        LedgerFileKind::Diff { base: hash } => {
            let base = base(hash).ok_or(CodecError::WrongBase)?;
            read_diff(r, &header, &base, verify)?
        }
    };
    Ok((ledger, header))
}

fn check_root(ledger: &mut Mask, header: &LedgerFileHeader) -> Result<(), CodecError> {
    let expected = header.ledger_hash.to_field::<Fp>()?;
    let root = match cached_hash(ledger, &Address::root()) {
        Some(root) => root,
        // Only an empty ledger has no root hash stored.
        None if ledger.num_accounts() == 0 => empty_hash(header.depth as usize),
        None => return Err(CodecError::MissingHash(0)),
    };
    if root != expected {
        return Err(CodecError::RootMismatch);
    }
    Ok(())
}

/// A node's hash as the ledger holds it: its own, or for a mask, the nearest
/// ancestor's. A child mask over a loaded ledger holds only the hashes of the
/// nodes on its own accounts' paths; the others are its parent's (I5d5).
pub(crate) fn cached_hash(ledger: &mut Mask, addr: &Address) -> Option<Fp> {
    if let Some(hash) = ledger.get_cached_hash(addr) {
        return Some(hash);
    }
    let mut parent = ledger.get_parent();
    while let Some(mut ancestor) = parent {
        if let Some(hash) = ancestor.get_cached_hash(addr) {
            return Some(hash);
        }
        parent = ancestor.get_parent();
    }
    None
}

/// A node's stored hash, or the empty hash for a subtree past the last
/// account.
fn stored_hash(
    ledger: &mut Mask,
    depth: u32,
    num_accounts: u64,
    level: u32,
    index: u64,
) -> Result<Fp, CodecError> {
    let addr = node(level, index);
    if first_leaf(&addr, depth as u8) >= num_accounts {
        return Ok(empty_hash((depth - level) as usize));
    }
    cached_hash(ledger, &addr).ok_or(CodecError::MissingHash(linear(level, index)))
}

/// Hashes the leaves `leaves` and their paths to the root, and checks each
/// node against the stored hashes.
pub fn verify_paths(
    ledger: &mut Mask,
    depth: u32,
    num_accounts: u64,
    leaves: &[u64],
) -> Result<(), CodecError> {
    check_depth(depth)?;
    for &leaf in leaves {
        let account = ledger
            .get_at_index(AccountIndex(leaf))
            .ok_or(CodecError::NotDense(leaf))?;
        let mut hash = account.hash();
        let mut index = leaf;
        for level in (0..=depth).rev() {
            if stored_hash(ledger, depth, num_accounts, level, index)? != hash {
                return Err(CodecError::HashMismatch(linear(level, index)));
            }
            if level == 0 {
                break;
            }
            let sibling = stored_hash(ledger, depth, num_accounts, level, index ^ 1)?;
            let (left, right) = if index & 1 == 0 {
                (hash, sibling)
            } else {
                (sibling, hash)
            };
            // The parent's subtree height, less one, as the ledger hashes it.
            hash = V2::hash_node((depth - level) as usize, left, right);
            index >>= 1;
        }
    }
    Ok(())
}

/// Rehashes the accounts at `changed` (increasing indices) and every node
/// above them, a level at a time, each from its children's stored hashes, and
/// checks each against its stored hash. A diff sets accounts and hashes only
/// on the paths of the accounts it changes (`DiffApplier` checks it), so with
/// its base verified in full this verifies the whole ledger, at the cost of
/// the changed paths rather than of every account.
pub fn verify_changed(ledger: &mut Mask, changed: &[u64]) -> Result<(), CodecError> {
    verify_changed_controlled(ledger, changed, &LoadControl::default())
}

fn verify_changed_controlled(
    ledger: &mut Mask,
    changed: &[u64],
    control: &LoadControl,
) -> Result<(), CodecError> {
    control.check()?;
    let depth = ledger.depth() as u32;
    check_depth(depth)?;
    let num_accounts = ledger.num_accounts() as u64;
    if changed.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(CodecError::Invalid("changed accounts out of order".into()));
    }
    let mut hashes = Vec::with_capacity(changed.len());
    for chunk in changed.chunks(1024) {
        control.check()?;
        let accounts = chunk
            .iter()
            .map(|&index| {
                ledger
                    .get_at_index(AccountIndex(index))
                    .ok_or(CodecError::NotDense(index))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let checked = accounts
            .par_iter()
            .map(|account| {
                control.check()?;
                Ok(account.hash())
            })
            .collect::<Result<Vec<_>, CodecError>>()?;
        hashes.extend(checked);
        control.completed(chunk.len() as u64)?;
    }
    let mut level_nodes = changed.iter().copied().zip(hashes).collect::<Vec<_>>();
    for level in (0..=depth).rev() {
        control.check()?;
        for &(index, hash) in &level_nodes {
            if stored_hash(ledger, depth, num_accounts, level, index)? != hash {
                return Err(CodecError::HashMismatch(linear(level, index)));
            }
        }
        if level == 0 {
            break;
        }
        let mut parents = level_nodes
            .iter()
            .map(|(index, _)| index >> 1)
            .collect::<Vec<_>>();
        parents.dedup();
        let children = parents
            .iter()
            .map(|&parent| {
                Ok((
                    stored_hash(ledger, depth, num_accounts, level, parent << 1)?,
                    stored_hash(ledger, depth, num_accounts, level, (parent << 1) | 1)?,
                ))
            })
            .collect::<Result<Vec<_>, CodecError>>()?;
        // The parent's subtree height, less one, as the ledger hashes it.
        let height = (depth - level) as usize;
        let hashes = children
            .par_iter()
            .map(|&(left, right)| {
                control.check()?;
                let hash = V2::hash_node(height, left, right);
                control.completed(1)?;
                Ok(hash)
            })
            .collect::<Result<Vec<_>, CodecError>>()?;
        level_nodes = parents.into_iter().zip(hashes).collect();
    }
    Ok(())
}

/// Hashes every account and node and checks them against the stored hashes.
pub fn verify_full(ledger: &mut Mask) -> Result<(), CodecError> {
    verify_full_controlled(ledger, &LoadControl::default())
}

fn verify_full_controlled(ledger: &mut Mask, control: &LoadControl) -> Result<(), CodecError> {
    control.check()?;
    let depth = ledger.depth() as u32;
    check_depth(depth)?;
    let num_accounts = ledger.num_accounts() as u64;
    // Bound temporary account copies independently of the ledger's size.
    // Only hashes survive a batch. A full-account copy used to coexist with
    // every decoded ledger during the restore peak.
    let mut level_hashes = Vec::with_capacity(num_accounts as usize);
    for start in (0..num_accounts).step_by(1024) {
        control.check()?;
        let accounts = (start..(start + 1024).min(num_accounts))
            .map(|index| {
                ledger
                    .get_at_index(AccountIndex(index))
                    .ok_or(CodecError::NotDense(index))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let hashes = accounts
            .par_iter()
            .map(|account| {
                control.check()?;
                Ok(account.hash())
            })
            .collect::<Result<Vec<_>, CodecError>>()?;
        level_hashes.extend(hashes);
        control.completed(accounts.len() as u64)?;
    }
    for level in (0..=depth).rev() {
        control.check()?;
        for (index, hash) in level_hashes.iter().enumerate() {
            if stored_hash(ledger, depth, num_accounts, level, index as u64)? != *hash {
                return Err(CodecError::HashMismatch(linear(level, index as u64)));
            }
        }
        if level == 0 {
            break;
        }
        let empty = empty_hash((depth - level) as usize);
        let height = (depth - level) as usize;
        level_hashes = level_hashes
            .par_chunks(2)
            .map(|pair| {
                control.check()?;
                let hash = V2::hash_node(height, pair[0], pair.get(1).copied().unwrap_or(empty));
                control.completed(1)?;
                Ok(hash)
            })
            .collect::<Result<Vec<_>, CodecError>>()?;
    }
    Ok(())
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use crate::persist::test_fixtures::{change, ledger_with};

    #[test]
    fn full_rehash_stops_between_bounded_account_batches() {
        let mut ledger = ledger_with(2049);
        let expected = ledger.merkle_root();
        let control = LoadControl::cancelling_after(1024);
        assert!(matches!(
            verify_full_controlled(&mut ledger, &control),
            Err(CodecError::Cancelled)
        ));
        assert_eq!(control.progress(), 1024);
        // All three batches still check every leaf and parent when allowed
        // to finish, including the last single-account batch.
        verify_full(&mut ledger).unwrap();
        assert_eq!(ledger.merkle_root(), expected);
    }

    #[test]
    fn changed_path_rehash_stops_without_altering_the_ledger() {
        let base = ledger_with(40);
        let mut target = base.copy();
        change(&mut target, &[0, 17, 39]);
        let expected = target.merkle_root();
        let control = LoadControl::cancelling_after(1);
        assert!(matches!(
            verify_changed_controlled(&mut target, &[0, 17, 39], &control),
            Err(CodecError::Cancelled)
        ));
        verify_changed(&mut target, &[0, 17, 39]).unwrap();
        assert_eq!(target.merkle_root(), expected);
    }

    #[test]
    fn full_file_decode_stops_before_finishing_its_accounts() {
        let ledger = ledger_with(300);
        let mut bytes = Vec::new();
        write_full(&mut bytes, &ledger).unwrap();
        let mut body = bytes.as_slice();
        let header = read_header(&mut body).unwrap();
        let control = LoadControl::cancelling_after(256);
        assert!(matches!(
            read_full_timed(&mut body, &header, VerifyMode::Full, &mut 0.0, &control),
            Err(CodecError::Cancelled)
        ));
        assert!(!body.is_empty());
        assert_eq!(control.progress(), 256);
        control.assert_observed_masks_dropped();
    }
}
