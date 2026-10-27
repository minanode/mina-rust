//! The store logic (R20 D2 to D4, D6): one writer, which writes data files,
//! then the manifest in the slot not holding the newest generation, then
//! deletes the files neither slot references.
//!
//! - **Commit point.** Data files are complete (and natively synced) before a
//!   manifest names them, and generations alternate between two checksummed
//!   slots. A crash at any point leaves the old or the new generation
//!   loadable. Files a crash left half-written are referenced by no manifest,
//!   and garbage collection deletes them when the store is opened, before any
//!   write.
//! - **Stored means listed.** A ledger counts as stored only when the newest
//!   manifest lists it with its file's length and CRC32, never because a file
//!   of that name exists.
//! - **Immutable files.** A file a manifest references is never written again:
//!   asked to write it, the store reuses it.
//! - **Versions (D6).** A store of an older format version, or of another
//!   chain, is deleted; one of a newer version is left untouched and the store
//!   doesn't open, so that running an older binary once doesn't destroy it.

use redux::Instant;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufReader, BufWriter, Read, Write},
};

use ledger::Mask;
use mina_core::ChainId;
use mina_p2p_messages::{binprot::SmallString1k, v2};

use super::{
    backend::{StoreBackend, StoreReader, WriteAdapter},
    diff::{LedgerDiff, LedgerWrite},
    format::{
        block_name, data_file_limit, decode_manifest, encode_manifest, is_data_file,
        ledger_diff_name, ledger_full_name, parts_name, staged_name, Attempts, ChainEntry,
        Checkpoint, FileRef, Manifest, SlotError, ATTEMPTS_FILE, ATTEMPTS_LEN, FORMAT_VERSION,
        MANIFEST_SLOTS, MAX_DIFF_CHAIN, MAX_MANIFEST_FILE,
    },
    ledger_codec::{
        diff_base_child, diff_base_copy, read_diff_onto_timed, read_full_timed, read_header,
        write_diff, write_full, write_full_from, LedgerFileHeader, LedgerFileKind, VerifyMode,
    },
    load::RestoreGuard,
    load_control::LoadControl,
    PersistError,
};

/// Reads and writes go through buffers of this size (D3: reads of 4 MiB).
const IO_BUFFER: usize = 4 << 20;

pub struct StoreConfig {
    pub chain_id: ChainId,
    pub genesis_state_hash: v2::StateHash,
    /// Recorded in each manifest, for diagnostics.
    pub node_commit: String,
    /// Milliseconds since the Unix epoch, for `Manifest::written_at_ms`.
    pub now_ms: Box<dyn Fn() -> u64>,
}

/// Writes a file's content: parts or a block, encoded as it is written.
/// `Send`, so that a [`CheckpointWrite`] made on the ledger thread can go to
/// the store thread.
pub type Blob = Box<dyn FnOnce(&mut dyn Write) -> io::Result<()> + Send>;

pub fn blob(bytes: Vec<u8>) -> Blob {
    Box::new(move |w| w.write_all(&bytes))
}

/// What a checkpoint writes (D4). The ledger thread takes the snapshot; the
/// store thread writes it.
pub struct CheckpointWrite {
    pub checkpoint: Checkpoint,
    /// The snarked ledgers to keep, the root snarked ledger among them. `None`
    /// for a ledger that is stored already. For a stored ledger, a
    /// [`LedgerWrite::Full`] replaces a stored diff (to cut a diff chain whose
    /// base is no longer kept), and a [`LedgerWrite::Diff`] is ignored.
    pub ledgers: Vec<(v2::LedgerHash, Option<LedgerWrite>)>,
    /// The root staged ledger as a diff over the root snarked ledger.
    pub staged: LedgerDiff,
    /// The root's `StagedLedgerAuxAndPendingCoinbases`, binprot.
    pub parts: Blob,
    /// The best chain from the root to the tip.
    pub chain: Vec<ChainEntry>,
    /// The blocks of `chain` that aren't stored yet, binprot.
    pub blocks: Vec<(v2::StateHash, Blob)>,
}

static_assertions::assert_impl_all!(CheckpointWrite: Send);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotStatus {
    Absent,
    Valid { generation: u64 },
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenOutcome {
    /// No usable manifest.
    Empty,
    Opened {
        generation: u64,
    },
    /// The store was of an older format version or another chain, and was
    /// deleted.
    Reset {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct OpenReport {
    pub outcome: OpenOutcome,
    pub slots: [SlotStatus; 2],
    /// Files garbage collection deleted.
    pub deleted: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainPutResult {
    pub generation: u64,
    /// The new chain's root isn't on the stored chain (after a bootstrap or a
    /// deep reorganisation): the manifest no longer has a checkpoint, and the
    /// node should take one.
    pub checkpoint_invalidated: bool,
}

struct Slot {
    index: usize,
    manifest: Manifest,
}

pub struct FrontierStore<B: StoreBackend> {
    backend: B,
    config: StoreConfig,
    /// The restore in progress, whose attempt a clean shutdown takes back.
    restore_guard: RestoreGuard,
    /// The newest valid generation.
    current: Option<Slot>,
    /// The other slot, if it holds a valid generation.
    previous: Option<Slot>,
}

/// The buffered writer a file's content is written to.
pub type FileSink<'a> = BufWriter<WriteAdapter<'a>>;

fn read_all(reader: Box<dyn StoreReader>, limit: u64) -> io::Result<Vec<u8>> {
    let len = reader.len();
    if len > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file exceeds size limit",
        ));
    }
    let mut bytes = Vec::new();
    // Also bound a backend whose file grows after its metadata was read.
    reader.take(len + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file length changed",
        ));
    }
    Ok(bytes)
}

