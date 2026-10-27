//! PC1: a store for proofs kept out of the heap, as OCaml's proof cache does.
//!
//! OCaml 4.0.2 writes a proof it keeps (in the snark pool, the scan state, the
//! frontier) to an on-disk cache, LMDB or one file per proof, and keeps only a
//! tag in memory; the cache frees an entry when the garbage collector frees
//! its tag. Here a [`CachedBlob`] is the tag: the binprot bytes of one proof,
//! in a file of the store's directory, freed when the last clone of the blob
//! is dropped. A store without a directory (the web node, tests, tools) keeps
//! the bytes in memory: compact, one allocation instead of a decoded proof's
//! ninety or so, but still on the heap. A blob whose file cannot be written
//! keeps its bytes in memory too.
//!
//! The files belong to one process: a store empties its directory when it
//! opens it, and nothing in them outlives the node.
//!
//! A file is trusted no more than the disk under it. The blob keeps, in
//! memory, the SHA-256 digest of the bytes it wrote, which are the encoding of
//! a proof the node verified, and every read checks the file against it. A
//! file that is missing, of another length, or of other bytes (corrupted, or
//! another valid proof swapped in) fails the read, and the blob is unreadable
//! from then on: its work is skipped wherever it would be used, and the snark
//! pool drops it. It is never trusted again, and never a panic.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/proof_cache_tag/proof_cache_tag.ml#L1-L38>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/disk_cache/filesystem/disk_cache.ml#L9-L42>

use std::{
    borrow::Cow,
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, OnceLock,
    },
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Where cached proofs go. Cloning it is cheap.
#[derive(Clone)]
pub struct ProofStore(Arc<StoreInner>);

struct StoreInner {
    /// `None` keeps blobs in memory.
    dir: Option<PathBuf>,
    next_id: AtomicU64,
    stats: Counters,
}

#[derive(Default)]
struct Counters {
    files: AtomicU64,
    file_bytes: AtomicU64,
    memory: AtomicU64,
    memory_bytes: AtomicU64,
    reads: AtomicU64,
    errors: AtomicU64,
    unreadable: AtomicU64,
}

/// What a store holds now, and how it was used.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct ProofStoreStats {
    pub dir: Option<String>,
    /// Blobs in files, and their bytes.
    pub files: u64,
    pub file_bytes: u64,
    /// Blobs in memory, and their bytes.
    pub memory: u64,
    pub memory_bytes: u64,
    /// File reads, and failed writes, reads or removals.
    pub reads: u64,
    pub errors: u64,
    /// Blobs that failed a read (missing file, other length, other bytes)
    /// and are no longer used.
    pub unreadable: u64,
}

impl ProofStore {
    /// A store that keeps blobs in memory.
    pub fn memory() -> Self {
        Self(Arc::new(StoreInner {
            dir: None,
            next_id: AtomicU64::new(0),
            stats: Counters::default(),
        }))
    }

