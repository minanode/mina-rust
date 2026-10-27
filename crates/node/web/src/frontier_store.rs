//! The web node's frontier store (I5e): the store a native node keeps in a
//! directory (R20, I5a to I5d5), kept in the browser's Origin Private File
//! System (OPFS), so that a reload restores from it with the native checks
//! instead of downloading the ledgers again.
//!
//! The store thread is the native one, a web worker here, and its backend is
//! [`ProxyBackend`]: it sends each file call to the file server this module
//! starts on a worker of its own ([`start_file_server`]), and waits. The
//! server runs an async loop, as OPFS needs (opening a file, listing and
//! deleting files, taking the lock are promises), and holds the open files'
//! `FileSystemSyncAccessHandle`s, which only the worker that opened them may
//! use. See `mina_node::persist::proxy`.
//!
//! Each chain's store is the directory `mina-frontier/<chain id>` of the
//! origin's OPFS. A Web Lock named after it is held for the page's life: a
//! second tab runs without a store, and a reloaded page waits for the old
//! page's workers to let go ([`LOCK_WAIT_MS`]). A browser without OPFS or its
//! sync access handles (an old one, some private modes) fails the mount, and
//! the node runs without a store, as it did before.

use std::io;

use futures::future::{self, Either};
use js_sys::{Function, Object, Promise, Reflect, Uint8Array};
use mina_node::{
    core::{thread, ChainId},
    persist::{
        proxy::{self, AsyncDir, FsClient, SyncFile, CHUNK},
        writer::now_ms,
        ProxyBackend,
    },
};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    File, FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetDirectoryOptions,
    FileSystemGetFileOptions, FileSystemHandle, FileSystemHandleKind, FileSystemReadWriteOptions,
    FileSystemRemoveOptions, FileSystemSyncAccessHandle, StorageManager,
};

/// The directory, at the root of the origin's OPFS, of the stores: one per
/// chain id.
pub const STORES_DIR: &str = "mina-frontier";

/// How long a page waits for its store's lock: a reloaded page's old workers
/// let go of it in a moment; another tab holds it for good.
pub const LOCK_WAIT_MS: u32 = 10_000;

/// The store of `chain_id`, mounted on the file server, and its name for the
/// logs. Stores of other chains untouched for a week are deleted.
pub fn backend(client: &FsClient, chain_id: &ChainId) -> io::Result<(ProxyBackend, String)> {
    let path = vec![STORES_DIR.to_owned(), chain_id.to_hex()];
    let name = format!("opfs:/{}", path.join("/"));
    let (backend, swept) = ProxyBackend::mount(client.clone(), path, now_ms())?;
    if !swept.is_empty() {
        mina_node::core::info!(
            mina_node::core::log::system_time();
            kind = "PersistSwept",
            summary = "deleted the frontier stores of other chains, untouched for a week",
            stores = swept.join(", "),
        );
    }
    Ok((backend, name))
}

/// Starts the file server on a web worker of its own.
pub fn start_file_server() -> FsClient {
    let (client, calls) = FsClient::channel();
    thread::Builder::new()
        .name("frontier-store-io".into())
        .spawn(move || {
            wasm_bindgen_futures::spawn_local(proxy::serve(OpfsDir::new(), calls));
            crate::keep_worker_alive_cursed_hack();
        })
        .expect("Failed: frontier store file server");
    client
}

/// An error of a JavaScript call as an I/O error, its kind from the
/// `DOMException`'s name.
fn js_error(error: JsValue) -> io::Error {
    let field = |name: &str| {
        Reflect::get(&error, &name.into())
            .ok()
            .and_then(|value| value.as_string())
    };
    let name = field("name").unwrap_or_default();
    let message = field("message").unwrap_or_else(|| format!("{error:?}"));
    let kind = match name.as_str() {
        "NotFoundError" => io::ErrorKind::NotFound,
        "QuotaExceededError" => io::ErrorKind::QuotaExceeded,
        // A sync access handle another worker holds.
        "NoModificationAllowedError" => io::ErrorKind::ResourceBusy,
        "SecurityError" | "NotAllowedError" => io::ErrorKind::PermissionDenied,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, format!("{name}: {message}"))
}

async fn settled(promise: Promise) -> io::Result<JsValue> {
    JsFuture::from(promise).await.map_err(js_error)
}

fn get(target: &JsValue, name: &str) -> io::Result<JsValue> {
    Reflect::get(target, &name.into()).map_err(js_error)
}

fn unsupported(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("this browser has no {what}"),
    )
}

