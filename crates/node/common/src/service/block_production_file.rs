//! The block producer's history in the work directory (F-producer): the
//! newest attempts and epoch evaluation stats, so that the dashboard's
//! producer page keeps its history, and the blocks' fate, across a restart.
//! Saved on every change of an attempt's status and when an epoch's
//! evaluation starts, never more than
//! [`MAX_SAVED_ATTEMPTS`](mina_node::stats::block_producer::MAX_SAVED_ATTEMPTS)
//! attempts. Readable by the user only, like the VRF cache: a scheduled
//! attempt discloses the next won slot.
//!
//! The state machine only hands a snapshot over; a thread of the file's own
//! serializes and writes it, so that a slow disk (an fsync on a Pi's SD card)
//! never holds up block production. Only the newest snapshot matters: one
//! that arrives while another waits replaces it.

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::JoinHandle,
};

use mina_node::stats::block_producer::{BlockProducerStats, BlockProductionHistory};

pub const BLOCK_PRODUCTION_FILE: &str = "block-production.json";

/// A file bigger than this is not what this node wrote: it is not read.
const MAX_FILE_BYTES: u64 = 16 << 20;

pub struct BlockProductionFile {
    path: PathBuf,
    /// The revision last handed to the writer.
    submitted: u64,
    shared: Arc<Shared>,
    /// Started at the first save.
    writer: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Shared {
    slot: Mutex<Slot>,
    /// A snapshot is waiting, or the file is closing.
    wake: Condvar,
    /// A write finished.
    written: Condvar,
}

#[derive(Default)]
struct Slot {
    /// The newest snapshot not written yet.
    latest: Option<BlockProductionHistory>,
    writing: bool,
    closed: bool,
    /// Writes done, failed ones included.
    writes: u64,
    /// Tests hold the writer to pile snapshots up.
    #[cfg(test)]
    held: bool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl BlockProductionFile {
    /// The file in `dir`, the work directory.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            path: dir.into().join(BLOCK_PRODUCTION_FILE),
            // A run starts with what the file holds: nothing to write until
            // the history changes.
            submitted: 0,
            shared: Arc::default(),
            writer: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The history the last run kept, if any.
    pub fn load(&self) -> anyhow::Result<Option<BlockProductionHistory>> {
        let path = &self.path;
        match std::fs::metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(anyhow::anyhow!("{}: {error}", path.display())),
            Ok(meta) if meta.len() > MAX_FILE_BYTES => {
                anyhow::bail!("{}: larger than {MAX_FILE_BYTES} bytes", path.display())
            }
            Ok(_) => {}
        }
        let bytes =
            std::fs::read(path).map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
        let history = serde_json::from_slice(&bytes)
            .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
        Ok(Some(history))
    }

    /// Hands the history to the writer if it changed since it was last
    /// handed over, or since the run began, and returns at once: the write
    /// happens on the writer's thread. Returns whether it handed it over.
    /// Where there are no threads (the browser), it writes here instead.
    pub fn save(&mut self, stats: &BlockProducerStats) -> std::io::Result<bool> {
        if self.submitted == stats.revision() {
            return Ok(false);
        }
        self.submitted = stats.revision();
        let history = stats.history();
        if !self.start_writer() {
            let bytes = serde_json::to_vec(&history).map_err(std::io::Error::other)?;
            write_private(&self.path, &bytes)?;
            return Ok(true);
        }
        let mut slot = self.shared.lock();
        // An older snapshot not written yet is dropped: this one supersedes
        // it.
        slot.latest = Some(history);
        drop(slot);
        self.shared.wake.notify_one();
        Ok(true)
    }