    /// A store whose blobs are files in `dir`. It makes the directory, and
    /// removes the files a previous process left there, as OCaml clears its
    /// cache directory. It refuses a `dir` that is a symbolic link or not a
    /// directory, and removes only regular files named as it names its own
    /// (`<number>.proof`), so nothing outside the directory is touched.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/disk_cache/utils/disk_cache_utils.ml#L8-L33>
    pub fn dir(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        match std::fs::symlink_metadata(&dir) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is a symbolic link", dir.display()),
                ));
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not a directory", dir.display()),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => std::fs::create_dir_all(&dir)?,
            Err(error) => return Err(error),
        }
        let dir = dir.canonicalize()?;
        remove_stale_files(&dir)?;
        Ok(Self(Arc::new(StoreInner {
            dir: Some(dir),
            next_id: AtomicU64::new(0),
            stats: Counters::default(),
        })))
    }

    pub fn is_memory(&self) -> bool {
        self.0.dir.is_none()
    }

    fn path(&self, id: u64) -> Option<PathBuf> {
        Some(self.0.dir.as_ref()?.join(format!("{id}.proof")))
    }

    /// Stores `bytes`: in a file if this store has a directory and the write
    /// works, in memory otherwise.
    pub fn put(&self, bytes: Vec<u8>) -> CachedBlob {
        let stats = &self.0.stats;
        let len = bytes.len() as u64;
        if let Some(dir) = &self.0.dir {
            let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!("{id}.proof"));
            // A new file only: an entry already there (another process's, or
            // a link) is neither followed nor overwritten.
            let written = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map(|mut file| {
                    let result = std::io::Write::write_all(&mut file, &bytes);
                    if result.is_err() {
                        let _ = std::fs::remove_file(&path);
                    }
                    result
                });
            match written {
                Ok(Ok(())) => {
                    stats.files.fetch_add(1, Ordering::Relaxed);
                    stats.file_bytes.fetch_add(len, Ordering::Relaxed);
                    return CachedBlob(Arc::new(BlobInner {
                        store: self.clone(),
                        place: Place::File {
                            id,
                            len,
                            digest: Sha256::digest(&bytes).into(),
                        },
                        unreadable: AtomicBool::new(false),
                    }));
                }
                Ok(Err(_)) | Err(_) => {
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        stats.memory.fetch_add(1, Ordering::Relaxed);
        stats.memory_bytes.fetch_add(len, Ordering::Relaxed);
        CachedBlob(Arc::new(BlobInner {
            store: self.clone(),
            place: Place::Memory(bytes.into_boxed_slice()),
            unreadable: AtomicBool::new(false),
        }))
    }

    pub fn stats(&self) -> ProofStoreStats {
        let s = &self.0.stats;
        ProofStoreStats {
            dir: self.0.dir.as_deref().map(|d| d.display().to_string()),
            files: s.files.load(Ordering::Relaxed),
            file_bytes: s.file_bytes.load(Ordering::Relaxed),
            memory: s.memory.load(Ordering::Relaxed),
            memory_bytes: s.memory_bytes.load(Ordering::Relaxed),
            reads: s.reads.load(Ordering::Relaxed),
            errors: s.errors.load(Ordering::Relaxed),
            unreadable: s.unreadable.load(Ordering::Relaxed),
        }
    }
}

static GLOBAL: OnceLock<ProofStore> = OnceLock::new();

/// Sets the store [`global`] returns: once, at node start, before any proof
/// is cached. A later call changes nothing and returns false.
pub fn set_global(store: ProofStore) -> bool {
    GLOBAL.set(store).is_ok()
}

/// The node's store, or a memory store if none was set.
pub fn global() -> &'static ProofStore {
    GLOBAL.get_or_init(ProofStore::memory)
}

/// Opens `dir` as the node's store. If it cannot be opened, the node keeps
/// cached proofs in memory, and the error says why.
pub fn set_global_dir(dir: &Path) -> io::Result<()> {
    let store = ProofStore::dir(dir)?;
    set_global(store);
    Ok(())
}

/// Removes the files a previous process's store left in `dir`: regular
/// files named `<number>.proof`. Links, directories and other names stay.
fn remove_stale_files(dir: &Path) -> io::Result<u64> {
    let mut removed = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        // `file_type` does not follow a link.
        if entry.file_type()?.is_file() && is_blob_name(&entry.file_name()) {
            std::fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Whether `name` is one a store gives its files: `<id>.proof`.
fn is_blob_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_suffix(".proof"))
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
}

/// The binprot bytes of one proof, in a store. Cloning it is cheap; the last
/// clone dropped frees its file.
#[derive(Clone)]
pub struct CachedBlob(Arc<BlobInner>);

struct BlobInner {
    store: ProofStore,
    place: Place,
    /// Set by the first failed read; the blob is not read again.
    unreadable: AtomicBool,
}

enum Place {
    /// `digest` is the SHA-256 of the bytes written: a read must give them
    /// back.
    File {
        id: u64,
        len: u64,
        digest: [u8; 32],
    },
    Memory(Box<[u8]>),
}

