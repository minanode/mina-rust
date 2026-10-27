//! A backend whose files another thread serves: the web node's (I5e).
//!
//! In the browser a store's files are in the Origin Private File System
//! (OPFS). Opening a file, listing or deleting files and taking the store's
//! lock answer with promises, and a thread can only see a promise settle by
//! returning to its event loop. The store thread can't: it runs a job to its
//! end, and the loader blocks in rayon. So its backend, [`ProxyBackend`],
//! sends each call to a file server that runs an async loop on a web worker
//! of its own ([`serve`]), and blocks until the server answers, as the ledger
//! thread's callers block on it. Reading and writing an open file (a
//! `FileSystemSyncAccessHandle`) are synchronous, but only on the worker that
//! opened it, so they go to the server too, in pieces of up to [`CHUNK`]
//! bytes. The bytes move between the threads in `Vec`s of the shared WASM
//! memory, not through JavaScript.
//!
//! [`serve`] is generic over the file system, [`AsyncDir`]: in the browser
//! OPFS (`mina-node-web`'s `frontier_store`), in the tests `MemDir`, which
//! can fail as OPFS does (a full quota, a file still open in a worker of the
//! page before a reload, a browser without OPFS, another tab's lock).
//!
//! The server also keeps the stores of other chains from piling up (R20 D6):
//! when it mounts a store, it deletes the stores beside it whose files are all
//! older than [`SWEEP_AFTER_MS`]. A browser has no flag to delete them, and a
//! hard fork would otherwise leave a store behind in every visitor's OPFS. It
//! doesn't delete them sooner, so that a devnet and a mainnet web node served
//! from one origin don't delete each other's.

use std::{collections::BTreeMap, io, sync::mpsc as std_mpsc};

use mina_core::channels::mpsc;

use super::{
    backend::{StoreBackend, StoreReader, StoreWriter, WriteSum},
    format::is_valid_name,
};

/// The most bytes one read or write call carries between the threads.
pub const CHUNK: usize = 4 << 20;

/// Other chains' stores whose newest file is older than this are deleted
/// when a store is mounted beside them (R20 D6): 7 days.
pub const SWEEP_AFTER_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// How often the server tries to open a file that another worker still holds
/// open ([`io::ErrorKind::ResourceBusy`]), waiting [`OPEN_RETRY_MS`] times
/// the attempt between tries: after a reload, the old page's workers may hold
/// their sync access handles for a moment (SQLite's OPFS VFS retries so too).
pub const OPEN_ATTEMPTS: u32 = 6;
pub const OPEN_RETRY_MS: u32 = 100;

pub type FileId = u64;

/// A call to the file server. The paths are of the mounted store.
#[derive(Debug)]
pub enum FsRequest {
    /// Makes the directory `path` (created if missing) the store the other
    /// calls are about, and deletes the stores beside it that are older than
    /// [`SWEEP_AFTER_MS`] at `now_ms`. Answers [`FsReply::Names`]: those
    /// deleted.
    Mount {
        path: Vec<String>,
        now_ms: u64,
    },
    /// Takes the store's lock for the server's life ([`StoreBackend::lock`]).
    Lock,
    List,
    /// Creates a file, or truncates the file of that name, and keeps it open.
    Create {
        name: String,
    },
    Open {
        name: String,
    },
    Delete {
        name: String,
    },
    WriteSlot {
        name: String,
        bytes: Vec<u8>,
    },
    /// Writes `data` at `at`; answers with `data`, for the next call.
    Write {
        file: FileId,
        at: u64,
        data: Vec<u8>,
    },
    /// Reads up to `len` bytes at `at` into `buf` (its content replaced);
    /// fewer only at the end of the file.
    Read {
        file: FileId,
        at: u64,
        len: usize,
        buf: Vec<u8>,
    },
    /// Flushes and closes a written file.
    Finish {
        file: FileId,
    },
    /// Closes a file without flushing: a reader done, or a writer given up.
    Close {
        file: FileId,
    },
}

#[derive(Debug)]
pub enum FsReply {
    Done,
    Names(Vec<String>),
    Opened { file: FileId, len: u64 },
    Bytes(Vec<u8>),
}

