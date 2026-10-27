//! Where a store's files live (R20 D3): a flat directory of named files, with
//! a lock for the session.
//!
//! Natively that's [`super::FsBackend`]; in the browser [`super::ProxyBackend`],
//! whose files a file server keeps in the Origin Private File System (I5e).
//! [`MemBackend`] keeps the files in memory, and can fail any call, or a write
//! part way, to test that a crash anywhere leaves a loadable store.

use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Mutex, MutexGuard},
};

/// The storage a [`super::FrontierStore`] runs on. Not `Send`: a browser
/// backend holds JavaScript handles, so it is made on the thread that uses it.
pub trait StoreBackend: 'static {
    /// Takes the store's lock for as long as the backend lives. Fails with
    /// [`io::ErrorKind::WouldBlock`] if another process holds it.
    fn lock(&mut self) -> io::Result<()>;
    /// The names of all files.
    fn list(&mut self) -> io::Result<Vec<String>>;
    /// Creates a file, replacing any file of that name.
    fn create(&mut self, name: &str) -> io::Result<Box<dyn StoreWriter>>;
    fn open(&mut self, name: &str) -> io::Result<Box<dyn StoreReader>>;
    /// Deletes a file; deleting a missing file is not an error.
    fn delete(&mut self, name: &str) -> io::Result<()>;
    /// Replaces a small file (a manifest slot): truncate, write, flush. Natively
    /// it also makes every file written before durable, so that a manifest
    /// never names a file a power loss could lose.
    fn write_slot(&mut self, name: &str, bytes: &[u8]) -> io::Result<()>;
    /// Writes and flushes the new bytes of small file `name` beside it,
    /// leaving the file as it is until [`Self::commit_slot`] (RC7-FIXES: the
    /// restore attempts record, whose commit a shutdown must be able to
    /// order against). The default writes the file in place at once.
    fn prepare_slot(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        self.write_slot(name, bytes)
    }
    /// Puts what [`Self::prepare_slot`] wrote in place, atomically and
    /// without flushing, so that it is quick under a lock.
    fn commit_slot(&mut self, _name: &str) -> io::Result<()> {
        Ok(())
    }
    /// Makes committed slots durable.
    fn sync_slots(&mut self) -> io::Result<()> {
        Ok(())
    }
    /// Replaces (`Some`) or deletes (`None`) a small file from another thread
    /// while the backend is busy, atomically: a clean shutdown during a
    /// restore takes its attempt back with it (RC7-FIXES). `None` if the
    /// backend can't.
    fn detached_slot_writer(&self) -> Option<DetachedSlotWriter> {
        None
    }
}

/// See [`StoreBackend::detached_slot_writer`].
pub type DetachedSlotWriter = Box<dyn FnOnce(&str, Option<&[u8]>) -> io::Result<()> + Send>;

/// Where [`StoreBackend::prepare_slot`] writes a slot's new bytes.
pub fn prepared_name(name: &str) -> String {
    format!("{name}.new")
}

pub trait StoreWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<()>;
    /// Flushes the file to stable storage and returns its length and CRC32.
    fn finish(self: Box<Self>) -> io::Result<(u64, u32)>;
}

pub trait StoreReader: io::Read {
    fn len(&self) -> u64;
}

/// Length and CRC32 of the bytes written so far, for backends' writers.
#[derive(Default)]
pub struct WriteSum {
    len: u64,
    crc: crc32fast::Hasher,
}

impl WriteSum {
    pub fn update(&mut self, buf: &[u8]) {
        self.len += buf.len() as u64;
        self.crc.update(buf);
    }

    pub fn finish(self) -> (u64, u32) {
        (self.len, self.crc.finalize())
    }
}

/// Adapts a [`StoreWriter`] to [`io::Write`].
pub struct WriteAdapter<'a>(pub &'a mut dyn StoreWriter);

