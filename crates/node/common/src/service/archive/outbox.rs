//! The archive outbox: the diffs the node has to deliver to its archive
//! process, including a bounded window of acknowledged diffs (R43b.1).
//!
//! OCaml's daemon keeps the diffs in memory and drops one after five failed
//! tries. Its archive is idempotent by state hash and tolerates any order, but
//! acknowledgements precede database commits. Retention and replay offer bounded
//! repair, not proof that the archive stored every block.
//!
//!
//! Layout of the outbox directory:
//!
//! - `LOCK`: held for the session, so two nodes can't share an outbox.
//! - `<seq>-<height>-<state hash>.diff`: one entry, written once to a `.tmp`
//!   name, synced, renamed and the directory synced, like the frontier store's
//!   data files. It holds a header, the payload and a SHA3-256 checksum.
//! - `cursor.json`: the blocks accepted most recently, to tell a block whose
//!   parent was never seen (a gap) from the next block of the chain.
//! - `gaps.jsonl`: every gap, one JSON object per line, appended and synced.
//! - `corrupt/`: entries that failed their checks, kept for inspection.
//!
//! `delivered/` retains acknowledged entries until they are 870 heights below
//! the highest accepted block (configurable). Startup and operator replay move
//! them back to pending. Both areas count toward the same capacity limits.

use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};

const MAGIC: &[u8; 8] = b"MINAAOB\0";
const FORMAT_VERSION: u32 = 1;
const CHECKSUM_LEN: usize = 32;
const ENTRY_SUFFIX: &str = ".diff";
const TMP_SUFFIX: &str = ".tmp";
const CURSOR_FILE: &str = "cursor.json";
const GAPS_FILE: &str = "gaps.jsonl";
const CORRUPT_DIR: &str = "corrupt";
const DELIVERED_DIR: &str = "delivered";
pub const DEFAULT_RETAIN_BLOCKS: u32 = 870;
const LOCK_FILE: &str = "LOCK";

/// How many accepted blocks the cursor remembers. More than a frontier (k =
/// 290 blocks), so a restart that re-sends the whole frontier finds every
/// parent.
pub const CURSOR_RECENT: usize = 2048;
/// How many gaps the status keeps in memory; `gaps.jsonl` keeps all of them.
const RECENT_GAPS: usize = 64;

pub const DEFAULT_MAX_ENTRIES: usize = 20_000;
pub const DEFAULT_MAX_BYTES: u64 = 4 << 30;

#[derive(Debug, Clone)]
pub struct OutboxConfig {
    pub dir: PathBuf,
    /// When an append would exceed either bound, the oldest entries are
    /// dropped, each recorded as a gap.
    pub max_entries: usize,
    pub max_bytes: u64,
    pub retain_blocks: u32,
}

impl OutboxConfig {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            max_entries: DEFAULT_MAX_ENTRIES,
            max_bytes: DEFAULT_MAX_BYTES,
            retain_blocks: DEFAULT_RETAIN_BLOCKS,
        }
    }
}

/// The block an entry carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRef {
    pub height: u32,
    pub state_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryMeta {
    pub seq: u64,
    pub block: BlockRef,
    pub size: u64,
}

/// An entry read for delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub meta: EntryMeta,
    pub parent_hash: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// A block arrived whose parent the outbox never accepted: the blocks in
    /// between never reached this node's frontier while it was feeding the
    /// archive (the node was down or bootstrapped past them).
    UnseenParent,
    /// The outbox was full and dropped its oldest entry.
    DroppedFull,
    /// An entry failed its checks and was moved to `corrupt/`.
    Corrupt,
}