pub struct FsCall {
    pub request: FsRequest,
    /// `None` for a call nobody waits for ([`FsRequest::Close`]).
    pub reply: Option<std_mpsc::SyncSender<io::Result<FsReply>>>,
}

/// The store thread's end of the file server.
#[derive(Clone)]
pub struct FsClient {
    calls: mpsc::UnboundedSender<FsCall>,
}

fn server_gone() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "the store's file server stopped")
}

fn unexpected(reply: FsReply) -> io::Error {
    io::Error::other(format!("unexpected file server answer {reply:?}"))
}

impl FsClient {
    /// A client, and the calls it sends, for [`serve`].
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<FsCall>) {
        let (calls, receiver) = mpsc::unbounded_channel();
        (Self { calls }, receiver)
    }

    /// Sends a call and blocks until the server answers it.
    pub fn call(&self, request: FsRequest) -> io::Result<FsReply> {
        let (reply, answer) = std_mpsc::sync_channel(1);
        self.calls
            .send(FsCall {
                request,
                reply: Some(reply),
            })
            .map_err(|_| server_gone())?;
        answer.recv().map_err(|_| server_gone())?
    }

    /// Sends a call nobody waits for.
    fn send(&self, request: FsRequest) {
        let _ = self.calls.send(FsCall {
            request,
            reply: None,
        });
    }

    fn done(&self, request: FsRequest) -> io::Result<()> {
        match self.call(request)? {
            FsReply::Done => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    fn opened(&self, request: FsRequest) -> io::Result<(FileId, u64)> {
        match self.call(request)? {
            FsReply::Opened { file, len } => Ok((file, len)),
            other => Err(unexpected(other)),
        }
    }

    fn bytes(&self, request: FsRequest) -> io::Result<Vec<u8>> {
        match self.call(request)? {
            FsReply::Bytes(bytes) => Ok(bytes),
            other => Err(unexpected(other)),
        }
    }

    fn names(&self, request: FsRequest) -> io::Result<Vec<String>> {
        match self.call(request)? {
            FsReply::Names(names) => Ok(names),
            other => Err(unexpected(other)),
        }
    }
}

/// A [`StoreBackend`] over a file server. Unlike a native backend it is
/// `Send`: it holds no JavaScript handle, only a channel to the worker that
/// holds them.
pub struct ProxyBackend {
    client: FsClient,
}

impl ProxyBackend {
    /// Mounts the store at `path` on the server. Returns the backend and the
    /// stores the server deleted beside it ([`SWEEP_AFTER_MS`]).
    pub fn mount(
        client: FsClient,
        path: Vec<String>,
        now_ms: u64,
    ) -> io::Result<(Self, Vec<String>)> {
        let swept = client.names(FsRequest::Mount { path, now_ms })?;
        Ok((Self { client }, swept))
    }
}

fn check_name(name: &str) -> io::Result<()> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid file name {name:?}"),
        ))
    }
}

impl StoreBackend for ProxyBackend {
    fn lock(&mut self) -> io::Result<()> {
        self.client.done(FsRequest::Lock)
    }

    fn list(&mut self) -> io::Result<Vec<String>> {
        let mut names = self.client.names(FsRequest::List)?;
        names.sort();
        Ok(names)
    }

    fn create(&mut self, name: &str) -> io::Result<Box<dyn StoreWriter>> {
        check_name(name)?;
        let (file, _) = self.client.opened(FsRequest::Create {
            name: name.to_owned(),
        })?;
        Ok(Box::new(ProxyWriter {
            client: self.client.clone(),
            file,
            at: 0,
            buf: Vec::with_capacity(CHUNK),
            sum: WriteSum::default(),
            open: true,
        }))
    }

    fn open(&mut self, name: &str) -> io::Result<Box<dyn StoreReader>> {
        check_name(name)?;
        let (file, len) = self.client.opened(FsRequest::Open {
            name: name.to_owned(),
        })?;
        Ok(Box::new(ProxyReader {
            client: self.client.clone(),
            file,
            len,
            at: 0,
            chunk: Vec::new(),
            pos: 0,
        }))
    }

    fn delete(&mut self, name: &str) -> io::Result<()> {
        check_name(name)?;
        self.client.done(FsRequest::Delete {
            name: name.to_owned(),
        })
    }