impl io::Write for WriteAdapter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// When a [`MemBackend`] fails. After the first failure every call fails, as
/// if the process had died there; [`MemBackend::reopen`] is the restart.
#[derive(Debug, Clone, Copy, Default)]
pub struct Faults {
    /// Fail the call with this number, counting every call from 0: `lock`,
    /// `list`, `create`, `open`, `delete`, `write_slot`, and the writers'
    /// `write` and `finish`.
    pub fail_at_call: Option<u64>,
    /// Fail the write that would take the bytes written by writers and
    /// `write_slot` together past this many, after writing the bytes up to it:
    /// a torn write.
    pub fail_at_byte: Option<u64>,
}

#[derive(Default)]
struct MemState {
    files: BTreeMap<String, Vec<u8>>,
    lock_holder: Option<u64>,
    next_handle: u64,
    faults: Faults,
    calls: u64,
    bytes: u64,
    crashed: bool,
    read_chunk: Option<usize>,
    #[cfg(test)]
    before_read: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Files in memory. Clones share the files: a test keeps one to inspect them.
pub struct MemBackend {
    state: Arc<Mutex<MemState>>,
    handle: u64,
}

fn injected() -> io::Error {
    io::Error::other("injected failure")
}

impl Default for MemBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MemBackend {
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(MemState::default()));
        let handle = Self::next_handle(&state);
        Self { state, handle }
    }

    fn next_handle(state: &Arc<Mutex<MemState>>) -> u64 {
        let mut state = state.lock().unwrap();
        state.next_handle += 1;
        state.next_handle
    }

    fn state(&self) -> MutexGuard<'_, MemState> {
        self.state.lock().unwrap()
    }

    /// Another handle on the same files, as a restarted process gets: the
    /// lock is free (its holder died), faults are cleared and calls work again.
    pub fn reopen(&self) -> Self {
        {
            let mut state = self.state();
            state.lock_holder = None;
            state.faults = Faults::default();
            state.crashed = false;
            state.calls = 0;
            state.bytes = 0;
        }
        Self {
            state: self.state.clone(),
            handle: Self::next_handle(&self.state),
        }
    }

    /// Another handle on the same files that does nothing to the lock or the
    /// faults, as a second process would be.
    pub fn second_handle(&self) -> Self {
        Self {
            state: self.state.clone(),
            handle: Self::next_handle(&self.state),
        }
    }

    pub fn set_faults(&self, faults: Faults) {
        let mut state = self.state();
        state.faults = faults;
        state.calls = 0;
        state.bytes = 0;
        state.crashed = false;
    }

    /// Readers return at most this many bytes per `read`.
    pub fn set_read_chunk(&self, chunk: Option<usize>) {
        self.state().read_chunk = chunk;
    }

    #[cfg(test)]
    pub(crate) fn set_before_read(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        self.state().before_read = Some(hook);
    }

    /// Calls and bytes written since the faults were last set.
    pub fn counters(&self) -> (u64, u64) {
        let state = self.state();
        (state.calls, state.bytes)
    }

    pub fn files(&self) -> BTreeMap<String, Vec<u8>> {
        self.state().files.clone()
    }

    pub fn set_files(&self, files: BTreeMap<String, Vec<u8>>) {
        self.state().files = files;
    }

    pub fn file(&self, name: &str) -> Option<Vec<u8>> {
        self.state().files.get(name).cloned()
    }

    pub fn put_file(&self, name: &str, bytes: Vec<u8>) {
        self.state().files.insert(name.to_owned(), bytes);
    }

    fn call(state: &Arc<Mutex<MemState>>) -> io::Result<()> {
        let mut state = state.lock().unwrap();
        if state.crashed {
            return Err(injected());
        }
        let call = state.calls;
        state.calls += 1;
        if state.faults.fail_at_call == Some(call) {
            state.crashed = true;
            return Err(injected());
        }
        Ok(())
    }

    /// Counts a write of `len` bytes, or the `Err` with how many of them may
    /// be written before it fails.
    fn allowed_bytes(state: &mut MemState, len: usize) -> Result<(), usize> {
        if let Some(limit) = state.faults.fail_at_byte {
            if state.bytes + len as u64 > limit {
                let allowed = limit.saturating_sub(state.bytes) as usize;
                state.bytes += allowed as u64;
                state.crashed = true;
                return Err(allowed);
            }
        }
        state.bytes += len as u64;
        Ok(())
    }
}