impl<B: StoreBackend> FrontierStore<B> {
    /// Takes the lock, reads both manifest slots, applies the version rules
    /// and deletes every data file no valid slot references.
    pub fn open(mut backend: B, config: StoreConfig) -> Result<(Self, OpenReport), PersistError> {
        backend.lock().map_err(|error| match error.kind() {
            io::ErrorKind::WouldBlock => PersistError::Locked,
            _ => PersistError::Io(error),
        })?;
        let names = backend.list()?.into_iter().collect::<BTreeSet<_>>();

        let mut slots = [SlotStatus::Absent, SlotStatus::Absent];
        let mut valid = Vec::new();
        let mut reset = None;
        for (index, slot) in MANIFEST_SLOTS.iter().enumerate() {
            if !names.contains(*slot) {
                continue;
            }
            let bytes = read_all(backend.open(slot)?, MAX_MANIFEST_FILE)?;
            let manifest = match decode_manifest(&bytes) {
                Ok(manifest) => manifest,
                Err(SlotError::NewerVersion(found)) => {
                    return Err(PersistError::NewerFormat {
                        found,
                        ours: FORMAT_VERSION,
                    })
                }
                Err(error) => {
                    if let SlotError::OlderVersion(version) = error {
                        reset = Some(format!("format version {version}"));
                    }
                    slots[index] = SlotStatus::Invalid(error.to_string());
                    continue;
                }
            };
            if manifest.chain_id != config.chain_id
                || manifest.genesis_state_hash != config.genesis_state_hash
            {
                reset = Some("another chain id or genesis".to_owned());
                slots[index] = SlotStatus::Invalid("another chain".into());
                continue;
            }
            match Self::check_files(&mut backend, &names, &manifest)? {
                Ok(()) => {
                    slots[index] = SlotStatus::Valid {
                        generation: manifest.generation,
                    };
                    valid.push(Slot { index, manifest });
                }
                Err(reason) => slots[index] = SlotStatus::Invalid(reason),
            }
        }

        valid.sort_by_key(|slot| std::cmp::Reverse(slot.manifest.generation));
        let mut valid = valid.into_iter();
        let mut store = Self {
            backend,
            config,
            restore_guard: RestoreGuard::default(),
            current: valid.next(),
            previous: valid.next(),
        };

        let (outcome, deleted) = match (&store.current, reset) {
            (Some(current), _) => {
                let generation = current.manifest.generation;
                (OpenOutcome::Opened { generation }, store.gc()?)
            }
            (None, Some(reason)) => (OpenOutcome::Reset { reason }, store.reset()?),
            (None, None) => (OpenOutcome::Empty, store.gc()?),
        };
        Ok((
            store,
            OpenReport {
                outcome,
                slots,
                deleted,
            },
        ))
    }

    /// Opens a store only to read it, as the offline checker does: the chain
    /// is the newest readable manifest's, and nothing is deleted or written.
    /// Takes the lock if `lock` is set, so that no node writes meanwhile.
    pub fn inspect(mut backend: B, lock: bool) -> Result<(Self, OpenReport), PersistError> {
        if lock {
            backend.lock().map_err(|error| match error.kind() {
                io::ErrorKind::WouldBlock => PersistError::Locked,
                _ => PersistError::Io(error),
            })?;
        }
        let names = backend.list()?.into_iter().collect::<BTreeSet<_>>();
        let mut slots = [SlotStatus::Absent, SlotStatus::Absent];
        let mut decoded = Vec::new();
        for (index, slot) in MANIFEST_SLOTS.iter().enumerate() {
            if !names.contains(*slot) {
                continue;
            }
            let bytes = read_all(backend.open(slot)?, MAX_MANIFEST_FILE)?;
            match decode_manifest(&bytes) {
                Ok(manifest) => decoded.push(Slot { index, manifest }),
                Err(SlotError::NewerVersion(found)) => {
                    return Err(PersistError::NewerFormat {
                        found,
                        ours: FORMAT_VERSION,
                    })
                }
                Err(error) => slots[index] = SlotStatus::Invalid(error.to_string()),
            }
        }
        decoded.sort_by_key(|slot| std::cmp::Reverse(slot.manifest.generation));
        let (chain_id, genesis_state_hash) = match decoded.first() {
            Some(slot) => (
                slot.manifest.chain_id.clone(),
                slot.manifest.genesis_state_hash.clone(),
            ),
            None => (
                ChainId::from_hex(&"0".repeat(64)).expect("64 hex digits"),
                v2::StateHash::zero(),
            ),
        };
        let mut valid = Vec::new();
        for slot in decoded {
            if slot.manifest.chain_id != chain_id
                || slot.manifest.genesis_state_hash != genesis_state_hash
            {
                slots[slot.index] = SlotStatus::Invalid("another chain".into());
                continue;
            }
            match Self::check_files(&mut backend, &names, &slot.manifest)? {
                Ok(()) => {
                    slots[slot.index] = SlotStatus::Valid {
                        generation: slot.manifest.generation,
                    };
                    valid.push(slot);
                }
                Err(reason) => slots[slot.index] = SlotStatus::Invalid(reason),
            }
        }
        let mut valid = valid.into_iter();
        let store = Self {
            backend,
            config: StoreConfig {
                chain_id,
                genesis_state_hash,
                node_commit: String::new(),
                now_ms: Box::new(|| 0),
            },
            restore_guard: RestoreGuard::default(),
            current: valid.next(),
            previous: valid.next(),
        };
        let outcome = match &store.current {
            Some(current) => OpenOutcome::Opened {
                generation: current.manifest.generation,
            },
            None => OpenOutcome::Empty,
        };
        Ok((
            store,
            OpenReport {
                outcome,
                slots,
                deleted: Vec::new(),
            },
        ))
    }

    /// The data files present that neither valid manifest references: what
    /// the next open's garbage collection deletes.
    pub fn unreferenced(&mut self) -> Result<Vec<String>, PersistError> {
        Ok(self
            .backend
            .list()?
            .into_iter()
            .filter(|name| is_data_file(name) && self.referenced_file(name).is_none())
            .collect())
    }