/// History the archive won't get from this node. Fill it from precomputed
/// blocks (`<network>-<height>-<state hash>.json`, imported with
/// `mina-archive-blocks --precomputed`), as OCaml's missing-blocks guardian
/// does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    pub kind: GapKind,
    /// The heights missing, inclusive.
    pub from_height: u32,
    pub to_height: u32,
    /// The missing block, when known (dropped and corrupt entries).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_hash: Option<String>,
    /// The missing parent, for `unseen_parent`: the top of the gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_hash: Option<String>,
    /// The block that revealed an `unseen_parent` gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_block_after: Option<BlockRef>,
    pub detected_at_ms: u64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Cursor {
    /// The first block this outbox accepted: the history below it was never
    /// this node's to deliver.
    #[serde(default)]
    first: Option<BlockRef>,
    /// The highest block height accepted.
    max_height: u32,
    /// The latest accepted blocks, oldest first.
    recent: VecDeque<BlockRef>,
}

impl Cursor {
    fn contains(&self, state_hash: &str) -> bool {
        self.recent.iter().any(|b| b.state_hash == state_hash)
    }

    fn push(&mut self, block: BlockRef) {
        if self.first.is_none() {
            self.first = Some(block.clone());
        }
        self.max_height = self.max_height.max(block.height);
        if !self.contains(&block.state_hash) {
            self.recent.push_back(block);
            while self.recent.len() > CURSOR_RECENT {
                self.recent.pop_front();
            }
        }
    }
}

/// What [`Outbox::append`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppendOutcome {
    Queued {
        seq: u64,
        /// Gaps this append detected or caused.
        gaps: Vec<Gap>,
    },
    /// An entry for this block is already waiting.
    AlreadyPending { seq: u64 },
}

/// The outbox's numbers, for the status and the logs.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OutboxStats {
    pub pending_entries: usize,
    pub pending_bytes: u64,
    pub retained_entries: usize,
    pub retained_bytes: u64,
    pub total_entries: usize,
    pub total_bytes: u64,
    pub retain_blocks: u32,
    pub highest_accepted_height: u32,
    pub oldest_retained: Option<BlockRef>,
    pub oldest_pending: Option<BlockRef>,
    pub newest_pending: Option<BlockRef>,
    pub max_entries: usize,
    pub max_bytes: u64,
    /// The first block the outbox ever accepted.
    pub feed_started_at: Option<BlockRef>,
    /// Since the outbox was opened.
    pub delivered: u64,
    pub dropped_full: u64,
    pub corrupt: u64,
    pub gaps_total: u64,
    pub last_delivered: Option<BlockRef>,
    pub recent_gaps: Vec<Gap>,
}