impl StoreBackend for MemBackend {
    fn lock(&mut self) -> io::Result<()> {
        Self::call(&self.state)?;
        let mut state = self.state();
        match state.lock_holder {
            Some(holder) if holder != self.handle => Err(io::ErrorKind::WouldBlock.into()),
            _ => {
                state.lock_holder = Some(self.handle);
                Ok(())
            }
        }
    }

    fn list(&mut self) -> io::Result<Vec<String>> {
        Self::call(&self.state)?;
        Ok(self.state().files.keys().cloned().collect())
    }

    fn create(&mut self, name: &str) -> io::Result<Box<dyn StoreWriter>> {
        Self::call(&self.state)?;
        self.state().files.insert(name.to_owned(), Vec::new());
        Ok(Box::new(MemWriter {
            state: self.state.clone(),
            name: name.to_owned(),
            sum: WriteSum::default(),
        }))
    }

    fn open(&mut self, name: &str) -> io::Result<Box<dyn StoreReader>> {
        Self::call(&self.state)?;
        let state = self.state();
        let bytes = state
            .files
            .get(name)
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        Ok(Box::new(MemReader {
            len: bytes.len() as u64,
            bytes: io::Cursor::new(bytes),
            chunk: state.read_chunk,
            #[cfg(test)]
            before_read: state.before_read.clone(),
        }))
    }

    fn delete(&mut self, name: &str) -> io::Result<()> {
        Self::call(&self.state)?;
        self.state().files.remove(name);
        Ok(())
    }

    fn write_slot(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        Self::call(&self.state)?;
        let mut state = self.state();
        match Self::allowed_bytes(&mut state, bytes.len()) {
            Ok(()) => {
                state.files.insert(name.to_owned(), bytes.to_vec());
                Ok(())
            }
            Err(allowed) => {
                // Truncated, then torn part way.
                state
                    .files
                    .insert(name.to_owned(), bytes[..allowed].to_vec());
                Err(injected())
            }
        }
    }

    fn prepare_slot(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        // One call and its bytes, as `write_slot`, for the fault plans.
        self.write_slot(&prepared_name(name), bytes)
    }

    fn commit_slot(&mut self, name: &str) -> io::Result<()> {
        let mut state = self.state();
        let bytes = state
            .files
            .remove(&prepared_name(name))
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        state.files.insert(name.to_owned(), bytes);
        Ok(())
    }

    fn detached_slot_writer(&self) -> Option<DetachedSlotWriter> {
        let state = self.state.clone();
        Some(Box::new(move |name, bytes| {
            let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
            match bytes {
                Some(bytes) => state.files.insert(name.to_owned(), bytes.to_vec()),
                None => state.files.remove(name),
            };
            Ok(())
        }))
    }
}

struct MemWriter {
    state: Arc<Mutex<MemState>>,
    name: String,
    sum: WriteSum,
}

impl StoreWriter for MemWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        MemBackend::call(&self.state)?;
        let mut state = self.state.lock().unwrap();
        let result = MemBackend::allowed_bytes(&mut state, buf.len());
        let written = match result {
            Ok(()) => buf,
            Err(allowed) => &buf[..allowed],
        };
        state
            .files
            .entry(self.name.clone())
            .or_default()
            .extend_from_slice(written);
        drop(state);
        self.sum.update(written);
        result.map_err(|_| injected())
    }

    fn finish(self: Box<Self>) -> io::Result<(u64, u32)> {
        MemBackend::call(&self.state)?;
        Ok(self.sum.finish())
    }
}

struct MemReader {
    bytes: io::Cursor<Vec<u8>>,
    len: u64,
    chunk: Option<usize>,
    #[cfg(test)]
    before_read: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl io::Read for MemReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        #[cfg(test)]
        if let Some(hook) = &self.before_read {
            hook();
        }
        let len = match self.chunk {
            Some(chunk) => buf.len().min(chunk),
            None => buf.len(),
        };
        self.bytes.read(&mut buf[..len])
    }
}

impl StoreReader for MemReader {
    fn len(&self) -> u64 {
        self.len
    }
}
