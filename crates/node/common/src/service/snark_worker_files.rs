//! The SNARK worker's files in the work directory (P5.4).
//!
//! - `snark-worker.json`: the key and fee the operator last started the
//!   worker with from the dashboard. A restart without `--run-snarker`
//!   starts the worker with them; `--run-snarker` wins. Stopping the worker
//!   from the dashboard deletes the file.
//! - `snark-worker-included.json`: the log of our work that blocks included
//!   ([`IncludedWorkLog`]), written when it changes.
//!
//! Both are written to a temporary file first and renamed, so a crash leaves
//! the old file or the new one.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use mina_node::{
    account::{parse_snarker, AccountPublicKey},
    external_snark_worker::{format_mina, IncludedWorkLog},
    SnarkerSetting,
};
use serde::{Deserialize, Serialize};

pub const SETTING_FILE: &str = "snark-worker.json";
pub const INCLUDED_WORK_FILE: &str = "snark-worker-included.json";

/// The operator's worker setting as the file keeps it: the key, and the fee
/// in MINA as `--snarker-fee` takes it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct SavedSetting {
    public_key: String,
    fee: String,
}

pub struct SnarkWorkerFiles {
    dir: PathBuf,
    /// The revision of the log last written.
    included_saved: Option<u64>,
}

impl SnarkWorkerFiles {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            included_saved: None,
        }
    }

    pub fn setting_path(&self) -> PathBuf {
        self.dir.join(SETTING_FILE)
    }

    pub fn included_work_path(&self) -> PathBuf {
        self.dir.join(INCLUDED_WORK_FILE)
    }

    /// The setting the dashboard saved, if any. A file that can't be read or
    /// holds an invalid key or fee is an error, not a silent stop.
    pub fn load_setting(&self) -> anyhow::Result<Option<SnarkerSetting>> {
        let path = self.setting_path();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(anyhow::anyhow!("{}: {error}", path.display())),
        };
        let saved: SavedSetting = serde_json::from_slice(&bytes)
            .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
        let (public_key, fee) = parse_snarker(Some(&saved.public_key), Some(&saved.fee))
            .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?
            .ok_or_else(|| anyhow::anyhow!("{}: no public key", path.display()))?;
        Ok(Some(SnarkerSetting { public_key, fee }))
    }

    /// Keeps `setting` for the next start, or deletes the file for `None`.
    pub fn save_setting(&self, setting: Option<&SnarkerSetting>) -> std::io::Result<()> {
        let path = self.setting_path();
        let Some(setting) = setting else {
            return match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            };
        };
        let saved = SavedSetting {
            public_key: AccountPublicKey::to_string(&setting.public_key),
            fee: format_mina(setting.fee),
        };
        let bytes = serde_json::to_vec_pretty(&saved).map_err(std::io::Error::other)?;
        write_atomic(&path, &bytes)
    }

    /// The log kept by the last run, if any.
    pub fn load_included_work(&mut self) -> anyhow::Result<Option<IncludedWorkLog>> {
        let path = self.included_work_path();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(anyhow::anyhow!("{}: {error}", path.display())),
        };
        let log: IncludedWorkLog = serde_json::from_slice(&bytes)
            .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
        self.included_saved = Some(log.revision);
        Ok(Some(log))
    }

    /// Writes the log if it changed since it was last written. Returns
    /// whether it wrote it.
    pub fn save_included_work(&mut self, log: &IncludedWorkLog) -> std::io::Result<bool> {
        if self.included_saved == Some(log.revision) {
            return Ok(false);
        }
        let bytes = serde_json::to_vec(log).map_err(std::io::Error::other)?;
        write_atomic(&self.included_work_path(), &bytes)?;
        self.included_saved = Some(log.revision);
        Ok(true)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // The setting names the account the worker sends fees to; the file
        // the next start trusts is its owner's business alone, like the
        // p2p identity.
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX";

    #[test]
    fn the_setting_round_trips_and_stopping_forgets_it() {
        let dir = tempfile::tempdir().unwrap();
        let files = SnarkWorkerFiles::new(dir.path());
        assert_eq!(files.load_setting().unwrap(), None);

        let setting = SnarkerSetting {
            public_key: KEY.parse().unwrap(),
            fee: 4_000_000,
        };
        files.save_setting(Some(&setting)).unwrap();
        let text = std::fs::read_to_string(files.setting_path()).unwrap();
        // The fee as `--snarker-fee` takes it.
        assert!(text.contains("\"0.004\""), "{text}");
        assert_eq!(files.load_setting().unwrap(), Some(setting));

        files.save_setting(None).unwrap();
        assert!(!files.setting_path().exists());
        assert_eq!(files.load_setting().unwrap(), None);
        // Stopping twice is fine.
        files.save_setting(None).unwrap();
    }

    #[test]
    fn a_bad_saved_setting_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let files = SnarkWorkerFiles::new(dir.path());
        for text in [
            "not json".to_owned(),
            format!(r#"{{"public_key":"{KEY}","fee":"-1"}}"#),
            r#"{"public_key":"B62qnot","fee":"0.1"}"#.to_owned(),
        ] {
            std::fs::write(files.setting_path(), text).unwrap();
            assert!(files.load_setting().is_err());
        }
    }

    /// The setting names the account the worker sends fees to; only its
    /// owner is to read or change it, like the p2p identity.
    #[cfg(unix)]
    #[test]
    fn saved_files_are_private_to_their_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mut files = SnarkWorkerFiles::new(dir.path());
        files
            .save_setting(Some(&SnarkerSetting {
                public_key: KEY.parse().unwrap(),
                fee: 1,
            }))
            .unwrap();
        let mut log = IncludedWorkLog::default();
        log.watch(&KEY.parse().unwrap(), 1_000);
        files.save_included_work(&log).unwrap();
        for path in [files.setting_path(), files.included_work_path()] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{}", path.display());
        }
    }

    #[test]
    fn the_log_is_written_when_it_changes_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = SnarkWorkerFiles::new(dir.path());
        assert_eq!(files.load_included_work().unwrap(), None);

        let mut log = IncludedWorkLog::default();
        log.watch(&KEY.parse().unwrap(), 1_000);
        assert!(files.save_included_work(&log).unwrap());
        // Unchanged: not written again.
        assert!(!files.save_included_work(&log).unwrap());

        let mut reopened = SnarkWorkerFiles::new(dir.path());
        assert_eq!(reopened.load_included_work().unwrap(), Some(log.clone()));
        assert!(!reopened.save_included_work(&log).unwrap());
        log.watch(
            &mina_node::account::AccountSecretKey::deterministic(1).public_key(),
            2_000,
        );
        assert!(reopened.save_included_work(&log).unwrap());
    }
}