pub struct Outbox {
    config: OutboxConfig,
    _lock: File,
    entries: BTreeMap<u64, EntryMeta>,
    retained: BTreeMap<u64, EntryMeta>,
    pending_bytes: u64,
    next_seq: u64,
    cursor: Cursor,
    in_flight: Option<u64>,
    stats: OutboxStats,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Writes `data` to `path` through a synced temporary file and a rename.
fn write_atomic(dir: &Path, name: &str, data: &[u8]) -> io::Result<()> {
    let tmp = dir.join(format!("{name}{TMP_SUFFIX}"));
    {
        let mut file = File::create(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, dir.join(name))?;
    sync_dir(dir)
}

fn entry_name(seq: u64, block: &BlockRef) -> String {
    format!(
        "{seq:020}-{:010}-{}{ENTRY_SUFFIX}",
        block.height, block.state_hash
    )
}

fn parse_entry_name(name: &str) -> Option<(u64, BlockRef)> {
    let stem = name.strip_suffix(ENTRY_SUFFIX)?;
    let mut parts = stem.splitn(3, '-');
    let seq = parts.next()?.parse().ok()?;
    let height = parts.next()?.parse().ok()?;
    let state_hash = parts.next()?.to_owned();
    if state_hash.is_empty() {
        return None;
    }
    Some((seq, BlockRef { height, state_hash }))
}

fn put_str(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u16).to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
}

/// The bytes of an entry file.
fn encode_entry(seq: u64, block: &BlockRef, parent_hash: &str, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(payload.len() + 160);
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    buf.extend_from_slice(&seq.to_le_bytes());
    buf.extend_from_slice(&block.height.to_le_bytes());
    put_str(&mut buf, &block.state_hash);
    put_str(&mut buf, parent_hash);
    buf.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    buf.extend_from_slice(payload);
    let checksum = Sha3_256::digest(&buf);
    buf.extend_from_slice(&checksum);
    buf
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.0.len() < n {
            return Err("truncated".to_owned());
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn str(&mut self) -> Result<String, String> {
        let len = self.u16()? as usize;
        String::from_utf8(self.take(len)?.to_vec()).map_err(|e| e.to_string())
    }
}

/// Checks an entry file against its checksum and the metadata of its name.
fn decode_entry(bytes: &[u8], expected: &EntryMeta) -> Result<Entry, String> {
    if bytes.len() < CHECKSUM_LEN {
        return Err("truncated".to_owned());
    }
    let (body, checksum) = bytes.split_at(bytes.len() - CHECKSUM_LEN);
    if Sha3_256::digest(body).as_slice() != checksum {
        return Err("checksum mismatch".to_owned());
    }
    let mut r = Reader(body);
    if r.take(MAGIC.len())? != MAGIC {
        return Err("bad magic".to_owned());
    }
    let version = r.u32()?;
    if version != FORMAT_VERSION {
        return Err(format!("unknown format version {version}"));
    }
    let seq = r.u64()?;
    let height = r.u32()?;
    let state_hash = r.str()?;
    let parent_hash = r.str()?;
    let len = r.u64()? as usize;
    let payload = r.take(len)?.to_vec();
    if !r.0.is_empty() {
        return Err("trailing bytes".to_owned());
    }
    let block = BlockRef { height, state_hash };
    if seq != expected.seq || block != expected.block {
        return Err("header doesn't match the file name".to_owned());
    }
    Ok(Entry {
        meta: expected.clone(),
        parent_hash,
        payload,
    })
}

// Both directory entries must be durable before forgetting the old location.
fn rename_file(from: &Path, to: &Path, name: &str) -> io::Result<()> {
    checkpoint("move_rename")?;
    match fs::rename(from.join(name), to.join(name)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound && to.join(name).is_file() => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

fn sync_move(from: &Path, to: &Path) -> io::Result<()> {
    checkpoint("move_dest_sync")?;
    sync_dir(to)?;
    checkpoint("move_source_sync")?;
    sync_dir(from)
}

fn move_file(from: &Path, to: &Path, name: &str) -> io::Result<()> {
    rename_file(from, to, name)?;
    sync_move(from, to)
}

fn gap_journal(dir: &Path) -> io::Result<File> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(dir.join(GAPS_FILE))?;
    let len = file.metadata()?.len();
    if len > 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut last = [0];
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            file.seek(SeekFrom::Start(0))?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            file.set_len(end as u64)?;
            file.sync_all()?;
        }
    }
    sync_dir(dir)?;
    Ok(file)
}

#[cfg(test)]
thread_local! {
    static FAULT: std::cell::RefCell<Option<(&'static str, bool)>> = const { std::cell::RefCell::new(None) };
}

fn checkpoint(_step: &'static str) -> io::Result<()> {
    #[cfg(test)]
    FAULT.with(|fault| {
        let mut fault = fault.borrow_mut();
        if let Some((step, crash)) = *fault {
            if step == _step {
                *fault = None;
                if crash {
                    panic!("injected crash at {step}");
                }
                return Err(io::Error::other(format!("injected I/O failure at {step}")));
            }
        }
        Ok(())
    })?;
    Ok(())
}

impl Outbox {
    /// Opens (or creates) the outbox in `config.dir`, removes leftover
    /// temporary files and quarantines files it can't name.
    pub fn open(config: OutboxConfig) -> io::Result<Self> {
        let dir = &config.dir;
        fs::create_dir_all(dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LOCK_FILE))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!(
                        "archive outbox {} is locked by another process",
                        dir.display()
                    ),
                ))
            }
            Err(fs::TryLockError::Error(e)) => return Err(e),
        }

        let mut outbox = Self {
            stats: OutboxStats {
                max_entries: config.max_entries,
                max_bytes: config.max_bytes,
                ..Default::default()
            },
            config,
            _lock: lock,
            entries: BTreeMap::new(),
            retained: BTreeMap::new(),
            pending_bytes: 0,
            next_seq: 0,
            cursor: Cursor::default(),
            in_flight: None,
        };

        fs::create_dir_all(outbox.config.dir.join(DELIVERED_DIR))?;
        sync_dir(&outbox.config.dir)?;
        outbox.load_recent_gaps()?;
        // Replay the retained window even if it has left the frontier. Each
        // rename is durable before starting delivery; interruption is resumable.
        outbox.requeue_retained_files()?;
        let mut unnamed = Vec::new();
        for dirent in fs::read_dir(&outbox.config.dir)? {
            let dirent = dirent?;
            if !dirent.file_type()?.is_file() {
                continue;
            }
            let name = dirent.file_name().to_string_lossy().into_owned();
            if name.ends_with(TMP_SUFFIX) {
                // A write that didn't reach its rename: the block was never
                // queued, and the node re-sends its frontier on restart.
                fs::remove_file(dirent.path())?;
            } else if name.ends_with(ENTRY_SUFFIX) {
                match parse_entry_name(&name) {
                    Some((seq, block)) => {
                        let size = dirent.metadata()?.len();
                        outbox.pending_bytes += size;
                        outbox.next_seq = outbox.next_seq.max(seq + 1);
                        outbox.entries.insert(seq, EntryMeta { seq, block, size });
                    }
                    None => unnamed.push(name),
                }
            }
        }
        for name in unnamed {
            outbox.quarantine_gap(&name, None, "unparseable entry name")?;
        }

        match fs::read(outbox.config.dir.join(CURSOR_FILE)) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(cursor) => outbox.cursor = cursor,
                Err(e) => mina_node::core::warn!(
                    mina_node::core::log::system_time();
                    summary = "archive outbox cursor unreadable, gap detection restarts",
                    error = e.to_string(),
                ),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }

        outbox.refresh_stats();
        Ok(outbox)
    }

    pub fn dir(&self) -> &Path {
        &self.config.dir
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn stats(&self) -> &OutboxStats {
        &self.stats
    }

    pub fn pending(&self) -> impl Iterator<Item = &EntryMeta> {
        self.entries.values()
    }

    /// Persists a diff for `block`, whose parent is `parent_hash`, before it
    /// is sent. Returns once the entry is on disk.
    pub fn append(
        &mut self,
        block: BlockRef,
        parent_hash: &str,
        payload: &[u8],
    ) -> io::Result<AppendOutcome> {
        if let Some(meta) = self
            .entries
            .values()
            .find(|m| m.block.state_hash == block.state_hash)
        {
            return Ok(AppendOutcome::AlreadyPending { seq: meta.seq });
        }

        // Persist before cursor advancement, even when a later append fails.
        let mut gaps = Vec::new();
        if let Some(gap) = self.unseen_parent_gap(&block, parent_hash) {
            self.record_gap(gap.clone())?;
            gaps.push(gap);
        }
        // Re-applying an acknowledged block explicitly requeues its saved diff.
        if let Some(meta) = self
            .retained
            .values()
            .find(|m| m.block.state_hash == block.state_hash)
            .cloned()
        {
            self.requeue_one(&meta)?;
            self.refresh_stats();
            return Ok(AppendOutcome::Queued {
                seq: meta.seq,
                gaps,
            });
        }
        self.prune_retained()?;

        let seq = self.next_seq;
        let bytes = encode_entry(seq, &block, parent_hash, payload);
        let size = bytes.len() as u64;

        // Make room first, so the outbox never holds more than its bound.
        while self.entries.len() + self.retained.len() + 1 > self.config.max_entries
            || self.total_bytes() + size > self.config.max_bytes
        {
            let Some(victim) = self.retained.keys().next().copied().or_else(|| {
                self.entries
                    .keys()
                    .copied()
                    .find(|seq| Some(*seq) != self.in_flight)
            }) else {
                break;
            };
            let meta = self
                .entries
                .get(&victim)
                .or_else(|| self.retained.get(&victim))
                .cloned()
                .unwrap();
            let gap = Gap {
                kind: GapKind::DroppedFull,
                from_height: meta.block.height,
                to_height: meta.block.height,
                state_hash: Some(meta.block.state_hash.clone()),
                parent_hash: None,
                first_block_after: None,
                detected_at_ms: now_ms(),
            };
            self.record_gap(gap.clone())?;
            self.remove_entry(victim)?;
            self.stats.dropped_full += 1;
            gaps.push(gap);
        }

        let name = entry_name(seq, &block);
        write_atomic(&self.config.dir, &name, &bytes)?;
        self.next_seq += 1;
        self.pending_bytes += size;
        self.entries.insert(
            seq,
            EntryMeta {
                seq,
                block: block.clone(),
                size,
            },
        );

        let mut cursor = self.cursor.clone();
        cursor.push(block);
        checkpoint("cursor_write")?;
        let bytes = serde_json::to_vec(&cursor).map_err(io::Error::other)?;
        write_atomic(&self.config.dir, CURSOR_FILE, &bytes)?;
        self.cursor = cursor;
        self.prune_retained()?;
        self.refresh_stats();
        Ok(AppendOutcome::Queued { seq, gaps })
    }

    fn unseen_parent_gap(&self, block: &BlockRef, parent_hash: &str) -> Option<Gap> {
        // The first block this outbox ever sees starts the feed; the history
        // before it is the archive's to have.
        if self.cursor.recent.is_empty()
            || block.height <= 1
            || self.cursor.contains(&block.state_hash)
            || self.cursor.contains(parent_hash)
        {
            return None;
        }
        let to_height = block.height - 1;
        let from_height = (self.cursor.max_height + 1).min(to_height);
        Some(Gap {
            kind: GapKind::UnseenParent,
            from_height,
            to_height,
            state_hash: None,
            parent_hash: Some(parent_hash.to_owned()),
            first_block_after: Some(block.clone()),
            detected_at_ms: now_ms(),
        })
    }

    /// The oldest entry, marked in flight so a full outbox doesn't drop it
    /// while it is being sent. A corrupt entry is quarantined, recorded as a
    /// gap and skipped.
    pub fn next_for_delivery(&mut self) -> io::Result<Option<Entry>> {
        loop {
            let Some(meta) = self.entries.values().next().cloned() else {
                self.in_flight = None;
                return Ok(None);
            };
            let name = entry_name(meta.seq, &meta.block);
            let decoded = match fs::read(self.config.dir.join(&name)) {
                Ok(bytes) => decode_entry(&bytes, &meta),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Err("file missing".to_owned()),
                Err(e) => return Err(e),
            };
            match decoded {
                Ok(entry) => {
                    self.in_flight = Some(meta.seq);
                    return Ok(Some(entry));
                }
                Err(reason) => {
                    self.quarantine_gap(&name, Some(&meta.block), &reason)?;
                    self.pending_bytes = self.pending_bytes.saturating_sub(meta.size);
                    self.entries.remove(&meta.seq);
                    self.stats.corrupt += 1;
                    self.refresh_stats();
                }
            }
        }
    }

    /// RPC acceptance is not a database commit. Retain the exact diff for replay.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/archive/lib/processor.ml#L4918-L4936>
    pub fn confirm(&mut self, seq: u64) -> io::Result<()> {
        if let Some(meta) = self.entries.get(&seq).cloned() {
            let name = entry_name(seq, &meta.block);
            let delivered = self.config.dir.join(DELIVERED_DIR);
            rename_file(&self.config.dir, &delivered, &name)?;
            self.entries.remove(&seq);
            self.pending_bytes = self.pending_bytes.saturating_sub(meta.size);
            self.stats.last_delivered = Some(meta.block.clone());
            self.retained.insert(seq, meta);
            self.stats.delivered += 1;
            self.in_flight = None;
            // Track the actual location even if a directory fsync fails. Both
            // pre-rename and post-rename layouts remain replayable on restart.
            self.refresh_stats();
            sync_move(&self.config.dir, &delivered)?;
        }
        if self.in_flight == Some(seq) {
            self.in_flight = None;
        }
        self.prune_retained()?;
        self.refresh_stats();
        Ok(())
    }

    /// Requeue all retained acknowledgements. Startup uses the same durable
    /// moves before indexing; the operator path wakes the delivery worker.
    pub fn resend_retained(&mut self) -> io::Result<usize> {
        let metas: Vec<_> = self.retained.values().cloned().collect();
        for meta in &metas {
            self.requeue_one(meta)?;
        }
        self.refresh_stats();
        Ok(metas.len())
    }

    fn requeue_retained_files(&mut self) -> io::Result<()> {
        let dir = self.config.dir.join(DELIVERED_DIR);
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.ends_with(ENTRY_SUFFIX) {
                    move_file(&dir, &self.config.dir, &name)?;
                }
            }
        }
        Ok(())
    }

    fn requeue_one(&mut self, meta: &EntryMeta) -> io::Result<()> {
        let delivered = self.config.dir.join(DELIVERED_DIR);
        rename_file(
            &delivered,
            &self.config.dir,
            &entry_name(meta.seq, &meta.block),
        )?;
        self.retained.remove(&meta.seq);
        self.entries.insert(meta.seq, meta.clone());
        self.pending_bytes += meta.size;
        self.refresh_stats();
        sync_move(&delivered, &self.config.dir)
    }

    fn total_bytes(&self) -> u64 {
        self.pending_bytes + self.retained.values().map(|m| m.size).sum::<u64>()
    }

    fn prune_retained(&mut self) -> io::Result<()> {
        let expired: Vec<_> = self
            .retained
            .values()
            .filter(|meta| {
                self.cursor.max_height.saturating_sub(meta.block.height)
                    >= self.config.retain_blocks
            })
            .map(|m| m.seq)
            .collect();
        for seq in expired {
            self.remove_entry(seq)?;
        }
        Ok(())
    }

    /// The delivery failed; it stays first in line.
    pub fn release(&mut self) {
        self.in_flight = None;
    }

    fn remove_entry(&mut self, seq: u64) -> io::Result<()> {
        let retained = self.retained.contains_key(&seq);
        let Some(meta) = self
            .entries
            .get(&seq)
            .or_else(|| self.retained.get(&seq))
            .cloned()
        else {
            return Ok(());
        };
        let dir = if retained {
            self.config.dir.join(DELIVERED_DIR)
        } else {
            self.config.dir.clone()
        };
        checkpoint("entry_remove")?;
        match fs::remove_file(dir.join(entry_name(seq, &meta.block))) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        checkpoint("entry_remove_sync")?;
        sync_dir(&dir)?;
        checkpoint("entry_removed")?;
        if retained {
            self.retained.remove(&seq);
        } else {
            self.entries.remove(&seq);
            self.pending_bytes = self.pending_bytes.saturating_sub(meta.size);
        }
        self.refresh_stats();
        Ok(())
    }

    fn quarantine_gap(
        &mut self,
        name: &str,
        block: Option<&BlockRef>,
        reason: &str,
    ) -> io::Result<()> {
        self.record_gap(Gap {
            kind: GapKind::Corrupt,
            // 0 means an unparseable filename: no trustworthy height/hash.
            from_height: block.map_or(0, |b| b.height),
            to_height: block.map_or(0, |b| b.height),
            state_hash: block.map(|b| b.state_hash.clone()),
            parent_hash: None,
            first_block_after: None,
            detected_at_ms: now_ms(),
        })?;
        self.quarantine(name, reason)
    }

    fn quarantine(&mut self, name: &str, reason: &str) -> io::Result<()> {
        let corrupt = self.config.dir.join(CORRUPT_DIR);
        fs::create_dir_all(&corrupt)?;
        let from = self.config.dir.join(name);
        if from.exists() {
            checkpoint("quarantine_rename")?;
            fs::rename(&from, corrupt.join(name))?;
        }
        checkpoint("quarantine_dest_sync")?;
        sync_dir(&corrupt)?;
        checkpoint("quarantine_source_sync")?;
        sync_dir(&self.config.dir)?;
        checkpoint("quarantine_done")?;
        mina_node::core::error!(
            mina_node::core::log::system_time();
            summary = "archive outbox entry corrupt, moved aside",
            file = name,
            reason = reason,
        );
        Ok(())
    }

    fn record_gap(&mut self, gap: Gap) -> io::Result<()> {
        let mut line = serde_json::to_vec(&gap).map_err(io::Error::other)?;
        line.push(b'\n');
        checkpoint("gap_open")?;
        let mut file = gap_journal(&self.config.dir)?;
        checkpoint("gap_write")?;
        file.write_all(&line)?;
        checkpoint("gap_file_sync")?;
        file.sync_all()?;
        checkpoint("gap_dir_sync")?;
        sync_dir(&self.config.dir)?;
        checkpoint("gap_durable")?;
        mina_node::core::error!(
            mina_node::core::log::system_time();
            summary = "archive history gap",
            kind = format!("{:?}", gap.kind),
            from_height = gap.from_height,
            to_height = gap.to_height,
            state_hash = gap.state_hash.clone().unwrap_or_default(),
            parent_hash = gap.parent_hash.clone().unwrap_or_default(),
        );
        self.stats.gaps_total += 1;
        self.stats.recent_gaps.push(gap);
        let excess = self.stats.recent_gaps.len().saturating_sub(RECENT_GAPS);
        self.stats.recent_gaps.drain(..excess);
        Ok(())
    }

    fn load_recent_gaps(&mut self) -> io::Result<()> {
        // A torn final append cannot be evidence for deletion. Trim it before
        // appending again, so it cannot swallow the next durable gap record.
        gap_journal(&self.config.dir)?;
        let text = fs::read_to_string(self.config.dir.join(GAPS_FILE))?;
        for line in text.lines() {
            let gap = serde_json::from_str(line).map_err(io::Error::other)?;
            self.stats.gaps_total += 1;
            self.stats.recent_gaps.push(gap);
            if self.stats.recent_gaps.len() > RECENT_GAPS {
                self.stats.recent_gaps.remove(0);
            }
        }
        Ok(())
    }

    fn refresh_stats(&mut self) {
        self.stats.retained_entries = self.retained.len();
        self.stats.retained_bytes = self.retained.values().map(|m| m.size).sum();
        self.stats.total_entries = self.entries.len() + self.retained.len();
        self.stats.total_bytes = self.total_bytes();
        self.stats.retain_blocks = self.config.retain_blocks;
        self.stats.highest_accepted_height = self.cursor.max_height;
        self.stats.oldest_retained = self
            .retained
            .values()
            .min_by_key(|m| m.block.height)
            .map(|m| m.block.clone());
        self.stats.feed_started_at = self.cursor.first.clone();
        self.stats.pending_entries = self.entries.len();
        self.stats.pending_bytes = self.pending_bytes;
        self.stats.oldest_pending = self.entries.values().next().map(|m| m.block.clone());
        self.stats.newest_pending = self.entries.values().next_back().map(|m| m.block.clone());
    }
}

#[cfg(test)]
mod tests;