    fn write_slot(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        check_name(name)?;
        self.client.done(FsRequest::WriteSlot {
            name: name.to_owned(),
            bytes: bytes.to_vec(),
        })
    }
}

/// Writes in pieces of [`CHUNK`] bytes, each sent once it is full.
struct ProxyWriter {
    client: FsClient,
    file: FileId,
    /// Where the next piece goes.
    at: u64,
    buf: Vec<u8>,
    sum: WriteSum,
    /// Until finished: dropped open, it is closed without a flush.
    open: bool,
}

impl ProxyWriter {
    fn send(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.buf);
        let len = data.len() as u64;
        let mut data = self.client.bytes(FsRequest::Write {
            file: self.file,
            at: self.at,
            data,
        })?;
        self.at += len;
        data.clear();
        self.buf = data;
        Ok(())
    }
}

impl StoreWriter for ProxyWriter {
    fn write(&mut self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let take = (CHUNK - self.buf.len()).min(buf.len());
            self.buf.extend_from_slice(&buf[..take]);
            self.sum.update(&buf[..take]);
            buf = &buf[take..];
            if self.buf.len() == CHUNK {
                self.send()?;
            }
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> io::Result<(u64, u32)> {
        self.send()?;
        self.open = false;
        self.client.done(FsRequest::Finish { file: self.file })?;
        Ok(std::mem::take(&mut self.sum).finish())
    }
}

impl Drop for ProxyWriter {
    fn drop(&mut self) {
        if self.open {
            self.client.send(FsRequest::Close { file: self.file });
        }
    }
}

/// Reads in pieces of [`CHUNK`] bytes.
struct ProxyReader {
    client: FsClient,
    file: FileId,
    /// The file's length when it was opened.
    len: u64,
    /// Where the next piece is read from.
    at: u64,
    chunk: Vec<u8>,
    /// How much of `chunk` was read.
    pos: usize,
}

impl io::Read for ProxyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.pos == self.chunk.len() {
            let chunk = self.client.bytes(FsRequest::Read {
                file: self.file,
                at: self.at,
                len: CHUNK,
                buf: std::mem::take(&mut self.chunk),
            })?;
            self.at += chunk.len() as u64;
            self.chunk = chunk;
            self.pos = 0;
        }
        let n = (self.chunk.len() - self.pos).min(buf.len());
        buf[..n].copy_from_slice(&self.chunk[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

impl StoreReader for ProxyReader {
    fn len(&self) -> u64 {
        self.len
    }
}

impl Drop for ProxyReader {
    fn drop(&mut self) {
        self.client.send(FsRequest::Close { file: self.file });
    }
}

/// The file system a server serves. Its calls that may wait are async (they
/// are promises in the browser); those on an open file are not.
#[allow(async_fn_in_trait)]
pub trait AsyncDir {
    type File: SyncFile;

    /// Makes the directory `path` (created if missing) the one the other
    /// calls are about.
    async fn mount(&mut self, path: &[String]) -> io::Result<()>;
    /// The directories beside the mounted one, each with the newest
    /// modification time of its files (milliseconds since the Unix epoch;
    /// `None` without files).
    async fn siblings(&mut self) -> io::Result<Vec<(String, Option<u64>)>>;
    /// Deletes a directory beside the mounted one, with what it holds.
    async fn remove_sibling(&mut self, name: &str) -> io::Result<()>;
    /// Takes the mounted directory's lock for as long as this lives;
    /// [`io::ErrorKind::WouldBlock`] if another holder keeps it.
    async fn lock(&mut self) -> io::Result<()>;
    /// The names of the files (not directories) in the mounted directory.
    async fn list(&mut self) -> io::Result<Vec<String>>;
    /// Opens a file for exclusive reading and writing, creating it if
    /// `create`. [`io::ErrorKind::ResourceBusy`] while another holder has it
    /// open; [`io::ErrorKind::NotFound`] if it isn't there.
    async fn open(&mut self, name: &str, create: bool) -> io::Result<Self::File>;
    /// Deletes a file; [`io::ErrorKind::NotFound`] if it isn't there.
    async fn remove(&mut self, name: &str) -> io::Result<()>;
    async fn sleep(&mut self, ms: u32);
}

/// An open file. Closed when dropped.
pub trait SyncFile {
    fn size(&mut self) -> io::Result<u64>;
    /// Reads at `at`; fewer bytes than `buf` holds only at the end.
    fn read_at(&mut self, buf: &mut [u8], at: u64) -> io::Result<usize>;
    fn write_at(&mut self, buf: &[u8], at: u64) -> io::Result<usize>;
    fn truncate(&mut self, len: u64) -> io::Result<()>;
    fn flush(&mut self) -> io::Result<()>;
}

struct Server<D: AsyncDir> {
    dir: D,
    mounted: bool,
    files: BTreeMap<FileId, (String, D::File)>,
    next_file: FileId,
}

fn not_mounted() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "no store is mounted")
}

fn no_file(file: FileId) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("no open file {file}"))
}

