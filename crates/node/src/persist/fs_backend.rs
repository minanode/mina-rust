//! The native backend: a directory, natively `<work_dir>/<chain_id_hex>/frontier/`
//! (R20 D2), locked with a file lock on `LOCK` for the session.
//!
//! Files are `sync_all`ed when finished, and the directory is synced before and
//! after a manifest slot is written, so that a manifest never names a file a
//! power loss could lose (D4).

use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

use mina_core::ChainId;

use super::{
    backend::{
        prepared_name, DetachedSlotWriter, StoreBackend, StoreReader, StoreWriter, WriteSum,
    },
    format::{is_valid_name, LOCK_FILE},
};

/// Write buffer per file. Ledger files are written in many small pieces.
const WRITE_BUFFER: usize = 1 << 20;

pub struct FsBackend {
    dir: PathBuf,
    lock: Option<File>,
}

/// Where a chain's store lives under the node's work directory.
pub fn store_dir(work_dir: &Path, chain_id: &ChainId) -> PathBuf {
    work_dir.join(chain_id.to_hex()).join("frontier")
}

impl FsBackend {
    /// Opens the directory, creating it if needed.
    pub fn new(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir, lock: None })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, name: &str) -> io::Result<PathBuf> {
        if !is_valid_name(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid file name {name:?}"),
            ));
        }
        Ok(self.dir.join(name))
    }
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

fn write_slot_at(dir: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    // The entries of the files the manifest is about to name.
    sync_dir(dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    // The slot's own entry, the first time it is written.
    sync_dir(dir)
}

impl StoreBackend for FsBackend {
    fn lock(&mut self) -> io::Result<()> {
        if self.lock.is_some() {
            return Ok(());
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path(LOCK_FILE)?)?;
        match file.try_lock() {
            Ok(()) => {
                self.lock = Some(file);
                Ok(())
            }
            Err(fs::TryLockError::WouldBlock) => Err(io::ErrorKind::WouldBlock.into()),
            Err(fs::TryLockError::Error(error)) => Err(error),
        }
    }

    fn list(&mut self) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            if let Ok(name) = entry.file_name().into_string() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    fn create(&mut self, name: &str) -> io::Result<Box<dyn StoreWriter>> {
        let file = File::create(self.path(name)?)?;
        Ok(Box::new(FsWriter {
            file: BufWriter::with_capacity(WRITE_BUFFER, file),
            sum: WriteSum::default(),
        }))
    }

    fn open(&mut self, name: &str) -> io::Result<Box<dyn StoreReader>> {
        let file = File::open(self.path(name)?)?;
        let len = file.metadata()?.len();
        Ok(Box::new(FsReader { file, len }))
    }

    fn delete(&mut self, name: &str) -> io::Result<()> {
        match fs::remove_file(self.path(name)?) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    fn write_slot(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let path = self.path(name)?;
        write_slot_at(&self.dir, &path, bytes)
    }

    fn prepare_slot(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let path = self.path(&prepared_name(name))?;
        let mut file = File::create(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    fn commit_slot(&mut self, name: &str) -> io::Result<()> {
        fs::rename(self.path(&prepared_name(name))?, self.path(name)?)
    }

    fn sync_slots(&mut self) -> io::Result<()> {
        sync_dir(&self.dir)
    }

    fn detached_slot_writer(&self) -> Option<DetachedSlotWriter> {
        let dir = self.dir.clone();
        Some(Box::new(move |name, bytes| {
            if !is_valid_name(name) {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            let path = dir.join(name);
            // The rename (or the unlink) is the change: done before any
            // flush, so that a stalled disk can't hold it back. The flushes
            // after it are for a power loss; a lost or empty record reads as
            // none, which counts no attempt.
            match bytes {
                Some(bytes) => {
                    let back = dir.join(format!("{name}.back"));
                    let mut file = File::create(&back)?;
                    file.write_all(bytes)?;
                    fs::rename(&back, &path)?;
                    file.sync_all()?;
                }
                None => match fs::remove_file(&path) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                    result => result?,
                },
            }
            sync_dir(&dir)
        }))
    }
}

struct FsWriter {
    file: BufWriter<File>,
    sum: WriteSum,
}

impl StoreWriter for FsWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        self.file.write_all(buf)?;
        self.sum.update(buf);
        Ok(())
    }

    fn finish(self: Box<Self>) -> io::Result<(u64, u32)> {
        let Self { file, sum } = *self;
        let file = file.into_inner().map_err(|error| error.into_error())?;
        file.sync_all()?;
        Ok(sum.finish())
    }
}

struct FsReader {
    file: File,
    len: u64,
}

impl io::Read for FsReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

impl StoreReader for FsReader {
    fn len(&self) -> u64 {
        self.len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RC7-FIXES review: a prepared slot changes nothing until committed;
    /// the commit and the take-back each replace the file by one rename.
    #[test]
    fn rc7_slots_are_prepared_beside_and_committed_by_rename() {
        let dir = tempfile::tempdir().unwrap();
        let mut backend = FsBackend::new(dir.path()).unwrap();
        let path = dir.path().join("ATTEMPTS");
        backend.write_slot("ATTEMPTS", b"one").unwrap();
        backend.prepare_slot("ATTEMPTS", b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one");
        backend.commit_slot("ATTEMPTS").unwrap();
        backend.sync_slots().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert!(!dir.path().join("ATTEMPTS.new").exists());

        // A reader holding the committed file keeps its bytes: the
        // take-back puts a new file in place rather than rewriting it.
        let held = File::open(&path).unwrap();
        backend.detached_slot_writer().unwrap()("ATTEMPTS", Some(b"one")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one");
        assert_eq!(std::io::read_to_string(held).unwrap(), "two");
        backend.detached_slot_writer().unwrap()("ATTEMPTS", None).unwrap();
        assert!(!path.exists());
        backend.detached_slot_writer().unwrap()("ATTEMPTS", None).unwrap();
        assert!(backend.detached_slot_writer().unwrap()("../x", None).is_err());
    }
}
