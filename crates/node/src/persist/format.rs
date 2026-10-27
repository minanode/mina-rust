//! The store's layout (R20 D2): the manifest, file names and the restore
//! attempts file.
//!
//! A store is one directory per chain id. Its files:
//!
//! ```text
//! MANIFEST-A, MANIFEST-B          double-buffered commit point
//! LOCK                            natively, locked for the session
//! ATTEMPTS                        restore attempts: manifest generation, count, CRC32
//! ledger-<ledger_hash>.full       full ledger
//! ledger-<ledger_hash>.diff       ledger as a diff over another stored ledger
//! staged-<state_hash>.diff        root staged ledger as a diff over the root snarked ledger
//! parts-<state_hash>.bin          root StagedLedgerAuxAndPendingCoinbases, binprot
//! block-<state_hash>.bin          one block, binprot MinaBlockBlockStableV2
//! ```
//!
//! Every data file is written once, named by what it holds, and never
//! modified. A file belongs to the store only if a manifest lists it, with its
//! length and CRC32.

use mina_core::ChainId;
use mina_p2p_messages::{
    binprot::{
        self,
        macros::{BinProtRead, BinProtWrite},
        BinProtRead, BinProtWrite, SmallString1k,
    },
    v2,
};

/// Bumped on any change to a file's layout or to what a file means (D6).
/// Which ledgers are kept is not part of it: ledgers are stored by hash.
pub const FORMAT_VERSION: u32 = 1;

/// The first bytes of a manifest slot. The magic and the format version that
/// follows it never move, so any binary can tell whose store it is.
pub const MANIFEST_MAGIC: [u8; 8] = *b"MINAFRS\0";
pub const ATTEMPTS_MAGIC: [u8; 8] = *b"MINAATT\0";

pub const MANIFEST_SLOTS: [&str; 2] = ["MANIFEST-A", "MANIFEST-B"];
pub const LOCK_FILE: &str = "LOCK";
pub const ATTEMPTS_FILE: &str = "ATTEMPTS";

/// The prefixes of data files, the only files garbage collection deletes.
pub const DATA_PREFIXES: [&str; 4] = ["ledger-", "staged-", "parts-", "block-"];

/// A manifest larger than this is not one of ours.
pub const MAX_MANIFEST_BODY: usize = 64 << 20;

pub const MAX_MANIFEST_FILE: u64 = MAX_MANIFEST_BODY as u64 + 20;
pub const ATTEMPTS_LEN: u64 = 28;

/// Resource budget: R20 D5's 4k-slot staleness horizon plus a k+1 frontier.
/// At most one block per slot, with Mesa k=290 (R20 section 1). An extended
/// checkpoint chain must be checkpointed again before exceeding this budget.
/// These are store resource limits, not new consensus constraints.
pub const MAX_CHAIN_ENTRIES: usize = 5 * 290 + 1;
/// One distinct snarked ledger per retained block, plus both epoch ledgers
/// (R20 section 3, LedgersToKeep). This also accommodates I25 retention.
pub const MAX_LEDGERS: usize = MAX_CHAIN_ENTRIES + 2;
pub const MAX_DIFF_CHAIN: usize = 8;
pub const MAX_FILES: usize = MAX_LEDGERS * (MAX_DIFF_CHAIN + 1) + MAX_CHAIN_ENTRIES + 2;

/// Resource ceilings with headroom over R20 D1's measured 85 MB ledgers,
/// 73-77 MB parts and 0.3-0.4 MB blocks. Larger stores need revised limits.
pub const MAX_LEDGER_FILE: u64 = 1 << 30;
pub const MAX_PARTS_FILE: u64 = 256 << 20;
pub const MAX_BLOCK_FILE: u64 = 16 << 20;

pub fn data_file_limit(name: &str) -> Option<u64> {
    if (name.starts_with("ledger-") && (name.ends_with(".full") || name.ends_with(".diff")))
        || (name.starts_with("staged-") && name.ends_with(".diff"))
    {
        Some(MAX_LEDGER_FILE)
    } else if name.starts_with("parts-") && name.ends_with(".bin") {
        Some(MAX_PARTS_FILE)
    } else if name.starts_with("block-") && name.ends_with(".bin") {
        Some(MAX_BLOCK_FILE)
    } else {
        None
    }
}