fn write_all_at<F: SyncFile>(file: &mut F, mut buf: &[u8], mut at: u64) -> io::Result<()> {
    while !buf.is_empty() {
        match file.write_at(buf, at)? {
            0 => return Err(io::ErrorKind::WriteZero.into()),
            n => {
                buf = &buf[n..];
                at += n as u64;
            }
        }
    }
    Ok(())
}

fn read_up_to<F: SyncFile>(file: &mut F, buf: &mut [u8], mut at: u64) -> io::Result<usize> {
    let mut read = 0;
    while read < buf.len() {
        match file.read_at(&mut buf[read..], at)? {
            0 => break,
            n => {
                read += n;
                at += n as u64;
            }
        }
    }
    Ok(read)
}

impl<D: AsyncDir> Server<D> {
    async fn handle(&mut self, request: FsRequest) -> io::Result<FsReply> {
        if let FsRequest::Mount { path, now_ms } = request {
            return self.mount(&path, now_ms).await.map(FsReply::Names);
        }
        if !self.mounted {
            return Err(not_mounted());
        }
        match request {
            FsRequest::Mount { .. } => unreachable!("handled above"),
            FsRequest::Lock => self.dir.lock().await.map(|()| FsReply::Done),
            FsRequest::List => self.dir.list().await.map(FsReply::Names),
            FsRequest::Create { name } => {
                let mut file = self.open(&name, true).await?;
                file.truncate(0)?;
                Ok(self.keep(name, file, 0))
            }
            FsRequest::Open { name } => {
                let mut file = self.open(&name, false).await?;
                let len = file.size()?;
                Ok(self.keep(name, file, len))
            }
            FsRequest::Delete { name } => {
                self.check_closed(&name)?;
                match self.dir.remove(&name).await {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(FsReply::Done),
                    result => result.map(|()| FsReply::Done),
                }
            }
            FsRequest::WriteSlot { name, bytes } => {
                let mut file = self.open(&name, true).await?;
                file.truncate(0)?;
                write_all_at(&mut file, &bytes, 0)?;
                file.flush()?;
                Ok(FsReply::Done)
            }
            FsRequest::Write { file, at, data } => {
                let (_, open) = self.files.get_mut(&file).ok_or_else(|| no_file(file))?;
                write_all_at(open, &data, at)?;
                Ok(FsReply::Bytes(data))
            }
            FsRequest::Read {
                file,
                at,
                len,
                mut buf,
            } => {
                let (_, open) = self.files.get_mut(&file).ok_or_else(|| no_file(file))?;
                buf.resize(len, 0);
                let read = read_up_to(open, &mut buf, at)?;
                buf.truncate(read);
                Ok(FsReply::Bytes(buf))
            }
            FsRequest::Finish { file } => {
                let (_, mut open) = self.files.remove(&file).ok_or_else(|| no_file(file))?;
                open.flush()?;
                Ok(FsReply::Done)
            }
            FsRequest::Close { file } => {
                self.files.remove(&file);
                Ok(FsReply::Done)
            }
        }
    }

    async fn mount(&mut self, path: &[String], now_ms: u64) -> io::Result<Vec<String>> {
        // A new store: what the old one had open goes.
        self.files.clear();
        self.mounted = false;
        self.dir.mount(path).await?;
        self.mounted = true;
        // Best effort: a store that can't be swept now is swept another
        // time, and doesn't keep this one from opening. An empty directory
        // is left: it costs nothing, and may be a store just mounted.
        let mut swept = Vec::new();
        for (name, newest) in self.dir.siblings().await.unwrap_or_default() {
            let old = newest.is_some_and(|newest| newest.saturating_add(SWEEP_AFTER_MS) < now_ms);
            if old && self.dir.remove_sibling(&name).await.is_ok() {
                swept.push(name);
            }
        }
        Ok(swept)
    }

