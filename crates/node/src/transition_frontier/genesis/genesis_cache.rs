//! What the genesis block commits to, kept for a daemon JSON config (I5d2).
//!
//! A node started with `--config` reads the daemon JSON and loads its three
//! genesis ledgers before anything else: 20 to 26 s on Mesa mainnet, most of
//! a restart from the frontier store (I5d1), which replaces those ledgers
//! anyway. With the frontier store on, the node keeps the
//! [`GenesisConfigLoaded`] its full load produced (the 6 KB that
//! `ledger-tool --without-ledgers` writes) in its work directory, named by the
//! config file's BLAKE2b-256 and the node's build. The next start with the
//! same file and build reads it instead of the JSON, and loads the genesis
//! ledgers only if the restore doesn't happen, before the node bootstraps
//! (`GenesisLedgersState`), checking then that they give the same
//! [`GenesisConfigLoaded`].
//!
//! A fresh node has no entry, so its start is unchanged.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

use blake2::{
    digest::{Update, VariableOutput},
    Blake2bVar,
};

use serde::{Deserialize, Serialize};

use super::GenesisConfigLoaded;

/// The node's genesis cache entry for one daemon JSON config.
#[derive(Debug, Clone)]
pub struct GenesisCache {
    path: PathBuf,
    key: GenesisCacheKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GenesisCacheKey {
    config_hash: String,
    build_hash: String,
}

#[derive(Serialize, Deserialize)]
struct GenesisCacheEntry {
    version: u32,
    key: GenesisCacheKey,
    genesis: GenesisConfigLoaded,
}

const CACHE_VERSION: u32 = 1;

fn blake2b_256() -> Blake2bVar {
    Blake2bVar::new(32).expect("32 is a valid BLAKE2b output size")
}

fn finish(hasher: Blake2bVar) -> String {
    let mut out = [0; 32];
    hasher
        .finalize_variable(&mut out)
        .expect("the output is 32 bytes");
    hex::encode(out)
}

pub(super) fn blake2b_hex(bytes: &[u8]) -> String {
    let mut hasher = blake2b_256();
    hasher.update(bytes);
    finish(hasher)
}

impl GenesisCache {
    /// The entry in `dir` for the config file `config`, read through once
    /// to hash it (about a second for Mesa mainnet's 736 MB), for a node of
    /// the build `build` (its version, commit and build time, so that a
    /// rebuilt node computes its own).
    pub fn for_config(dir: &Path, config: &Path, build: &str) -> std::io::Result<Self> {
        let mut file = std::fs::File::open(config)?;
        let mut hasher = blake2b_256();
        let mut buf = vec![0; 1 << 20];
        loop {
            let read = file.read(&mut buf)?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
        }
        Ok(Self::new(dir, &finish(hasher), build))
    }

    /// The entry in `dir` for a config whose BLAKE2b-256 is `config_hash`.
    pub fn new(dir: &Path, config_hash: &str, build: &str) -> Self {
        let build_hash = blake2b_hex(build.as_bytes());
        Self {
            path: dir.join(format!("genesis-{config_hash}-{}.json", &build_hash[..16])),
            key: GenesisCacheKey {
                config_hash: config_hash.to_owned(),
                build_hash,
            },
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The kept genesis, if there is one and it is whole: its first line is
    /// the BLAKE2b-256 of the rest: the format version, full config/build
    /// key and [`GenesisConfigLoaded`] in JSON. Old, unkeyed entries are
    /// rejected too, so the normal full-config load replaces them.
    pub fn read(&self) -> Result<Option<GenesisConfigLoaded>, String> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let newline = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or("no checksum line")?;
        let (checksum, json) = (&bytes[..newline], &bytes[newline + 1..]);
        if checksum != blake2b_hex(json).as_bytes() {
            return Err("checksum mismatch".to_owned());
        }
        let entry: GenesisCacheEntry =
            serde_json::from_slice(json).map_err(|error| error.to_string())?;
        if entry.version != CACHE_VERSION {
            return Err("unsupported genesis cache version".to_owned());
        }
        if entry.key != self.key {
            return Err("genesis cache config/build key mismatch".to_owned());
        }
        Ok(Some(entry.genesis))
    }

    /// Keeps `loaded`, replacing the entry whole (a new file renamed over
    /// it), so that a crash leaves the old entry or the new one.
    pub fn write(&self, loaded: &GenesisConfigLoaded) -> std::io::Result<()> {
        let json = serde_json::to_vec(&GenesisCacheEntry {
            version: CACHE_VERSION,
            key: self.key.clone(),
            genesis: loaded.clone(),
        })?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(blake2b_hex(&json).as_bytes())?;
            file.write_all(b"\n")?;
            file.write_all(&json)?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)
    }

    /// Drops the entry, if there is one.
    pub fn remove(&self) -> std::io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// Whether two loads give the same genesis: the same [`GenesisConfigLoaded`],
/// compared in the form the cache keeps it.
pub fn same_genesis(a: &GenesisConfigLoaded, b: &GenesisConfigLoaded) -> bool {
    match (serde_json::to_vec(a), serde_json::to_vec(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