impl CachedBlob {
    /// The bytes, read from the file if they are in one, and checked against
    /// the length and digest of the bytes written. A failed read makes the
    /// blob unreadable for good.
    pub fn bytes(&self) -> io::Result<Cow<'_, [u8]>> {
        let (id, len, digest) = match &self.0.place {
            Place::Memory(bytes) => return Ok(Cow::Borrowed(bytes)),
            Place::File { id, len, digest } => (*id, *len, digest),
        };
        if self.is_unreadable() {
            return Err(io::Error::other(format!(
                "cached proof {id} failed an earlier read"
            )));
        }
        let store = &self.0.store;
        store.0.stats.reads.fetch_add(1, Ordering::Relaxed);
        let read = store
            .path(id)
            .ok_or_else(|| io::Error::other("proof store has no directory"))
            .and_then(|path| read_regular_file(&path, id, len))
            .and_then(|bytes| {
                if bytes.len() as u64 != len {
                    Err(io::Error::other(format!(
                        "cached proof {id} has {} bytes, not {len}",
                        bytes.len()
                    )))
                } else if Sha256::digest(&bytes).as_slice() != digest {
                    Err(io::Error::other(format!(
                        "cached proof {id} is not the bytes written: digest mismatch"
                    )))
                } else {
                    Ok(bytes)
                }
            });
        read.map(Cow::Owned)
            .inspect_err(|error| self.mark_unreadable(error))
    }

    fn mark_unreadable(&self, error: &io::Error) {
        let stats = &self.0.store.0.stats;
        stats.errors.fetch_add(1, Ordering::Relaxed);
        if !self.0.unreadable.swap(true, Ordering::Relaxed) {
            stats.unreadable.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("proof cache: {error}; the proof is no longer used");
        }
    }

    /// Whether a read of this blob failed: its proof is not to be used.
    pub fn is_unreadable(&self) -> bool {
        self.0.unreadable.load(Ordering::Relaxed)
    }

    pub fn len(&self) -> u64 {
        match &self.0.place {
            Place::File { len, .. } => *len,
            Place::Memory(bytes) => bytes.len() as u64,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the bytes are in a file rather than on the heap.
    pub fn is_file(&self) -> bool {
        matches!(self.0.place, Place::File { .. })
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Heap bytes this blob holds (its bytes when in memory), shared clones
    /// counted once by the caller.
    pub fn heap_bytes(&self) -> usize {
        match &self.0.place {
            Place::File { .. } => 0,
            Place::Memory(bytes) => bytes.len(),
        }
    }

    pub fn as_ptr(&self) -> *const () {
        Arc::as_ptr(&self.0) as *const ()
    }
}

/// Reads a proof's file only after checking what it is. A nonregular file
/// in place of the proof, such as a FIFO or a device a directory writer
/// swapped in, would make [`std::fs::read`] block or never end; a regular
/// file of another length is refused without reading it, so a file far
/// larger than the proof written cannot exhaust memory. What the file
/// holds is still checked against the digest of the bytes written.
fn read_regular_file(path: &Path, id: u64, len: u64) -> io::Result<Vec<u8>> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::other(format!(
            "cached proof {id} is not a regular file"
        )));
    }
    if metadata.len() != len {
        return Err(io::Error::other(format!(
            "cached proof {id} has {} bytes, not {len}",
            metadata.len()
        )));
    }
    std::fs::read(path)
}

