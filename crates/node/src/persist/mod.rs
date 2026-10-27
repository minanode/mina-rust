//! The frontier store: what a node keeps on disk so that a restart a few
//! minutes later doesn't bootstrap again.
//!
//! The persistence design was built in steps
//! I5a to I5h. I5a: the formats, the ledger codec, the Merkle diff, the store
//! logic with its garbage collection and version rules, and the storage
//! backends. I5c: the store thread ([`writer`]) the node writes through, its
//! events ([`event`]) and the offline checker ([`check`]); the node's side is
//! `transition_frontier::persist` and the ledger thread's `ledger_persist`.
//! I5d1: the loader ([`load`]) and its policy, which a restart restores from.
//!
//! - [`format`]: the manifest (the store's only commit point), file names and
//!   the restore attempts file.
//! - [`ledger_codec`]: ledger files, full or as a diff over another ledger,
//!   written and read in chunks, checked against the ledger hash on load.
//! - [`diff`]: the Merkle diff between two ledgers, found by comparing their
//!   hash matrices from the root, without hashing.
//! - [`store`]: [`FrontierStore`], which writes data files, then the manifest,
//!   and deletes what no manifest references.
//! - [`backend`]: the storage interface and an in-memory backend, which can
//!   inject failures for the crash tests; [`fs_backend`]: the native one;
//!   [`proxy`]: the web node's (I5e), whose files a file server on another
//!   worker keeps in the browser's Origin Private File System.
//!
//! All data files are written once, named by what they hold, and never
//! changed. The manifest is double-buffered and checksummed, so a crash at any
//! point leaves the old or the new generation loadable (D4).

pub mod backend;
#[cfg(not(target_family = "wasm"))]
pub mod check;
pub mod diff;
pub mod event;
pub mod format;
#[cfg(not(target_family = "wasm"))]
pub mod fs_backend;
pub mod ledger_codec;
pub mod load;
pub mod load_control;
pub mod proxy;
pub mod store;
pub mod writer;

#[cfg(test)]
mod profile_tests;
#[cfg(test)]
pub(crate) mod test_fixtures;
#[cfg(test)]
mod tests;

pub use backend::{MemBackend, StoreBackend, StoreReader, StoreWriter};
pub use diff::{choose_ledger_write, merkle_diff, LedgerDiff, LedgerWrite};
pub use format::{Attempts, ChainEntry, Checkpoint, FileRef, Manifest, FORMAT_VERSION};
#[cfg(not(target_family = "wasm"))]
pub use fs_backend::FsBackend;
pub use ledger_codec::{LedgerFileHeader, LedgerFileKind, VerifyMode};
pub use proxy::{FsClient, ProxyBackend};
pub use store::{
    blob, Blob, ChainPutResult, CheckpointWrite, FileReader, FrontierStore, OpenOutcome,
    OpenReport, SlotStatus, StoreConfig,
};

/// Errors of the frontier store. None of them is a panic: a store that can't
/// be used is reported and the node bootstraps as it would without one.
#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    #[error("the store is locked by another process")]
    Locked,
    #[error(
        "the store was written by format version {found}, newer than ours ({ours}); left untouched"
    )]
    NewerFormat { found: u32, ours: u32 },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ledger file: {0}")]
    Codec(#[from] ledger_codec::CodecError),
    #[error("ledger diff: {0}")]
    Diff(#[from] diff::DiffError),
    #[error("file {name}: {reason}")]
    Corrupt { name: String, reason: String },
    #[error("the manifest references {0}, which isn't there")]
    MissingFile(String),
    #[error("invalid request: {0}")]
    Invalid(String),
}

impl PersistError {
    pub(crate) fn corrupt(name: impl Into<String>, reason: impl std::fmt::Display) -> Self {
        Self::Corrupt {
            name: name.into(),
            reason: reason.to_string(),
        }
    }
}