    fn check_closed(&self, name: &str) -> io::Result<()> {
        if self.files.values().any(|(open, _)| open == name) {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                format!("{name} is open"),
            ));
        }
        Ok(())
    }

    /// Opens a file, retrying while another worker still holds it.
    async fn open(&mut self, name: &str, create: bool) -> io::Result<D::File> {
        self.check_closed(name)?;
        let mut attempt = 1;
        loop {
            match self.dir.open(name, create).await {
                Err(error)
                    if error.kind() == io::ErrorKind::ResourceBusy && attempt < OPEN_ATTEMPTS =>
                {
                    self.dir.sleep(OPEN_RETRY_MS * attempt).await;
                    attempt += 1;
                }
                result => return result,
            }
        }
    }

    fn keep(&mut self, name: String, file: D::File, len: u64) -> FsReply {
        let id = self.next_file;
        self.next_file += 1;
        self.files.insert(id, (name, file));
        FsReply::Opened { file: id, len }
    }
}

/// Serves `calls` from `dir`, one at a time in the order they come, until
/// every client is gone.
pub async fn serve<D: AsyncDir>(dir: D, mut calls: mpsc::UnboundedReceiver<FsCall>) {
    let mut server = Server {
        dir,
        mounted: false,
        files: BTreeMap::new(),
        next_file: 0,
    };
    while let Some(FsCall { request, reply }) = calls.recv().await {
        let result = server.handle(request).await;
        if let Some(reply) = reply {
            let _ = reply.send(result);
        }
    }
}

/// Runs a future to its end on this thread, for a server whose file system
/// never waits on anything but its calls ([`MemDir`]).
#[cfg(test)]
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::{
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
        thread::{self, Thread},
    };

    struct Unpark(Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park(),
        }
    }
}

#[cfg(test)]
pub use mem::{MemDir, MemFs};

/// An in-memory [`AsyncDir`], for the tests, that fails as OPFS does.
#[cfg(test)]
mod mem {
    use std::{
        collections::{BTreeMap, BTreeSet},
        io,
        sync::{Arc, Mutex, MutexGuard},
    };

    use super::{AsyncDir, SyncFile};

    #[derive(Default)]
    struct State {
        /// Files by path (`dir/name`), with their modification time.
        files: BTreeMap<String, (Vec<u8>, u64)>,
        /// Directories, by path.
        dirs: BTreeSet<String>,
        /// Files open, by path: OPFS's exclusive sync access handles.
        open: BTreeSet<String>,
        /// Locked directories, by path, and the holder.
        locks: BTreeMap<String, u64>,
        next_holder: u64,
        now_ms: u64,
        /// Bytes all files together may hold.
        quota: Option<u64>,
        /// Opens of a path that fail as busy before one succeeds.
        busy: BTreeMap<String, u32>,
        /// Mounts fail so: a browser without OPFS.
        mount_error: Option<io::ErrorKind>,
        sleeps: Vec<u32>,
    }

    /// The file system: clones share it, as the workers of one origin share
    /// its OPFS.
    #[derive(Clone, Default)]
    pub struct MemFs(Arc<Mutex<State>>);

    fn join(dir: &str, name: &str) -> String {
        format!("{dir}/{name}")
    }

    impl MemFs {
        pub fn new() -> Self {
            Self::default()
        }

        fn state(&self) -> MutexGuard<'_, State> {
            self.0.lock().unwrap()
        }

        /// A file system client, as a worker gets one.
        pub fn dir(&self) -> MemDir {
            let holder = {
                let mut state = self.state();
                state.next_holder += 1;
                state.next_holder
            };
            MemDir {
                fs: self.clone(),
                holder,
                mounted: None,
            }
        }

        pub fn set_now_ms(&self, now_ms: u64) {
            self.state().now_ms = now_ms;
        }

        pub fn set_quota(&self, quota: Option<u64>) {
            self.state().quota = quota;
        }

        pub fn set_busy(&self, path: &str, opens: u32) {
            self.state().busy.insert(path.to_owned(), opens);
        }