/// The commit point: what belongs together. Stored as
///
/// ```text
/// MANIFEST_MAGIC (8 bytes) | FORMAT_VERSION (u32 LE) | body length (u32 LE)
///     | body (binprot Manifest) | CRC32 of all the bytes before (u32 LE)
/// ```
///
/// in the slot not holding the newest generation.
#[derive(Debug, Clone, PartialEq, BinProtWrite)]
pub struct Manifest {
    /// One more on every write; the higher valid slot wins.
    pub generation: u64,
    pub chain_id: ChainId,
    pub genesis_state_hash: v2::StateHash,
    /// For diagnostics, and for the browser's age-out of other chains' stores.
    pub written_at_ms: u64,
    /// For diagnostics only.
    pub node_commit: SmallString1k,
    /// Stored snarked ledgers, with or without a checkpoint. Garbage collection
    /// keeps every ledger listed here; the next checkpoint's list replaces it.
    pub ledgers: Vec<v2::LedgerHash>,
    pub checkpoint: Option<Checkpoint>,
    /// Best chain from `checkpoint.root` to the tip, oldest first. Without a
    /// checkpoint, the chain last put, so that the next checkpoint can reuse
    /// its blocks.
    pub chain: Vec<ChainEntry>,
    /// Every file the above references, with the bases of the ledger diffs.
    pub files: Vec<FileRef>,
}

#[derive(
    Debug, Clone, PartialEq, BinProtRead, BinProtWrite, serde::Serialize, serde::Deserialize,
)]
pub struct Checkpoint {
    pub root: v2::StateHash,
    pub root_height: u32,
    pub root_snarked_ledger: v2::LedgerHash,
    pub root_staged_ledger_hash: v2::MinaBaseStagedLedgerHashStableV1,
}

#[derive(
    Debug, Clone, PartialEq, BinProtRead, BinProtWrite, serde::Serialize, serde::Deserialize,
)]
pub struct ChainEntry {
    pub hash: v2::StateHash,
    pub height: u32,
    /// For the load's staleness check (D5).
    pub global_slot: u32,
    /// So that phase 2 can restore an `AppliedBlock` without replaying it.
    pub just_emitted_a_proof: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, BinProtRead, BinProtWrite)]
pub struct FileRef {
    pub name: SmallString1k,
    pub len: u64,
    pub crc32: u32,
}

impl FileRef {
    pub fn new(name: impl Into<String>, len: u64, crc32: u32) -> Self {
        Self {
            name: SmallString1k(name.into()),
            len,
            crc32,
        }
    }

    pub fn name(&self) -> &str {
        &self.name.0
    }
}

impl Manifest {
    // Do not derive BinProtRead: Vec's reader reserves the untrusted length.
    // Decode from a slice so every length is bounded by the actual body too.
    fn read_body(body: &mut &[u8]) -> Result<Self, binprot::Error> {
        Ok(Self {
            generation: BinProtRead::binprot_read(body)?,
            chain_id: BinProtRead::binprot_read(body)?,
            genesis_state_hash: BinProtRead::binprot_read(body)?,
            written_at_ms: BinProtRead::binprot_read(body)?,
            node_commit: BinProtRead::binprot_read(body)?,
            ledgers: read_collection(body, MAX_LEDGERS, 32)?,
            checkpoint: BinProtRead::binprot_read(body)?,
            chain: read_collection(body, MAX_CHAIN_ENTRIES, 35)?,
            files: read_collection(body, MAX_FILES, 3)?,
        })
    }

    pub(crate) fn check_limits(&self) -> Result<(), String> {
        if self.ledgers.len() > MAX_LEDGERS
            || self.chain.len() > MAX_CHAIN_ENTRIES
            || self.files.len() > MAX_FILES
        {
            return Err("manifest collection exceeds store limit".into());
        }
        for file in &self.files {
            match data_file_limit(file.name()) {
                Some(limit) if file.len <= limit => {}
                _ => {
                    return Err(format!(
                        "{} exceeds its file limit or has an unknown kind",
                        file.name()
                    ))
                }
            }
        }
        Ok(())
    }

    pub fn file(&self, name: &str) -> Option<&FileRef> {
        self.files.iter().find(|file| file.name() == name)
    }

    pub fn references(&self, name: &str) -> bool {
        self.file(name).is_some()
    }

    /// The file holding a ledger the manifest lists or a diff's base, full or
    /// diff.
    pub fn ledger_file(&self, hash: &v2::LedgerHash) -> Option<&FileRef> {
        self.file(&ledger_full_name(hash))
            .or_else(|| self.file(&ledger_diff_name(hash)))
    }

    /// Whether the ledger counts as stored: listed in `ledgers` and with its
    /// file recorded. A file that merely exists, say one a crash tore, never
    /// counts (D4).
    pub fn has_ledger(&self, hash: &v2::LedgerHash) -> bool {
        self.ledgers.contains(hash) && self.ledger_file(hash).is_some()
    }
}

/// `min_bytes` is a conservative lower bound for one wire entry: a hash is
/// 32 bytes, a chain entry adds two integers and a bool, a file has a string
/// length and two integers. Never reserve based on the advertised count.
fn read_collection<T: BinProtRead>(
    body: &mut &[u8],
    limit: usize,
    min_bytes: usize,
) -> Result<Vec<T>, binprot::Error> {
    let binprot::Nat0(len) = binprot::Nat0::binprot_read(body)?;
    if len > limit as u64 || len > (body.len() / min_bytes) as u64 {
        return Err(binprot::Error::TooMuchData(len));
    }
    let mut entries = Vec::new();
    for _ in 0..len {
        entries.push(T::binprot_read(body)?);
    }
    Ok(entries)
}