    /// Waits until the writer has written everything handed to it.
    pub fn flush(&self) {
        let mut slot = self.shared.lock();
        while self.writer.is_some() && (slot.latest.is_some() || slot.writing) {
            slot = self
                .shared
                .written
                .wait(slot)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// Starts the writer's thread if it isn't running; false if it can't.
    fn start_writer(&mut self) -> bool {
        if self.writer.is_some() {
            return true;
        }
        let shared = self.shared.clone();
        let path = self.path.clone();
        match std::thread::Builder::new()
            .name("block-production-file".to_owned())
            .spawn(move || run_writer(&shared, &path))
        {
            Ok(handle) => {
                self.writer = Some(handle);
                true
            }
            Err(_) => false,
        }
    }

    #[cfg(test)]
    fn writes(&self) -> u64 {
        self.shared.lock().writes
    }

    #[cfg(test)]
    fn hold(&mut self, held: bool) {
        assert!(self.start_writer());
        self.shared.lock().held = held;
        self.shared.wake.notify_one();
    }
}

impl Drop for BlockProductionFile {
    /// Writes what is waiting, then stops the writer.
    fn drop(&mut self) {
        let Some(writer) = self.writer.take() else {
            return;
        };
        self.shared.lock().closed = true;
        self.shared.wake.notify_one();
        let _ = writer.join();
    }
}

fn run_writer(shared: &Shared, path: &Path) {
    loop {
        let history = {
            let mut slot = shared.lock();
            loop {
                #[cfg(test)]
                let held = slot.held && !slot.closed;
                #[cfg(not(test))]
                let held = false;
                if !held {
                    if let Some(history) = slot.latest.take() {
                        slot.writing = true;
                        break history;
                    }
                    if slot.closed {
                        return;
                    }
                }
                slot = shared
                    .wake
                    .wait(slot)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        let result = serde_json::to_vec(&history)
            .map_err(std::io::Error::other)
            .and_then(|bytes| write_private(path, &bytes));
        if let Err(error) = result {
            mina_core::warn!(
                mina_core::log::system_time();
                kind = "BlockProductionHistoryNotSaved",
                summary = "the block production history was not saved",
                path = path.display().to_string(),
                error = error.to_string(),
            );
        }
        let mut slot = shared.lock();
        slot.writing = false;
        slot.writes += 1;
        drop(slot);
        shared.written.notify_all();
    }
}

/// Writes `bytes` to `path` through a temporary file and a rename, with mode
/// 0600 on Unix, and syncs the directory so the rename survives a crash.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent();
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&tmp)?;
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    if let Some(dir) = dir {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mina_node::stats::block_producer::{BlockProductionAttempt, EpochStake};

    fn stake() -> EpochStake {
        EpochStake {
            delegated_stake: 6_000_000_000_000_000,
            total_currency: 1_200_000_000_000_000_000,
            delegators: 7,
        }
    }

    /// Attempts as the node records them: one scheduled, one proving.
    fn attempts() -> Vec<BlockProductionAttempt> {
        let attempt = |slot: u32, status: &str, proof_start: serde_json::Value| {
            serde_json::json!({
                "won_slot": {
                    "slot_time": 1792807200000000000u64 + u64::from(slot - 62720) * 90_000_000_000,
                    "global_slot": slot,
                    "epoch": 8,
                    "delegator": ["B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX", 6195],
                    "value_with_threshold": [0.0027, 0.0198]
                },
                "block": null,
                "times": {
                    "scheduled": 1792804549762535806u64,
                    "staged_ledger_diff_create_start": null,
                    "staged_ledger_diff_create_end": null,
                    "produced": null,
                    "proof_create_start": proof_start,
                    "proof_create_end": null,
                    "block_apply_start": null,
                    "block_apply_end": null,
                    "committed": null,
                    "discarded": null
                },
                "status": status
            })
        };
        serde_json::from_value(serde_json::json!([
            attempt(62736, "ProofCreatePending", 1792808634250000000u64.into()),
            attempt(62780, "Scheduled", serde_json::Value::Null),
        ]))
        .unwrap()
    }

    #[test]
    fn the_history_round_trips_and_is_written_only_when_it_changed() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = BlockProductionFile::new(dir.path());
        assert!(file.load().unwrap().is_none());

        let mut stats = BlockProducerStats::default();
        assert!(!file.save(&stats).unwrap(), "nothing new: no file");
        file.flush();
        assert!(!file.path().exists());
        stats.new_epoch_evaluation(8, 1569, stake());
        stats.increment_slot_evaluated(8, true);
        stats.increment_slot_evaluated(8, false);
        assert!(file.save(&stats).unwrap());
        assert!(
            !file.save(&stats).unwrap(),
            "unchanged: not handed over again"
        );
        file.flush();
        assert_eq!(file.writes(), 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(file.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "readable by the user only");
        }
        assert!(!file.path().with_extension("json.tmp").exists());

        let history = file.load().unwrap().unwrap();
        let epoch = &history.vrf_evaluator[&8];
        assert_eq!((epoch.evaluated_slots, epoch.won_slots), (2, 1));
        assert_eq!(epoch.delegators, Some(7));

        // A new run takes it up.
        let mut next = BlockProducerStats::default();
        next.restore(history);
        assert_eq!(next.vrf_evaluator[&8].won_slots, 1);
    }

    #[test]
    fn a_burst_of_changes_is_written_a_bounded_number_of_times_ending_with_the_last() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = BlockProductionFile::new(dir.path());
        let mut stats = BlockProducerStats::default();

        // While the writer is busy (held here), 1,000 changes pile up: each
        // replaces the one before, and one write follows.
        file.hold(true);
        for epoch in 0..1000 {
            stats.new_epoch_evaluation(epoch, 7140, stake());
            assert!(file.save(&stats).unwrap());
        }
        assert_eq!(file.writes(), 0);
        file.hold(false);
        file.flush();
        assert_eq!(file.writes(), 1, "superseded snapshots are dropped");
        let history = file.load().unwrap().unwrap();
        assert_eq!(history.vrf_evaluator.keys().last(), Some(&999));

        // Unheld, a burst is written at most once per change, and the file
        // ends with the last.
        for epoch in 1000..1200 {
            stats.new_epoch_evaluation(epoch, 7140, stake());
            file.save(&stats).unwrap();
        }
        file.flush();
        let writes = file.writes() - 1;
        assert!((1..=200).contains(&writes), "{writes} writes");
        let history = file.load().unwrap().unwrap();
        assert_eq!(history.vrf_evaluator.keys().last(), Some(&1199));
    }

    #[test]
    fn a_scheduled_and_a_proving_attempt_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        {
            // The run before the restart: the epoch's evaluation starts, a
            // won slot is scheduled and another is being proved.
            let mut file = BlockProductionFile::new(dir.path());
            let mut stats = BlockProducerStats::default();
            stats.restore(BlockProductionHistory {
                attempts: attempts(),
                ..Default::default()
            });
            stats.new_epoch_evaluation(8, 1569, stake());
            file.save(&stats).unwrap();
            // Killed: the file is not dropped, so nothing is flushed on the
            // way out; the writer finishes on its own.
            let shared = file.shared.clone();
            std::mem::forget(file);
            let mut slot = shared.lock();
            while slot.latest.is_some() || slot.writing {
                slot = shared.written.wait(slot).unwrap();
            }
        }

        // The next run takes it up.
        let file = BlockProductionFile::new(dir.path());
        let mut stats = BlockProducerStats::default();
        stats.restore(file.load().unwrap().unwrap());
        let history = stats.history();
        let slots: Vec<_> = history
            .attempts
            .iter()
            .map(|a| a.won_slot.global_slot)
            .collect();
        assert_eq!(slots, [62736, 62780]);
        let json = serde_json::to_value(&history.attempts).unwrap();
        assert_eq!(json[0]["status"], "ProofCreatePending");
        assert_eq!(json[1]["status"], "Scheduled");
        assert_eq!(
            history.vrf_evaluator[&8].delegated_stake,
            Some(6_000_000_000_000_000)
        );
    }

    #[test]
    fn dropping_the_file_writes_what_is_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = BlockProductionFile::new(dir.path());
        let mut stats = BlockProducerStats::default();
        file.hold(true);
        stats.new_epoch_evaluation(8, 1569, stake());
        file.save(&stats).unwrap();
        let path = file.path().to_owned();
        // Closing lets a held writer go.
        drop(file);
        assert!(path.exists());
    }

    #[test]
    fn an_unreadable_file_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(BLOCK_PRODUCTION_FILE), b"not json").unwrap();
        assert!(BlockProductionFile::new(dir.path()).load().is_err());
    }
}