        pub fn set_mount_error(&self, kind: Option<io::ErrorKind>) {
            self.state().mount_error = kind;
        }

        pub fn sleeps(&self) -> Vec<u32> {
            self.state().sleeps.clone()
        }

        /// Releases every lock and open file, as the browser does when a
        /// page's workers are gone.
        pub fn release_all(&self) {
            let mut state = self.state();
            state.locks.clear();
            state.open.clear();
        }

        /// The files under `dir`, by name.
        pub fn files(&self, dir: &str) -> BTreeMap<String, Vec<u8>> {
            let prefix = format!("{dir}/");
            self.state()
                .files
                .iter()
                .filter_map(|(path, (bytes, _))| {
                    let name = path.strip_prefix(&prefix)?;
                    (!name.contains('/')).then(|| (name.to_owned(), bytes.clone()))
                })
                .collect()
        }

        /// Puts a file, creating its directory, at the current time.
        pub fn put(&self, dir: &str, name: &str, bytes: Vec<u8>) {
            let mut state = self.state();
            let mut path = String::new();
            for part in dir.split('/') {
                if !path.is_empty() {
                    path.push('/');
                }
                path.push_str(part);
                state.dirs.insert(path.clone());
            }
            let now = state.now_ms;
            state.files.insert(join(dir, name), (bytes, now));
        }

        pub fn dirs(&self) -> BTreeSet<String> {
            self.state().dirs.clone()
        }

        pub fn is_open(&self, dir: &str, name: &str) -> bool {
            self.state().open.contains(&join(dir, name))
        }