/// The origin's OPFS root, if this browser has OPFS and its sync access
/// handles (Chrome 102, Firefox 111, Safari 15.2 and later).
async fn opfs_root() -> io::Result<FileSystemDirectoryHandle> {
    let global = js_sys::global();
    let storage = get(&get(&global, "navigator")?, "storage")?;
    if storage.is_undefined() || !get(&storage, "getDirectory")?.is_function() {
        return Err(unsupported("Origin Private File System"));
    }
    let file_handle = get(&global, "FileSystemFileHandle")?;
    if file_handle.is_undefined()
        || !get(&get(&file_handle, "prototype")?, "createSyncAccessHandle")?.is_function()
    {
        return Err(unsupported(
            "synchronous access to the Origin Private File System",
        ));
    }
    let root = settled(storage.unchecked_into::<StorageManager>().get_directory()).await?;
    Ok(root.unchecked_into())
}

/// The handles in a directory.
async fn entries(dir: &FileSystemDirectoryHandle) -> io::Result<Vec<FileSystemHandle>> {
    let iterator = dir.values();
    let mut entries = Vec::new();
    loop {
        let next = settled(iterator.next().map_err(js_error)?).await?;
        let next = next.unchecked_into::<js_sys::IteratorNext>();
        if next.done() {
            return Ok(entries);
        }
        entries.push(next.value().unchecked_into());
    }
}

/// The newest modification time of the files in a directory.
async fn newest_file(dir: &FileSystemDirectoryHandle) -> io::Result<Option<u64>> {
    let mut newest = None;
    for entry in entries(dir).await? {
        if entry.kind() != FileSystemHandleKind::File {
            continue;
        }
        let file = settled(entry.unchecked_into::<FileSystemFileHandle>().get_file()).await?;
        let modified = file.unchecked_into::<File>().last_modified() as u64;
        newest = newest.max(Some(modified));
    }
    Ok(newest)
}

/// The OPFS side of the file server.
pub struct OpfsDir {
    /// The directory of the stores, and the mounted store's.
    parent: Option<FileSystemDirectoryHandle>,
    dir: Option<FileSystemDirectoryHandle>,
    name: String,
    lock_name: String,
    locked: bool,
    /// Bytes go to and from OPFS through this array, outside the WASM
    /// memory: a view of the shared memory needs a browser that takes
    /// `SharedArrayBuffer`s there, and the copy costs little next to the
    /// file's own I/O.
    scratch: Uint8Array,
}

impl Default for OpfsDir {
    fn default() -> Self {
        Self::new()
    }
}

impl OpfsDir {
    /// Made on the server's worker: it holds JavaScript objects.
    pub fn new() -> Self {
        Self {
            parent: None,
            dir: None,
            name: String::new(),
            lock_name: String::new(),
            locked: false,
            scratch: Uint8Array::new_with_length(CHUNK as u32),
        }
    }

    fn dir(&self) -> io::Result<&FileSystemDirectoryHandle> {
        self.dir
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "not mounted"))
    }

    fn parent(&self) -> io::Result<&FileSystemDirectoryHandle> {
        self.parent
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "not mounted"))
    }
}

impl AsyncDir for OpfsDir {
    type File = OpfsFile;

    async fn mount(&mut self, path: &[String]) -> io::Result<()> {
        self.parent = None;
        self.dir = None;
        let mut dir = opfs_root().await?;
        let mut parent = dir.clone();
        let options = FileSystemGetDirectoryOptions::new();
        options.set_create(true);
        for name in path {
            parent = dir;
            dir = settled(parent.get_directory_handle_with_options(name, &options))
                .await?
                .unchecked_into();
        }
        self.name = path.last().cloned().unwrap_or_default();
        self.lock_name = path.join("/");
        self.parent = Some(parent);
        self.dir = Some(dir);
        Ok(())
    }

    async fn siblings(&mut self) -> io::Result<Vec<(String, Option<u64>)>> {
        let mut siblings = Vec::new();
        for entry in entries(self.parent()?).await? {
            let name = entry.name();
            if entry.kind() != FileSystemHandleKind::Directory || name == self.name {
                continue;
            }
            let newest = newest_file(&entry.unchecked_into()).await?;
            siblings.push((name, newest));
        }
        Ok(siblings)
    }

    async fn remove_sibling(&mut self, name: &str) -> io::Result<()> {
        let options = FileSystemRemoveOptions::new();
        options.set_recursive(true);
        settled(self.parent()?.remove_entry_with_options(name, &options)).await?;
        Ok(())
    }