/// Why a manifest slot can't be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SlotError {
    #[error("empty or truncated")]
    Truncated,
    #[error("not a manifest")]
    BadMagic,
    #[error("format version {0}, newer than ours")]
    NewerVersion(u32),
    #[error("format version {0}, older than ours")]
    OlderVersion(u32),
    #[error("checksum mismatch")]
    BadCrc,
    #[error("undecodable: {0}")]
    Decode(String),
}

pub fn encode_manifest(manifest: &Manifest) -> Vec<u8> {
    let mut body = Vec::with_capacity(16 << 10);
    manifest
        .binprot_write(&mut body)
        .expect("writing to a Vec doesn't fail");
    let mut bytes = Vec::with_capacity(body.len() + 20);
    bytes.extend_from_slice(&MANIFEST_MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&body);
    let crc = crc32fast::hash(&bytes);
    bytes.extend_from_slice(&crc.to_le_bytes());
    bytes
}

/// Reads the magic and the format version, which every version keeps in the
/// same place.
pub fn manifest_version(bytes: &[u8]) -> Result<u32, SlotError> {
    if bytes.len() < 12 {
        return Err(SlotError::Truncated);
    }
    if bytes[..8] != MANIFEST_MAGIC {
        return Err(SlotError::BadMagic);
    }
    Ok(u32::from_le_bytes(bytes[8..12].try_into().unwrap()))
}

pub fn decode_manifest(bytes: &[u8]) -> Result<Manifest, SlotError> {
    let version = manifest_version(bytes)?;
    if version > FORMAT_VERSION {
        return Err(SlotError::NewerVersion(version));
    }
    if version < FORMAT_VERSION {
        return Err(SlotError::OlderVersion(version));
    }
    if bytes.len() < 20 {
        return Err(SlotError::Truncated);
    }
    let body_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    if body_len > MAX_MANIFEST_BODY || bytes.len() < 16 + body_len + 4 {
        return Err(SlotError::Truncated);
    }
    if bytes.len() > 16 + body_len + 4 {
        return Err(SlotError::Decode("trailing bytes".into()));
    }
    let (covered, crc) = bytes.split_at(16 + body_len);
    if crc32fast::hash(covered) != u32::from_le_bytes(crc.try_into().unwrap()) {
        return Err(SlotError::BadCrc);
    }
    let mut body = &covered[16..];
    let manifest = Manifest::read_body(&mut body).map_err(|e| SlotError::Decode(e.to_string()))?;
    if !body.is_empty() {
        return Err(SlotError::Decode("trailing bytes in the body".into()));
    }
    manifest.check_limits().map_err(SlotError::Decode)?;
    Ok(manifest)
}

/// Restore attempts of one manifest generation (D5): the loader counts an
/// attempt before it starts and a finished replay clears it, so a store that
/// panics the node on every load is given up after a few tries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempts {
    pub generation: u64,
    pub count: u32,
}

impl Attempts {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(ATTEMPTS_LEN as usize);
        bytes.extend_from_slice(&ATTEMPTS_MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&self.generation.to_le_bytes());
        bytes.extend_from_slice(&self.count.to_le_bytes());
        let crc = crc32fast::hash(&bytes);
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes
    }

    /// `None` for anything but an intact attempts record of our version.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() as u64 != ATTEMPTS_LEN || bytes[..8] != ATTEMPTS_MAGIC {
            return None;
        }
        let (covered, crc) = bytes.split_at(24);
        if crc32fast::hash(covered) != u32::from_le_bytes(crc.try_into().ok()?) {
            return None;
        }
        if u32::from_le_bytes(covered[8..12].try_into().ok()?) != FORMAT_VERSION {
            return None;
        }
        Some(Self {
            generation: u64::from_le_bytes(covered[12..20].try_into().ok()?),
            count: u32::from_le_bytes(covered[20..24].try_into().ok()?),
        })
    }
}

pub fn ledger_full_name(hash: &v2::LedgerHash) -> String {
    format!("ledger-{hash}.full")
}

pub fn ledger_diff_name(hash: &v2::LedgerHash) -> String {
    format!("ledger-{hash}.diff")
}

pub fn staged_name(root: &v2::StateHash) -> String {
    format!("staged-{root}.diff")
}

pub fn parts_name(root: &v2::StateHash) -> String {
    format!("parts-{root}.bin")
}

pub fn block_name(hash: &v2::StateHash) -> String {
    format!("block-{hash}.bin")
}

pub fn is_data_file(name: &str) -> bool {
    DATA_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// A name a backend accepts: no path separators, no `.` or `..`.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && name.len() <= 255
}