    /// The ledgers the newest manifest lists, each with the base its file is
    /// a diff over, if it is one.
    pub fn stored_ledgers(
        &mut self,
    ) -> Result<Vec<(v2::LedgerHash, Option<v2::LedgerHash>)>, PersistError> {
        let ledgers = self
            .manifest()
            .map(|manifest| manifest.ledgers.clone())
            .unwrap_or_default();
        ledgers
            .into_iter()
            .map(|hash| {
                let base = self.ledger_base(&hash)?;
                Ok((hash, base))
            })
            .collect()
    }

    /// Every file the manifest references exists with its recorded length
    /// (the inner `Err` says which doesn't). Their checksums are checked as
    /// they are read.
    ///
    /// Only a missing or short file invalidates the slot. Any other error
    /// fails the open: a slot found invalid has its files deleted by the
    /// garbage collection that follows, so a transient error (a busy file in
    /// the browser, say) must not count against it.
    fn check_files(
        backend: &mut B,
        names: &BTreeSet<String>,
        manifest: &Manifest,
    ) -> Result<Result<(), String>, PersistError> {
        if let Err(reason) = manifest.check_limits() {
            return Ok(Err(reason));
        }
        for file in &manifest.files {
            if !names.contains(file.name()) {
                return Ok(Err(format!("{} is missing", file.name())));
            }
            let len = match backend.open(file.name()) {
                Ok(reader) => reader.len(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok(Err(format!("{} is missing", file.name())))
                }
                Err(error) => return Err(error.into()),
            };
            if len != file.len {
                return Ok(Err(format!(
                    "{} has {len} bytes, not {}",
                    file.name(),
                    file.len
                )));
            }
        }
        Ok(Ok(()))
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// The newest valid manifest.
    pub fn manifest(&self) -> Option<&Manifest> {
        self.current.as_ref().map(|slot| &slot.manifest)
    }

    /// The other valid manifest, one generation older.
    pub fn previous_manifest(&self) -> Option<&Manifest> {
        self.previous.as_ref().map(|slot| &slot.manifest)
    }

    pub fn generation(&self) -> u64 {
        self.manifest().map_or(0, |manifest| manifest.generation)
    }

    pub fn is_ledger_stored(&self, hash: &v2::LedgerHash) -> bool {
        self.manifest()
            .is_some_and(|manifest| manifest.has_ledger(hash))
    }

    pub fn is_block_stored(&self, hash: &v2::StateHash) -> bool {
        self.manifest()
            .is_some_and(|manifest| manifest.references(&block_name(hash)))
    }

    /// The file of that name either valid manifest references.
    fn referenced_file(&self, name: &str) -> Option<&FileRef> {
        self.manifest()
            .and_then(|manifest| manifest.file(name))
            .or_else(|| {
                self.previous_manifest()
                    .and_then(|manifest| manifest.file(name))
            })
    }

    /// Deletes every data file neither valid manifest references.
    pub fn gc(&mut self) -> Result<Vec<String>, PersistError> {
        let mut deleted = Vec::new();
        for name in self.backend.list()? {
            if is_data_file(&name) && self.referenced_file(&name).is_none() {
                self.backend.delete(&name)?;
                deleted.push(name);
            }
        }
        Ok(deleted)
    }

    /// Deletes the whole store: manifests, restore attempts and data files.
    pub fn reset(&mut self) -> Result<Vec<String>, PersistError> {
        // A take-back must not write the record into an emptied store.
        self.restore_guard.end();
        self.current = None;
        self.previous = None;
        let mut deleted = Vec::new();
        for name in self.backend.list()? {
            if is_data_file(&name)
                || MANIFEST_SLOTS.contains(&name.as_str())
                || name == ATTEMPTS_FILE
            {
                self.backend.delete(&name)?;
                deleted.push(name);
            }
        }
        Ok(deleted)
    }

    /// Gives up the newest generation, which failed to load (a checksum, a
    /// decode or a hash check): its slot is deleted, the previous generation
    /// becomes the newest, and what only the given-up one referenced is
    /// deleted. Returns whether a generation is left.
    pub fn fall_back(&mut self) -> Result<bool, PersistError> {
        let Some(current) = self.current.take() else {
            return Ok(false);
        };
        self.current = self.previous.take();
        self.backend.delete(MANIFEST_SLOTS[current.index])?;
        self.gc()?;
        Ok(self.current.is_some())
    }

    /// Writes a data file, or reuses it if a manifest references it already.
    fn write_file<F>(&mut self, name: &str, write: F) -> Result<FileRef, PersistError>
    where
        F: FnOnce(&mut FileSink<'_>) -> Result<(), PersistError>,
    {
        if let Some(file) = self.referenced_file(name) {
            return Ok(file.clone());
        }
        let mut writer = self.backend.create(name)?;
        let written = {
            let mut sink = BufWriter::with_capacity(IO_BUFFER, WriteAdapter(writer.as_mut()));
            let result = write(&mut sink).and_then(|()| Ok(sink.flush()?));
            // Don't flush what is left after an error.
            let _ = sink.into_parts();
            result
        };
        let result = written.and_then(|()| Ok(writer.finish()?));
        match result {
            Ok((len, crc32)) => Ok(FileRef::new(name, len, crc32)),
            Err(error) => {
                // Best effort: garbage collection deletes it at the next open.
                let _ = self.backend.delete(name);
                Err(error)
            }
        }
    }

    fn write_ledger(
        &mut self,
        hash: &v2::LedgerHash,
        write: &LedgerWrite,
    ) -> Result<FileRef, PersistError> {
        let (name, expected) = match write {
            LedgerWrite::Full(_) => (ledger_full_name(hash), hash.clone()),
            LedgerWrite::Stream(source) => {
                (ledger_full_name(hash), source.header().ledger_hash.clone())
            }
            LedgerWrite::Diff(diff) => {
                (ledger_diff_name(hash), v2::LedgerHash::from_fp(diff.target))
            }
        };
        if &expected != hash {
            return Err(PersistError::Invalid(format!(
                "a diff for {expected} given for ledger {hash}"
            )));
        }
        self.write_file(&name, |sink| {
            let header = match write {
                LedgerWrite::Full(mask) => write_full(sink, mask)?,
                LedgerWrite::Stream(source) => write_full_from(sink, source.as_ref())?,
                LedgerWrite::Diff(diff) => write_diff(sink, diff)?,
            };
            if &header.ledger_hash != hash {
                return Err(PersistError::Invalid(format!(
                    "ledger {} given for {hash}",
                    header.ledger_hash
                )));
            }
            Ok(())
        })
    }

    /// Opens a file a manifest references, checking its length now and its
    /// checksum when [`FileReader::finish`] is called.
    pub fn open_file(&mut self, file: &FileRef) -> Result<FileReader, PersistError> {
        let reader = self.backend.open(file.name()).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                PersistError::MissingFile(file.name().to_owned())
            } else {
                PersistError::Io(error)
            }
        })?;
        let limit = data_file_limit(file.name())
            .ok_or_else(|| PersistError::corrupt(file.name(), "unknown file kind"))?;
        if reader.len() > limit || file.len > limit {
            return Err(PersistError::corrupt(
                file.name(),
                "file exceeds size limit",
            ));
        }
        if reader.len() != file.len {
            return Err(PersistError::corrupt(
                file.name(),
                format!("{} bytes, not {}", reader.len(), file.len),
            ));
        }
        Ok(FileReader {
            reader: BufReader::with_capacity(
                (IO_BUFFER as u64).min(file.len.max(1)) as usize,
                CrcRead {
                    inner: reader.take(file.len + 1),
                    crc: crc32fast::Hasher::new(),
                    read: 0,
                },
            ),
            file: file.clone(),
        })
    }

    /// The whole content of a small file (parts, a block), checked.
    pub fn read_blob(&mut self, file: &FileRef) -> Result<Vec<u8>, PersistError> {
        self.read_blob_controlled(file, &LoadControl::default())
    }

    pub(crate) fn read_blob_controlled(
        &mut self,
        file: &FileRef,
        control: &LoadControl,
    ) -> Result<Vec<u8>, PersistError> {
        control.check()?;
        let mut reader = self.open_file(file)?;
        let mut bytes = Vec::with_capacity(file.len as usize);
        let mut chunk = [0u8; 64 * 1024];
        loop {
            control.check()?;
            let n = reader.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..n]);
            control.completed(n as u64)?;
        }
        reader.finish()?;
        Ok(bytes)
    }

    /// Reads a file to its end and checks its length and checksum, without
    /// decoding it.
    ///
    /// Ledger files are checked so before they are decoded (and again while
    /// they are): converting a damaged account can panic in the ledger crate
    /// rather than fail, and a file that passes its checksum is one this store
    /// wrote. It costs a second read of each ledger file.
    pub fn verify_file(&mut self, file: &FileRef) -> Result<(), PersistError> {
        let mut reader = self.open_file(file)?;
        io::copy(&mut reader, &mut io::sink())?;
        reader.finish()
    }

    /// The base a stored ledger diff was written over, from its header.
    pub fn ledger_base(
        &mut self,
        hash: &v2::LedgerHash,
    ) -> Result<Option<v2::LedgerHash>, PersistError> {
        let Some(file) = self
            .manifest()
            .and_then(|manifest| manifest.ledger_file(hash))
        else {
            return Ok(None);
        };
        let file = file.clone();
        let mut reader = self.open_file(&file)?;
        Ok(match read_header(&mut reader)?.kind {
            LedgerFileKind::Full => None,
            LedgerFileKind::Diff { base } => Some(base),
        })
    }

    /// Reads the files of the ledgers `hashes` and of their bases, each once,
    /// into memory, its length and CRC32 checked before anything decodes it
    /// (converting a damaged account can panic in the ledger crate rather
    /// than fail, and a file that passes its checksum is one this store
    /// wrote). Nothing is decoded but the headers.
    pub fn read_ledger_files(
        &mut self,
        manifest: &Manifest,
        hashes: &[v2::LedgerHash],
    ) -> Result<LedgerFiles, PersistError> {
        self.read_ledger_files_controlled(manifest, hashes, &LoadControl::default())
    }

    pub(crate) fn read_ledger_files_controlled(
        &mut self,
        manifest: &Manifest,
        hashes: &[v2::LedgerHash],
        control: &LoadControl,
    ) -> Result<LedgerFiles, PersistError> {
        let mut files = LedgerFiles::default();
        let mut wanted = hashes.iter().rev().cloned().collect::<Vec<_>>();
        while let Some(hash) = wanted.pop() {
            if files.0.contains_key(&hash) {
                continue;
            }
            let file = manifest
                .ledger_file(&hash)
                .ok_or_else(|| PersistError::MissingFile(ledger_full_name(&hash)))?
                .clone();
            let bytes = self.read_blob_controlled(&file, control)?;
            let mut body = bytes.as_slice();
            let header = read_header(&mut body)?;
            if header.ledger_hash != hash {
                return Err(PersistError::corrupt(file.name(), "holds another ledger"));
            }
            let offset = bytes.len() - body.len();
            if let LedgerFileKind::Diff { base } = &header.kind {
                wanted.push(base.clone());
            }
            files.0.insert(
                hash,
                LedgerFileBytes {
                    name: file.name().to_owned(),
                    header,
                    bytes,
                    offset,
                },
            );
        }
        Ok(files)
    }

    /// Loads every ledger the manifest lists, with their bases: the files
    /// read and checked ([`Self::read_ledger_files`]), then decoded and
    /// verified ([`decode_ledgers`]).
    pub fn read_ledgers(
        &mut self,
        manifest: &Manifest,
        verify: VerifyMode,
    ) -> Result<BTreeMap<v2::LedgerHash, Mask>, PersistError> {
        let files = self.read_ledger_files(manifest, &manifest.ledgers)?;
        decode_ledgers(files, verify)
    }

    /// Reads the checkpoint's root staged ledger's file, checked as
    /// [`Self::read_ledger_files`] does, and its header: a diff over the root
    /// snarked ledger, of the root's staged ledger.
    pub fn read_staged_file(
        &mut self,
        manifest: &Manifest,
    ) -> Result<LedgerFileBytes, PersistError> {
        self.read_staged_file_controlled(manifest, &LoadControl::default())
    }

    pub(crate) fn read_staged_file_controlled(
        &mut self,
        manifest: &Manifest,
        control: &LoadControl,
    ) -> Result<LedgerFileBytes, PersistError> {
        let checkpoint = manifest
            .checkpoint
            .as_ref()
            .ok_or_else(|| PersistError::Invalid("the manifest has no checkpoint".into()))?;
        let name = staged_name(&checkpoint.root);
        let file = manifest
            .file(&name)
            .ok_or_else(|| PersistError::MissingFile(name.clone()))?
            .clone();
        let bytes = self.read_blob_controlled(&file, control)?;
        let mut body = bytes.as_slice();
        let header = read_header(&mut body)?;
        if header.kind
            != (LedgerFileKind::Diff {
                base: checkpoint.root_snarked_ledger.clone(),
            })
        {
            return Err(PersistError::corrupt(
                &name,
                "not a diff over the root snarked ledger",
            ));
        }
        if header.ledger_hash != checkpoint.root_staged_ledger_hash.non_snark.ledger_hash {
            return Err(PersistError::corrupt(&name, "not the root's staged ledger"));
        }
        let offset = bytes.len() - body.len();
        Ok(LedgerFileBytes {
            name,
            header,
            bytes,
            offset,
        })
    }

    /// Loads the checkpoint's root staged ledger: its diff over the root
    /// snarked ledger, applied to a child mask of `root_snarked`, which
    /// keeps it as its child (I5d5), as OCaml's restored root staged ledger
    /// is a mask over the root snarked ledger. `root_snarked` must be loaded
    /// with its hashes checked in full: the staged ledger's verification reads
    /// the hashes off its changed paths from it.
    pub fn read_staged(
        &mut self,
        manifest: &Manifest,
        root_snarked: &Mask,
        verify: VerifyMode,
    ) -> Result<Mask, PersistError> {
        let file = self.read_staged_file(manifest)?;
        let base = file.base_child(root_snarked)?;
        file.decode(Some(base), verify)
    }

    /// The files a ledger's file needs: itself and its bases, from what this
    /// operation wrote (`written`) or from the newest manifest.
    fn ledger_closure(
        &mut self,
        hash: &v2::LedgerHash,
        written: &BTreeMap<v2::LedgerHash, FileRef>,
        out: &mut Vec<FileRef>,
    ) -> Result<(), PersistError> {
        let mut hash = hash.clone();
        for _ in 0..=MAX_DIFF_CHAIN {
            let file = match written.get(&hash) {
                Some(file) => file.clone(),
                None => self
                    .manifest()
                    .and_then(|manifest| manifest.ledger_file(&hash))
                    .cloned()
                    .ok_or_else(|| PersistError::Invalid(format!("ledger {hash} is not stored")))?,
            };
            let mut reader = self.open_file(&file)?;
            let kind = read_header(&mut reader)?.kind;
            out.push(file);
            match kind {
                LedgerFileKind::Full => return Ok(()),
                LedgerFileKind::Diff { base } => hash = base,
            }
        }
        Err(PersistError::Invalid(format!(
            "the chain of diff bases of {hash} is too long"
        )))
    }

    /// Writes the manifest in the slot not holding the newest generation,
    /// then deletes the files neither slot references.
    fn commit(&mut self, mut manifest: Manifest) -> Result<u64, PersistError> {
        manifest.generation = self.generation() + 1;
        manifest.chain_id = self.config.chain_id.clone();
        manifest.genesis_state_hash = self.config.genesis_state_hash.clone();
        manifest.written_at_ms = (self.config.now_ms)();
        manifest.node_commit = SmallString1k(self.config.node_commit.chars().take(200).collect());
        let mut seen = BTreeSet::new();
        manifest
            .files
            .retain(|file| seen.insert(file.name().to_owned()));

        manifest.check_limits().map_err(PersistError::Invalid)?;
        let index = match &self.current {
            Some(current) => 1 - current.index,
            None => 0,
        };
        if let Err(error) = self
            .backend
            .write_slot(MANIFEST_SLOTS[index], &encode_manifest(&manifest))
        {
            // The slot may be torn now.
            if self
                .previous
                .as_ref()
                .is_some_and(|slot| slot.index == index)
            {
                self.previous = None;
            }
            return Err(error.into());
        }
        let generation = manifest.generation;
        self.previous = self.current.take();
        self.current = Some(Slot { index, manifest });
        self.gc()?;
        Ok(generation)
    }

    fn empty_manifest(&self) -> Manifest {
        Manifest {
            generation: 0,
            chain_id: self.config.chain_id.clone(),
            genesis_state_hash: self.config.genesis_state_hash.clone(),
            written_at_ms: 0,
            node_commit: SmallString1k(String::new()),
            ledgers: Vec::new(),
            checkpoint: None,
            chain: Vec::new(),
            files: Vec::new(),
        }
    }

    /// Writes the files of the chain's blocks that aren't stored, and returns
    /// the files of all of them.
    fn put_blocks(
        &mut self,
        chain: &[ChainEntry],
        blocks: Vec<(v2::StateHash, Blob)>,
    ) -> Result<Vec<FileRef>, PersistError> {
        let mut blocks = blocks.into_iter().collect::<BTreeMap<_, _>>();
        let mut files = Vec::with_capacity(chain.len());
        for entry in chain {
            let name = block_name(&entry.hash);
            let file = match self.referenced_file(&name) {
                Some(file) => file.clone(),
                None => {
                    let blob = blocks.remove(&entry.hash).ok_or_else(|| {
                        PersistError::Invalid(format!("block {} is not stored", entry.hash))
                    })?;
                    self.write_file(&name, |sink| Ok(blob(sink)?))?
                }
            };
            files.push(file);
        }
        Ok(files)
    }

    /// Stores a new best chain (D4): the stored chain up to the new chain's
    /// root, then the new chain. If the root isn't on the stored chain, the
    /// checkpoint can't be replayed to it any more, and is dropped.
    pub fn chain_put(
        &mut self,
        chain: Vec<ChainEntry>,
        blocks: Vec<(v2::StateHash, Blob)>,
    ) -> Result<ChainPutResult, PersistError> {
        let result = self.chain_put_inner(chain, blocks);
        if result.is_err() {
            // Best effort: what the failed write left is deleted at the next
            // open anyway.
            let _ = self.gc();
        }
        result
    }

    fn chain_put_inner(
        &mut self,
        chain: Vec<ChainEntry>,
        blocks: Vec<(v2::StateHash, Blob)>,
    ) -> Result<ChainPutResult, PersistError> {
        let new_root = chain
            .first()
            .ok_or_else(|| PersistError::Invalid("an empty chain".into()))?
            .hash
            .clone();
        let mut manifest = self
            .manifest()
            .cloned()
            .unwrap_or_else(|| self.empty_manifest());
        let had_checkpoint = manifest.checkpoint.is_some();
        let stored_chain = std::mem::take(&mut manifest.chain);
        let (full_chain, checkpoint_invalidated) =
            match stored_chain.iter().position(|entry| entry.hash == new_root) {
                Some(position) => {
                    let mut full = stored_chain[..position].to_vec();
                    full.extend(chain);
                    (full, false)
                }
                None => {
                    manifest.checkpoint = None;
                    (chain, had_checkpoint)
                }
            };
        if Some(&full_chain) == self.manifest().map(|manifest| &manifest.chain) {
            return Ok(ChainPutResult {
                generation: self.generation(),
                checkpoint_invalidated: false,
            });
        }

        let block_files = self.put_blocks(&full_chain, blocks)?;
        let mut files = Vec::new();
        let ledgers = manifest.ledgers.clone();
        for hash in &ledgers {
            self.ledger_closure(hash, &BTreeMap::new(), &mut files)?;
        }
        if let Some(checkpoint) = &manifest.checkpoint {
            for name in [staged_name(&checkpoint.root), parts_name(&checkpoint.root)] {
                let file = self
                    .manifest()
                    .and_then(|manifest| manifest.file(&name))
                    .cloned()
                    .ok_or(PersistError::MissingFile(name))?;
                files.push(file);
            }
        }
        files.extend(block_files);
        manifest.chain = full_chain;
        manifest.files = files;
        let generation = self.commit(manifest)?;
        Ok(ChainPutResult {
            generation,
            checkpoint_invalidated,
        })
    }

    /// Writes a checkpoint (D4): the kept snarked ledgers not stored yet, the
    /// root staged ledger's diff, the root's parts and the chain's blocks, then
    /// a manifest naming them.
    pub fn checkpoint(&mut self, input: CheckpointWrite) -> Result<u64, PersistError> {
        let result = self.checkpoint_inner(input);
        if result.is_err() {
            let _ = self.gc();
        }
        result
    }

    fn checkpoint_inner(&mut self, input: CheckpointWrite) -> Result<u64, PersistError> {
        let CheckpointWrite {
            checkpoint,
            ledgers,
            staged,
            parts,
            chain,
            blocks,
        } = input;
        if chain.first().map(|entry| &entry.hash) != Some(&checkpoint.root) {
            return Err(PersistError::Invalid(
                "the chain doesn't start at the checkpoint's root".into(),
            ));
        }
        if !ledgers
            .iter()
            .any(|(hash, _)| hash == &checkpoint.root_snarked_ledger)
        {
            return Err(PersistError::Invalid(
                "the root snarked ledger is not among the ledgers".into(),
            ));
        }
        if v2::LedgerHash::from_fp(staged.base) != checkpoint.root_snarked_ledger
            || v2::LedgerHash::from_fp(staged.target)
                != checkpoint.root_staged_ledger_hash.non_snark.ledger_hash
        {
            return Err(PersistError::Invalid(
                "the staged diff is not the root's, over its snarked ledger".into(),
            ));
        }

        // Full files first: a diff's base may be one of them.
        let mut order = (0..ledgers.len()).collect::<Vec<_>>();
        order.sort_by_key(|&i| !ledgers[i].1.as_ref().is_some_and(LedgerWrite::is_full));
        let mut written = BTreeMap::new();
        for i in order {
            let (hash, write) = &ledgers[i];
            let stored = self.is_ledger_stored(hash);
            let file = match write {
                Some(LedgerWrite::Diff(_)) | None if stored => continue,
                Some(write) => self.write_ledger(hash, write)?,
                None => {
                    return Err(PersistError::Invalid(format!(
                        "ledger {hash} is not stored and nothing was given to write"
                    )))
                }
            };
            written.insert(hash.clone(), file);
        }
        let mut files = Vec::new();
        let mut kept = Vec::with_capacity(ledgers.len());
        for (hash, _) in &ledgers {
            self.ledger_closure(hash, &written, &mut files)?;
            kept.push(hash.clone());
        }

        let staged_file = self.write_file(&staged_name(&checkpoint.root), |sink| {
            write_diff(sink, &staged)?;
            Ok(())
        })?;
        let parts_file = self.write_file(&parts_name(&checkpoint.root), |sink| Ok(parts(sink)?))?;
        let block_files = self.put_blocks(&chain, blocks)?;
        files.push(staged_file);
        files.push(parts_file);
        files.extend(block_files);

        let mut manifest = self.empty_manifest();
        manifest.ledgers = kept;
        manifest.checkpoint = Some(checkpoint);
        manifest.chain = chain;
        manifest.files = files;
        self.commit(manifest)
    }

    /// The restore attempts record, if there is an intact one.
    pub fn attempts(&mut self) -> Result<Option<Attempts>, PersistError> {
        match self.backend.open(ATTEMPTS_FILE) {
            Ok(reader) if reader.len() != ATTEMPTS_LEN => Ok(None),
            Ok(reader) => Ok(Attempts::decode(&read_all(reader, ATTEMPTS_LEN)?)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn set_attempts(&mut self, attempts: Attempts) -> Result<(), PersistError> {
        Ok(self.backend.write_slot(ATTEMPTS_FILE, &attempts.encode())?)
    }

    pub fn clear_attempts(&mut self) -> Result<(), PersistError> {
        self.restore_guard.end();
        Ok(self.backend.delete(ATTEMPTS_FILE)?)
    }

    /// Shares the restore guard of a node's signal thread (RC7-FIXES).
    pub fn set_restore_guard(&mut self, guard: RestoreGuard) {
        self.restore_guard = guard;
    }

    pub fn restore_guard(&self) -> &RestoreGuard {
        &self.restore_guard
    }

    /// Counts a restore's attempt, `attempts`, over `previous`, the record
    /// before it (RC7-FIXES): the new record is written beside the old one,
    /// then committed and the take-back armed under the restore guard's
    /// lock. Fails, leaving the record as it was, if a shutdown has begun.
    pub(crate) fn count_attempt(
        &mut self,
        previous: Option<Attempts>,
        attempts: Attempts,
    ) -> Result<(), PersistError> {
        self.backend
            .prepare_slot(ATTEMPTS_FILE, &attempts.encode())?;
        let backend = &mut self.backend;
        let committed = match backend.detached_slot_writer() {
            Some(write) => {
                let previous = previous.map(|attempts| attempts.encode());
                self.restore_guard.commit_counted(
                    Box::new(move || write(ATTEMPTS_FILE, previous.as_deref())),
                    || backend.commit_slot(ATTEMPTS_FILE),
                )?
            }
            // No signals to take it back on: the browser.
            None => backend.commit_slot(ATTEMPTS_FILE).map(|()| true)?,
        };
        if !committed {
            // Transient: the load stops and the store is kept as it is.
            return Err(PersistError::Io(io::Error::new(
                io::ErrorKind::Interrupted,
                "the node is shutting down: no restore attempt counted",
            )));
        }
        Ok(self.backend.sync_slots()?)
    }
}

/// A ledger file read into memory and checked
/// ([`FrontierStore::read_ledger_files`]), with its header decoded.
pub struct LedgerFileBytes {
    pub name: String,
    pub header: LedgerFileHeader,
    bytes: Vec<u8>,
    /// Where the body starts.
    offset: usize,
}

impl LedgerFileBytes {
    /// For a diff, a copy of its base to apply it to, checked to be its base.
    /// Made apart from [`Self::decode`] because a mask may be used by one
    /// thread at a time: the copies are made on one thread, and the decodes,
    /// each with its own copy, run in parallel.
    pub fn base_copy(&self, base: &Mask) -> Result<Mask, PersistError> {
        Ok(diff_base_copy(&self.header, base)?)
    }

    /// For a diff, a child mask of its base to apply it to, checked to be its
    /// base: the root staged ledger over the root snarked ledger (I5d5).
    /// Nothing else may use `base` or the child while the diff is decoded.
    pub fn base_child(&self, base: &Mask) -> Result<Mask, PersistError> {
        Ok(diff_base_child(&self.header, base)?)
    }

    /// Decodes and verifies the ledger: a full file, or a diff over `base`,
    /// its [`Self::base_copy`] or [`Self::base_child`].
    pub fn decode(self, base: Option<Mask>, verify: VerifyMode) -> Result<Mask, PersistError> {
        self.decode_timed(base, verify).map(|(ledger, _)| ledger)
    }

    /// Returns the ledger and the time spent recomputing and checking hashes.
    pub fn decode_timed(
        self,
        base: Option<Mask>,
        verify: VerifyMode,
    ) -> Result<(Mask, f64), PersistError> {
        self.decode_controlled(base, verify, &LoadControl::default())
    }

    pub(crate) fn decode_controlled(
        self,
        base: Option<Mask>,
        verify: VerifyMode,
        control: &LoadControl,
    ) -> Result<(Mask, f64), PersistError> {
        control.check()?;
        let mut body = &self.bytes[self.offset..];
        let mut verify_ms = 0.0;
        let ledger = match (&self.header.kind, base) {
            (LedgerFileKind::Full, None) => {
                read_full_timed(&mut body, &self.header, verify, &mut verify_ms, control)?
            }
            (LedgerFileKind::Diff { .. }, Some(base)) => read_diff_onto_timed(
                &mut body,
                &self.header,
                base,
                verify,
                &mut verify_ms,
                control,
            )?,
            _ => {
                return Err(PersistError::Invalid(format!(
                    "{}: a diff needs its base, a full file none",
                    self.name
                )))
            }
        };
        if !body.is_empty() {
            return Err(PersistError::corrupt(&self.name, "bytes after the end"));
        }
        Ok((ledger, verify_ms))
    }
}

/// Ledger files read and checked, by ledger hash, not decoded yet.
#[derive(Default)]
pub struct LedgerFiles(BTreeMap<v2::LedgerHash, LedgerFileBytes>);

#[cfg(test)]
impl LedgerFiles {
    /// Files as [`FrontierStore::read_ledger_files`] would give them, from
    /// their bytes, by the ledger hash their headers name.
    pub(crate) fn from_bytes(files: Vec<Vec<u8>>) -> Self {
        Self(
            files
                .into_iter()
                .map(|bytes| {
                    let mut body = bytes.as_slice();
                    let header = read_header(&mut body).unwrap();
                    let offset = bytes.len() - body.len();
                    let hash = header.ledger_hash.clone();
                    let name = match header.kind {
                        LedgerFileKind::Full => ledger_full_name(&hash),
                        LedgerFileKind::Diff { .. } => ledger_diff_name(&hash),
                    };
                    let file = LedgerFileBytes {
                        name,
                        header,
                        bytes,
                        offset,
                    };
                    (hash, file)
                })
                .collect(),
        )
    }
}

/// Decodes and verifies ledger files by rounds: the full files first, then
/// in each round the diffs whose bases the rounds before loaded, the ledgers
/// of a round in parallel (I5d4: one after another, the three snarked
/// ledgers of a mainnet store took 6.5 to 8.8 s on a loaded machine). Each is
/// decoded and verified as it was when they were loaded one at a time; a
/// diff more than [`MAX_DIFF_CHAIN`] diffs above a full file, or above a
/// cycle of bases, is refused, as it was.
pub fn decode_ledgers(
    files: LedgerFiles,
    verify: VerifyMode,
) -> Result<BTreeMap<v2::LedgerHash, Mask>, PersistError> {
    decode_ledgers_timed(files, verify).map(|(ledgers, _, _)| ledgers)
}

/// As [`decode_ledgers`], with the sum of hash verification work across
/// ledger files. The sum can exceed wall time when independent files overlap.
pub fn decode_ledgers_timed(
    files: LedgerFiles,
    verify: VerifyMode,
) -> Result<(BTreeMap<v2::LedgerHash, Mask>, f64, f64), PersistError> {
    decode_ledgers_controlled(files, verify, &LoadControl::default())
}

pub(crate) fn decode_ledgers_controlled(
    files: LedgerFiles,
    verify: VerifyMode,
    control: &LoadControl,
) -> Result<(BTreeMap<v2::LedgerHash, Mask>, f64, f64), PersistError> {
    // Restore-only work. Freeze a base before copying it for a stored diff,
    // so the independent databases share accounts as bootstrap's masks do.
    // Respect both disk opt-outs; no new work runs during block production.
    let accounts_dir = ledger::database::disk::root_on_disk()
        .then(ledger::database::disk::global_dir)
        .flatten();
    decode_ledgers_with_disk(files, verify, control, accounts_dir)
}

pub(crate) fn decode_ledgers_with_disk(
    files: LedgerFiles,
    verify: VerifyMode,
    control: &LoadControl,
    accounts_dir: Option<&std::path::Path>,
) -> Result<(BTreeMap<v2::LedgerHash, Mask>, f64, f64), PersistError> {
    use rayon::prelude::*;

    let mut loaded = BTreeMap::new();
    let mut verify_ms = 0.0;
    let mut rebuild_ms = 0.0;
    let mut pending = files.0;
    let bases = pending
        .values()
        .filter_map(|file| match &file.header.kind {
            LedgerFileKind::Full => None,
            LedgerFileKind::Diff { base } => Some(base.clone()),
        })
        .collect::<BTreeSet<_>>();
    for _ in 0..=MAX_DIFF_CHAIN {
        control.check()?;
        if pending.is_empty() {
            break;
        }
        let (ready, rest): (BTreeMap<_, _>, BTreeMap<_, _>) =
            pending
                .into_iter()
                .partition(|(_, file): &(_, LedgerFileBytes)| match &file.header.kind {
                    LedgerFileKind::Full => true,
                    LedgerFileKind::Diff { base } => loaded.contains_key(base),
                });
        pending = rest;
        let jobs = ready
            .into_iter()
            .map(|(hash, file)| {
                control.check()?;
                let base = match &file.header.kind {
                    LedgerFileKind::Full => None,
                    LedgerFileKind::Diff { base } => Some(file.base_copy(&loaded[base])?),
                };
                Ok((hash, file, base))
            })
            .collect::<Result<Vec<_>, PersistError>>()?;
        let decoded = jobs
            .into_par_iter()
            .map(|(hash, file, base)| {
                let started = Instant::now();
                let (ledger, verify_ms) = file.decode_controlled(base, verify, control)?;
                let rebuild_ms = started.elapsed().as_secs_f64() * 1000.0 - verify_ms;
                control.completed(1)?;
                Ok((hash, ledger, verify_ms, rebuild_ms))
            })
            .collect::<Vec<Result<_, PersistError>>>();
        for result in decoded {
            let (hash, ledger, file_verify_ms, file_rebuild_ms) = result?;
            verify_ms += file_verify_ms;
            rebuild_ms += file_rebuild_ms;
            // Copying a frozen root shares its immutable file and retains
            // independent overlays and hash/index validation. Only bases that
            // are still in memory need a write; never refreeze a small diff.
            if let Some(dir) =
                accounts_dir.filter(|_| bases.contains(&hash) && ledger.disk_state().is_none())
            {
                control.check()?;
                if let Err(error) = ledger.freeze_to_disk(dir) {
                    mina_core::warn!(
                        mina_core::log::system_time();
                        kind = "LedgerFreezeFailed",
                        summary = "restored ledger base stays in memory after a failed freeze",
                        ledger = hash.to_string(),
                        error = error.to_string(),
                    );
                }
                control.check()?;
            }
            loaded.insert(hash, ledger);
        }
    }
    if let Some(hash) = pending.keys().next() {
        return Err(PersistError::corrupt(
            ledger_diff_name(hash),
            "the chain of diff bases is too long, or doesn't end in a full file",
        ));
    }
    Ok((loaded, verify_ms, rebuild_ms))
}

/// Counts and checksums the bytes read.
struct CrcRead {
    inner: io::Take<Box<dyn StoreReader>>,
    crc: crc32fast::Hasher,
    read: u64,
}

impl Read for CrcRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.crc.update(&buf[..n]);
        self.read += n as u64;
        Ok(n)
    }
}

/// A data file being read, buffered; [`Self::finish`] checks that it was
/// read to its end and that its checksum is the one recorded.
pub struct FileReader {
    reader: BufReader<CrcRead>,
    file: FileRef,
}

impl Read for FileReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buf)
    }
}

impl FileReader {
    pub fn finish(mut self) -> Result<(), PersistError> {
        let mut rest = [0u8; 1];
        if self.reader.read(&mut rest)? != 0 {
            return Err(PersistError::corrupt(
                self.file.name(),
                "bytes after the end",
            ));
        }
        let read = self.reader.get_ref();
        if read.read != self.file.len || read.crc.clone().finalize() != self.file.crc32 {
            return Err(PersistError::corrupt(self.file.name(), "checksum mismatch"));
        }
        Ok(())
    }
}