    /// A Web Lock (`navigator.locks`), requested with a signal that gives up
    /// after [`LOCK_WAIT_MS`], and held until this worker is gone: its
    /// callback returns a promise that never settles.
    async fn lock(&mut self) -> io::Result<()> {
        if self.locked {
            return Ok(());
        }
        let global = js_sys::global();
        let locks = get(&get(&global, "navigator")?, "locks")?;
        if locks.is_undefined() {
            // No Web Locks (every current browser has them): the files' own
            // exclusive handles still keep a second tab from writing.
            self.locked = true;
            return Ok(());
        }
        let options = Object::new();
        let abort_signal = get(&global, "AbortSignal")?;
        let timeout = if abort_signal.is_undefined() {
            JsValue::UNDEFINED
        } else {
            get(&abort_signal, "timeout")?
        };
        if let Some(timeout) = timeout.dyn_ref::<Function>() {
            let signal = timeout
                .call1(&abort_signal, &LOCK_WAIT_MS.into())
                .map_err(js_error)?;
            Reflect::set(&options, &"signal".into(), &signal).map_err(js_error)?;
        } else {
            Reflect::set(&options, &"ifAvailable".into(), &true.into()).map_err(js_error)?;
        }

        let (granted, on_grant) = futures::channel::oneshot::channel::<bool>();
        let callback = Closure::once_into_js(move |lock: JsValue| -> Promise {
            // `null` when not available (`ifAvailable`).
            let _ = granted.send(!lock.is_null());
            Promise::new(&mut |_, _| {})
        });
        let request = get(&locks, "request")?
            .dyn_into::<Function>()
            .map_err(js_error)?
            .call3(&locks, &self.lock_name.clone().into(), &options, &callback)
            .map_err(js_error)?
            .dyn_into::<Promise>()
            .map_err(js_error)?;
        let busy = || {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "another tab holds this store's lock",
            )
        };
        match future::select(on_grant, JsFuture::from(request)).await {
            Either::Left((Ok(true), _)) => {
                self.locked = true;
                Ok(())
            }
            Either::Left((Ok(false), _)) => Err(busy()),
            Either::Left((Err(_), _)) => Err(io::Error::other("the lock request was dropped")),
            Either::Right((Err(error), _)) => match js_error(error) {
                error if error.kind() == io::ErrorKind::Other => {
                    // The signal's abort: `AbortError`, or `TimeoutError`.
                    let message = error.to_string();
                    if message.starts_with("AbortError") || message.starts_with("TimeoutError") {
                        Err(busy())
                    } else {
                        Err(error)
                    }
                }
                error => Err(error),
            },
            // The request settles only after the lock is let go.
            Either::Right((Ok(_), _)) => Err(busy()),
        }
    }

    async fn list(&mut self) -> io::Result<Vec<String>> {
        Ok(entries(self.dir()?)
            .await?
            .into_iter()
            .filter(|entry| entry.kind() == FileSystemHandleKind::File)
            .map(|entry| entry.name())
            .collect())
    }

    async fn open(&mut self, name: &str, create: bool) -> io::Result<OpfsFile> {
        let options = FileSystemGetFileOptions::new();
        options.set_create(create);
        let file = settled(self.dir()?.get_file_handle_with_options(name, &options))
            .await?
            .unchecked_into::<FileSystemFileHandle>();
        let handle = settled(file.create_sync_access_handle())
            .await?
            .unchecked_into::<FileSystemSyncAccessHandle>();
        Ok(OpfsFile {
            handle,
            scratch: self.scratch.clone(),
        })
    }

    async fn remove(&mut self, name: &str) -> io::Result<()> {
        settled(self.dir()?.remove_entry(name)).await?;
        Ok(())
    }

    async fn sleep(&mut self, ms: u32) {
        gloo_timers::future::TimeoutFuture::new(ms).await;
    }
}

/// An open file: its sync access handle, closed when dropped.
pub struct OpfsFile {
    handle: FileSystemSyncAccessHandle,
    scratch: Uint8Array,
}

impl OpfsFile {
    fn at(at: u64) -> FileSystemReadWriteOptions {
        let options = FileSystemReadWriteOptions::new();
        options.set_at(at as f64);
        options
    }
}

impl SyncFile for OpfsFile {
    fn size(&mut self) -> io::Result<u64> {
        Ok(self.handle.get_size().map_err(js_error)? as u64)
    }

    fn read_at(&mut self, buf: &mut [u8], at: u64) -> io::Result<usize> {
        let len = buf.len().min(self.scratch.length() as usize);
        let view = self.scratch.subarray(0, len as u32);
        let read = self
            .handle
            .read_with_buffer_source_and_options(&view, &Self::at(at))
            .map_err(js_error)? as usize;
        let read = read.min(len);
        view.subarray(0, read as u32).copy_to(&mut buf[..read]);
        Ok(read)
    }

    fn write_at(&mut self, buf: &[u8], at: u64) -> io::Result<usize> {
        let len = buf.len().min(self.scratch.length() as usize);
        let view = self.scratch.subarray(0, len as u32);
        view.copy_from(&buf[..len]);
        let written = self
            .handle
            .write_with_buffer_source_and_options(&view, &Self::at(at))
            .map_err(js_error)?;
        Ok(written as usize)
    }

    fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.handle.truncate_with_f64(len as f64).map_err(js_error)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.handle.flush().map_err(js_error)
    }
}

impl Drop for OpfsFile {
    fn drop(&mut self) {
        self.handle.close();
    }
}