        fn used(state: &State) -> u64 {
            state
                .files
                .values()
                .map(|(bytes, _)| bytes.len() as u64)
                .sum()
        }
    }

    /// One worker's view of a [`MemFs`]: its lock holder and mounted
    /// directory.
    pub struct MemDir {
        fs: MemFs,
        holder: u64,
        mounted: Option<String>,
    }

    impl MemDir {
        fn mounted(&self) -> io::Result<&str> {
            self.mounted
                .as_deref()
                .ok_or_else(|| io::Error::other("not mounted"))
        }

        fn parent(&self) -> io::Result<&str> {
            let mounted = self.mounted()?;
            Ok(mounted.rsplit_once('/').map_or("", |(parent, _)| parent))
        }
    }

    pub struct MemFile {
        fs: MemFs,
        path: String,
    }

    impl Drop for MemFile {
        fn drop(&mut self) {
            self.fs.state().open.remove(&self.path);
        }
    }

    impl AsyncDir for MemDir {
        type File = MemFile;

        async fn mount(&mut self, path: &[String]) -> io::Result<()> {
            if let Some(kind) = self.fs.state().mount_error {
                return Err(io::Error::new(kind, "no file system"));
            }
            let mut state = self.fs.state();
            let mut dir = String::new();
            for part in path {
                if !dir.is_empty() {
                    dir.push('/');
                }
                dir.push_str(part);
                state.dirs.insert(dir.clone());
            }
            self.mounted = Some(dir);
            Ok(())
        }

        async fn siblings(&mut self) -> io::Result<Vec<(String, Option<u64>)>> {
            let parent = self.parent()?.to_owned();
            let mounted = self.mounted()?.to_owned();
            let prefix = if parent.is_empty() {
                String::new()
            } else {
                format!("{parent}/")
            };
            let state = self.fs.state();
            let mut siblings = Vec::new();
            for dir in &state.dirs {
                let Some(name) = dir.strip_prefix(&prefix) else {
                    continue;
                };
                if name.contains('/') || dir == &mounted || name.is_empty() {
                    continue;
                }
                let inside = format!("{dir}/");
                let newest = state
                    .files
                    .iter()
                    .filter(|(path, _)| path.starts_with(&inside))
                    .map(|(_, (_, time))| *time)
                    .max();
                siblings.push((name.to_owned(), newest));
            }
            Ok(siblings)
        }

        async fn remove_sibling(&mut self, name: &str) -> io::Result<()> {
            let dir = match self.parent()? {
                "" => name.to_owned(),
                parent => join(parent, name),
            };
            let inside = format!("{dir}/");
            let mut state = self.fs.state();
            if state.open.iter().any(|path| path.starts_with(&inside)) {
                return Err(io::Error::new(
                    io::ErrorKind::ResourceBusy,
                    "a file is open",
                ));
            }
            state.files.retain(|path, _| !path.starts_with(&inside));
            state
                .dirs
                .retain(|path| path != &dir && !path.starts_with(&inside));
            Ok(())
        }

        async fn lock(&mut self) -> io::Result<()> {
            let dir = self.mounted()?.to_owned();
            let mut state = self.fs.state();
            match state.locks.get(&dir) {
                Some(holder) if *holder != self.holder => Err(io::ErrorKind::WouldBlock.into()),
                _ => {
                    state.locks.insert(dir, self.holder);
                    Ok(())
                }
            }
        }

        async fn list(&mut self) -> io::Result<Vec<String>> {
            let dir = self.mounted()?.to_owned();
            Ok(self.fs.files(&dir).into_keys().collect())
        }

        async fn open(&mut self, name: &str, create: bool) -> io::Result<MemFile> {
            let path = join(self.mounted()?, name);
            let mut state = self.fs.state();
            if let Some(busy) = state.busy.get_mut(&path) {
                if *busy > 0 {
                    *busy -= 1;
                    return Err(io::Error::new(
                        io::ErrorKind::ResourceBusy,
                        "held elsewhere",
                    ));
                }
            }
            if state.open.contains(&path) {
                return Err(io::Error::new(
                    io::ErrorKind::ResourceBusy,
                    "held elsewhere",
                ));
            }
            if !state.files.contains_key(&path) {
                if !create {
                    return Err(io::ErrorKind::NotFound.into());
                }
                let now = state.now_ms;
                state.files.insert(path.clone(), (Vec::new(), now));
            }
            state.open.insert(path.clone());
            Ok(MemFile {
                fs: self.fs.clone(),
                path,
            })
        }

        async fn remove(&mut self, name: &str) -> io::Result<()> {
            let path = join(self.mounted()?, name);
            let mut state = self.fs.state();
            if state.open.contains(&path) {
                return Err(io::Error::new(io::ErrorKind::ResourceBusy, "open"));
            }
            match state.files.remove(&path) {
                Some(_) => Ok(()),
                None => Err(io::ErrorKind::NotFound.into()),
            }
        }

        async fn sleep(&mut self, ms: u32) {
            self.fs.state().sleeps.push(ms);
        }
    }

    impl SyncFile for MemFile {
        fn size(&mut self) -> io::Result<u64> {
            let state = self.fs.state();
            Ok(state
                .files
                .get(&self.path)
                .map_or(0, |(bytes, _)| bytes.len() as u64))
        }

        fn read_at(&mut self, buf: &mut [u8], at: u64) -> io::Result<usize> {
            let state = self.fs.state();
            let bytes = state
                .files
                .get(&self.path)
                .map(|(bytes, _)| bytes.as_slice())
                .unwrap_or_default();
            let start = (at as usize).min(bytes.len());
            let n = (bytes.len() - start).min(buf.len());
            buf[..n].copy_from_slice(&bytes[start..start + n]);
            Ok(n)
        }

        fn write_at(&mut self, buf: &[u8], at: u64) -> io::Result<usize> {
            let mut state = self.fs.state();
            let used = MemFs::used(&state);
            let now = state.now_ms;
            let quota = state.quota;
            let (bytes, time) = state.files.entry(self.path.clone()).or_default();
            let end = at as usize + buf.len();
            let grows = end.saturating_sub(bytes.len()) as u64;
            if quota.is_some_and(|quota| used + grows > quota) {
                // OPFS writes nothing of a call over the quota.
                return Err(io::Error::new(
                    io::ErrorKind::QuotaExceeded,
                    "QuotaExceededError",
                ));
            }
            if bytes.len() < end {
                bytes.resize(end, 0);
            }
            bytes[at as usize..end].copy_from_slice(buf);
            *time = now;
            Ok(buf.len())
        }

        fn truncate(&mut self, len: u64) -> io::Result<()> {
            let mut state = self.fs.state();
            let now = state.now_ms;
            let (bytes, time) = state.files.entry(self.path.clone()).or_default();
            bytes.resize(len as usize, 0);
            *time = now;
            Ok(())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