impl Drop for BlobInner {
    fn drop(&mut self) {
        let stats = &self.store.0.stats;
        match &self.place {
            Place::File { id, len, .. } => {
                if let Some(path) = self.store.path(*id) {
                    if std::fs::remove_file(path).is_err() {
                        stats.errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
                stats.files.fetch_sub(1, Ordering::Relaxed);
                stats.file_bytes.fetch_sub(*len, Ordering::Relaxed);
            }
            Place::Memory(bytes) => {
                stats.memory.fetch_sub(1, Ordering::Relaxed);
                stats
                    .memory_bytes
                    .fetch_sub(bytes.len() as u64, Ordering::Relaxed);
            }
        }
    }
}

impl std::fmt::Debug for CachedBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0.place {
            Place::File { id, len, .. } => write!(f, "CachedBlob(file {id}, {len} bytes)"),
            Place::Memory(bytes) => write!(f, "CachedBlob(memory, {} bytes)", bytes.len()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mina-proof-store-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn file_blob_reads_back_and_its_last_clone_removes_the_file() {
        let dir = scratch("file");
        let store = ProofStore::dir(&dir).unwrap();
        let blob = store.put(vec![1, 2, 3]);
        assert!(blob.is_file());
        assert_eq!(&*blob.bytes().unwrap(), &[1, 2, 3]);
        assert_eq!(store.stats().files, 1);
        assert_eq!(store.stats().file_bytes, 3);
        let clone = blob.clone();
        drop(blob);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert_eq!(&*clone.bytes().unwrap(), &[1, 2, 3]);
        drop(clone);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert_eq!(store.stats().files, 0);
        assert_eq!(store.stats().reads, 2);
        assert_eq!(store.stats().errors, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opening_a_store_removes_a_previous_process_files() {
        let dir = scratch("reopen");
        let first = ProofStore::dir(&dir).unwrap();
        let blob = first.put(vec![7; 10]);
        std::mem::forget(blob);
        std::fs::write(dir.join("keep.txt"), b"x").unwrap();
        let _second = ProofStore::dir(&dir).unwrap();
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("keep.txt")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// BP-SAFE: a store refuses a cache directory that is a symbolic link,
    /// so opening it can't remove the files of the directory it points to.
    #[test]
    fn a_symlinked_cache_directory_is_refused_and_its_target_untouched() {
        let root = scratch("symlinked-dir");
        let target = root.join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("3.proof"), b"not ours").unwrap();
        let link = root.join("proof-cache");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let error = ProofStore::dir(&link).err().expect("a symlinked directory");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read(target.join("3.proof")).unwrap(), b"not ours");
        // Not a directory either.
        std::fs::write(root.join("file"), b"x").unwrap();
        assert!(ProofStore::dir(root.join("file")).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// BP-SAFE: opening a store removes only regular files it could have
    /// written. A linked entry is neither removed, followed nor overwritten.
    #[test]
    fn opening_a_store_leaves_links_and_other_names_alone() {
        let root = scratch("foreign-entries");
        let dir = root.join("proof-cache");
        let outside = root.join("outside.proof");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        std::fs::write(dir.join("12.proof"), b"stale").unwrap();
        std::fs::write(dir.join("notes.proof"), b"x").unwrap();
        std::fs::create_dir(dir.join("4.proof")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("0.proof")).unwrap();
        let store = ProofStore::dir(&dir).unwrap();
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["0.proof", "4.proof", "notes.proof"]);
        // The first blob's name is the link's: it stays in memory instead.
        let blob = store.put(vec![9; 16]);
        assert!(!blob.is_file());
        assert_eq!(&*blob.bytes().unwrap(), &[9; 16]);
        drop(blob);
        assert!(std::fs::symlink_metadata(dir.join("0.proof"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
        let blob = store.put(vec![1; 8]);
        assert!(blob.is_file());
        drop(blob);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        let dir = scratch("missing");
        let store = ProofStore::dir(&dir).unwrap();
        let blob = store.put(vec![5; 4]);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(blob.bytes().is_err());
        assert!(blob.is_unreadable());
        drop(blob);
        assert!(store.stats().errors >= 2);
        assert_eq!(store.stats().unreadable, 1);
    }

    #[test]
    fn other_bytes_of_the_same_length_fail_the_read_for_good() {
        let dir = scratch("swapped");
        let store = ProofStore::dir(&dir).unwrap();
        let blob = store.put(vec![1, 2, 3, 4]);
        let path = dir.join("0.proof");
        std::fs::write(&path, [1, 2, 3, 5]).unwrap();
        let error = blob.bytes().unwrap_err();
        assert!(error.to_string().contains("digest mismatch"), "{error}");
        assert!(blob.is_unreadable());
        // Fail closed: the right bytes back do not make it trusted again.
        std::fs::write(&path, [1, 2, 3, 4]).unwrap();
        assert!(blob.bytes().is_err());
        let stats = store.stats();
        assert_eq!((stats.unreadable, stats.reads), (1, 1));
        drop(blob);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_of_another_length_fails_the_read() {
        let dir = scratch("length");
        let store = ProofStore::dir(&dir).unwrap();
        let blob = store.put(vec![1, 2, 3, 4]);
        std::fs::write(dir.join("0.proof"), [1, 2, 3]).unwrap();
        assert!(blob.bytes().unwrap_err().to_string().contains("3 bytes"));
        assert!(blob.is_unreadable());
        drop(blob);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A directory writer can put a FIFO or a device in place of the file.
    /// The store must refuse it from its metadata, not by reading it: a FIFO
    /// would block the read forever, a device could never end. A symlink to
    /// a device stands in for the FIFO case without hanging the test.
    #[cfg(unix)]
    #[test]
    fn a_nonregular_file_in_place_of_the_proof_is_refused_unread() {
        let dir = scratch("nonregular");
        let store = ProofStore::dir(&dir).unwrap();
        let blob = store.put(vec![1, 2, 3, 4]);
        let path = dir.join("0.proof");
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("/dev/null", &path).unwrap();
        let error = blob.bytes().unwrap_err();
        assert!(error.to_string().contains("not a regular file"), "{error}");
        assert!(blob.is_unreadable());
        // Fail closed, as for every other read failure.
        assert!(blob.bytes().is_err());
        // Dropping the blob removes the symlink, not its target.
        drop(blob);
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn memory_store_keeps_bytes_on_the_heap() {
        let store = ProofStore::memory();
        let blob = store.put(vec![9; 5]);
        assert!(!blob.is_file());
        assert_eq!(blob.heap_bytes(), 5);
        assert_eq!(&*blob.bytes().unwrap(), &[9; 5]);
        assert_eq!(store.stats().memory, 1);
        drop(blob);
        assert_eq!(store.stats().memory, 0);
    }
}
