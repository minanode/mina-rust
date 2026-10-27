//! Shared proof handles. Caching releases the decoded value for all holders.
//!
//! Binprot and serde always encode the proof, never a process-local cache tag.
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/ledger_proof/ledger_proof.ml#L35-L60>

use std::{
    fmt, io,
    mem::size_of,
    sync::{Arc, Mutex, MutexGuard},
};

use binprot::{BinProtRead, BinProtWrite};
use malloc_size_of::{MallocSizeOf, MallocSizeOfOps};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::proof_cache::{CachedBlob, ProofStore};

pub struct ProofRef<T>(Arc<Mutex<Repr<T>>>);

enum Repr<T> {
    Decoded(Arc<T>),
    Cached(CachedBlob),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cached proof unavailable: {0}")]
pub struct ProofUnavailable(pub String);

impl<T> Clone for ProofRef<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> From<T> for ProofRef<T> {
    fn from(value: T) -> Self {
        Self::from(Arc::new(value))
    }
}

impl<T> From<Arc<T>> for ProofRef<T> {
    fn from(value: Arc<T>) -> Self {
        Self(Arc::new(Mutex::new(Repr::Decoded(value))))
    }
}

impl<T> ProofRef<T> {
    fn lock(&self) -> MutexGuard<'_, Repr<T>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn as_ptr(&self) -> *const () {
        Arc::as_ptr(&self.0).cast()
    }
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Snapshot for diagnostics; never reads a cache file.
    pub fn decoded(&self) -> Option<Arc<T>> {
        match &*self.lock() {
            Repr::Decoded(p) => Some(p.clone()),
            _ => None,
        }
    }

    pub fn cached_blob(&self) -> Option<CachedBlob> {
        match &*self.lock() {
            Repr::Cached(b) => Some(b.clone()),
            _ => None,
        }
    }

    pub fn is_cached(&self) -> bool {
        self.cached_blob().is_some()
    }
    pub fn is_unreadable(&self) -> bool {
        self.cached_blob().is_some_and(|b| b.is_unreadable())
    }
}

impl<T: BinProtRead> ProofRef<T> {
    /// A temporary decoded value. A cached read always checks the digest;
    /// decoded values are not retained by this handle after the call.
    pub fn get(&self) -> Result<Arc<T>, ProofUnavailable> {
        let blob = match &*self.lock() {
            Repr::Decoded(p) => return Ok(p.clone()),
            Repr::Cached(b) => b.clone(),
        };
        let bytes = blob.bytes().map_err(|e| ProofUnavailable(e.to_string()))?;
        T::binprot_read(&mut &bytes[..])
            .map(Arc::new)
            .map_err(|e| ProofUnavailable(e.to_string()))
    }
}

impl<T: BinProtWrite> ProofRef<T> {
    /// Cache once, after verification and job hashing. Every clone sees the
    /// change. Failed writes use the store's compact memory fallback.
    pub fn cache_in(&self, store: &ProofStore) -> io::Result<()> {
        let mut repr = self.lock();
        if let Repr::Decoded(p) = &*repr {
            let mut bytes = Vec::with_capacity(12 * 1024);
            p.binprot_write(&mut bytes)?;
            *repr = Repr::Cached(store.put(bytes));
        }
        Ok(())
    }

    pub fn cache(&self) -> io::Result<()> {
        self.cache_in(crate::proof_cache::global())
    }
}

impl<T: BinProtWrite> BinProtWrite for ProofRef<T> {
    fn binprot_write<W: io::Write>(&self, w: &mut W) -> io::Result<()> {
        let blob = match &*self.lock() {
            Repr::Decoded(p) => return p.binprot_write(w),
            Repr::Cached(b) => b.clone(),
        };
        w.write_all(&blob.bytes()?)
    }
}

impl<T: BinProtRead> BinProtRead for ProofRef<T> {
    fn binprot_read<R: io::Read + ?Sized>(r: &mut R) -> Result<Self, binprot::Error> {
        T::binprot_read(r).map(Self::from)
    }
}

impl<T: BinProtRead + Serialize> Serialize for ProofRef<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.get()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ProofRef<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::from)
    }
}

impl<T> fmt::Debug for ProofRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &*self.lock() {
            Repr::Decoded(_) => f.write_str("ProofRef(decoded)"),
            Repr::Cached(b) => b.fmt(f),
        }
    }
}

impl<T: BinProtRead + PartialEq> PartialEq for ProofRef<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self.get(), other.get()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}

impl<T: MallocSizeOf> MallocSizeOf for ProofRef<T> {
    fn size_of(&self, ops: &mut MallocSizeOfOps) -> usize {
        if ops.have_seen_ptr(self.as_ptr()) {
            return 0;
        }
        size_of::<Mutex<Repr<T>>>()
            + match &*self.lock() {
                Repr::Decoded(p) => size_of::<T>() + p.size_of(ops),
                Repr::Cached(b) => b.heap_bytes(),
            }
    }
}
