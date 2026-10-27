//! A ledger database's accounts in a file instead of the heap (PC3a).
//!
//! The node holds its epoch ledgers (the staking and next epoch ledgers) as
//! full databases, about 664 bytes of heap per account
//!. Neither changes once its
//! epoch has started, and OCaml keeps both in RocksDB with only the masks above
//! them in memory. A database can be frozen into a [`DiskAccounts`]: each
//! account's binprot bytes go to one file, and what stays in memory is, per
//! account, the record's offset, a CRC-32 of its bytes and a keyed 64-bit hash
//! of its id with its index, 24 bytes, beside the database's hash matrix, which
//! is unchanged. Changes made after freezing go to an overlay in the database
//! ([`super::database_impl`]), so a frozen database still behaves as a
//! database, only slower to read.
//!
//! The files belong to one process, like the snark pool's proof cache (PC1):
//! the directory is locked and its stale accounts removed at start. The last clone of a frozen
//! database's accounts deletes its file. Reads are positional, so clones on
//! different threads share one open file. A read checks the record's length
//! and checksum. A record that fails is read again, from the disk rather than
//! the page cache, [`READ_ATTEMPTS`] times in all. One that still fails is the
//! loss of the only copy of that account: the database stops the process
//! ([`unreadable`]), and a restart rebuilds the ledgers from the frontier
//! store or from peers.
//!
//! PC3b freezes the root snarked ledger too, which changes with every root
//! move: its changes collect in the overlay, and once there are
//! [`refreeze_threshold`] of them the database is written again
//! ([`DiskAccounts::merge`]), copying the records of the slots that didn't
//! change as they are. A small cache of decoded accounts ([`AccountCache`])
//! serves the reads that come in pairs, as a lookup by id and the read of the
//! slot it finds, or a write and the read of what it replaces.
//!
//! None of this is persistence. The files are never read by another process:
//! a restart rebuilds the ledgers from the frontier store, whose files are
//! written once and checked against their ledger hashes when loaded, so a
//! crash at any point of a freeze, a refreeze or a commit leaves nothing a
//! later start would trust.

use std::{
    collections::hash_map::RandomState,
    fs::{File, OpenOptions},
    hash::BuildHasher,
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
        Mutex, OnceLock,
    },
};

use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    v2::MinaBaseAccountBinableArgStableV2,
};

use crate::{Account, AccountId};

/// The file name suffix of an accounts file.
const SUFFIX: &str = "accounts";

struct AccountsDirectory {
    path: PathBuf,
    enabled: bool,
    // Retain the lock even in memory mode. Never unlink the lock file: a
    // later process must lock the same inode as the current owner.
    _lock: File,
}

static GLOBAL_DIR: OnceLock<AccountsDirectory> = OnceLock::new();
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

/// Process-wide counters, for `/stats/memory` and tests.
static FILES: AtomicU64 = AtomicU64::new(0);
static FILE_BYTES: AtomicU64 = AtomicU64::new(0);
static FILE_ACCOUNTS: AtomicU64 = AtomicU64::new(0);
static READS: AtomicU64 = AtomicU64::new(0);
static SCANS: AtomicU64 = AtomicU64::new(0);
static FREEZES: AtomicU64 = AtomicU64::new(0);
static CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static RAW_COPIES: AtomicU64 = AtomicU64::new(0);
static READ_RETRIES: AtomicU64 = AtomicU64::new(0);

/// Whether an unreadable record ends the process ([`unreadable`]) rather than
/// panicking. The node sets it; tests and tools keep the panic.
static EXIT_ON_UNREADABLE: AtomicBool = AtomicBool::new(false);

/// Reads of a record, the first included, before it counts as unreadable.
pub const READ_ATTEMPTS: u32 = 3;

/// Whether the root snarked ledger goes to disk too (PC3b), when there is a
/// directory. On by default; `--no-root-ledger-disk` turns it off.
static ROOT_ON_DISK: AtomicBool = AtomicBool::new(true);
/// The overlay size that makes a frozen database be written again;
/// `u64::MAX` for [`refreeze_threshold`]'s default.
static REFREEZE_SLOTS: AtomicU64 = AtomicU64::new(u64::MAX);

/// The fewest changed slots that make a frozen database be written again.
pub const REFREEZE_MIN: u64 = 4096;
/// Decoded accounts each [`AccountCache`] generation holds.
pub const CACHE_SLOTS: usize = 512;

/// Keep the root snarked ledger's accounts in a file (PC3b), or in memory.
pub fn set_root_on_disk(on: bool) {
    ROOT_ON_DISK.store(on, Relaxed);
}

pub fn root_on_disk() -> bool {
    ROOT_ON_DISK.load(Relaxed)
}

/// Overrides [`refreeze_threshold`], for tests and the kill loop: 1 writes
/// the root again at every commit that changes it, and 0 at every commit,
/// changed or not, so that a devnet follower, whose root snarked ledger
/// changes about once an hour, writes it at every block.
pub fn set_refreeze_slots(slots: Option<u64>) {
    REFREEZE_SLOTS.store(slots.unwrap_or(u64::MAX), Relaxed);
}

/// Whether a frozen database with no changes is written again too
/// ([`set_refreeze_slots`] 0).
pub fn rewrite_unchanged() -> bool {
    REFREEZE_SLOTS.load(Relaxed) == 0
}

/// A kill point for the crash tests (PC3b's kill loop): with
/// `MINA_KILL_AT=<point>:<n>` in its environment the process sends itself
/// SIGKILL the n-th time it reaches `point`, so that a kill lands at an exact
/// step of a commit or a freeze. Without it, a load and a compare.
pub fn kill_point(point: &str) {
    #[cfg(unix)]
    {
        static AT: OnceLock<Option<(String, u64)>> = OnceLock::new();
        static SEEN: AtomicU64 = AtomicU64::new(0);
        let at = AT.get_or_init(|| {
            let at = std::env::var("MINA_KILL_AT").ok()?;
            let (name, n) = at.rsplit_once(':')?;
            Some((name.to_owned(), n.parse().ok()?))
        });
        if let Some((name, n)) = at {
            if name == point && SEEN.fetch_add(1, Relaxed) + 1 == *n {
                eprintln!("MINA_KILL_AT: SIGKILL at {point} #{n}");
                // SAFETY: a signal to this process; it doesn't return.
                unsafe {
                    libc::kill(libc::getpid(), libc::SIGKILL);
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = point;
}

/// How many changed slots a frozen database of `slots` slots holds in its
/// overlay before it is written again: a sixteenth of them, at least
/// [`REFREEZE_MIN`]. For the root snarked ledger that is about 5,800 accounts
/// on devnet and 18,000 on mainnet, a few MB of overlay, written again every
/// few hundred root moves.
pub fn refreeze_threshold(slots: u64) -> usize {
    match REFREEZE_SLOTS.load(Relaxed) {
        u64::MAX => (slots / 16).max(REFREEZE_MIN) as usize,
        n => n as usize,
    }
}

/// Where frozen databases' files go, set once at start. Takes an exclusive
/// lock before removing this node's stale files, held until process exit.
pub fn set_global_dir(dir: &Path) -> std::io::Result<()> {
    configure_global_dir(dir, true)
}

/// Owns the accounts directory for this run and removes stale accounts even
/// when `enabled` is false (`--no-ledger-disk`). A second process is refused
/// before any file is removed. Only regular files with our writer's exact
/// name shape are removed; unrelated files, directories and symlinks stay.
/// This runs at startup, never while producing a block.
pub fn configure_global_dir(dir: &Path, enabled: bool) -> std::io::Result<()> {
    if GLOBAL_DIR.get().is_some() {
        return Err(std::io::Error::other(
            "the ledger accounts directory is already set",
        ));
    }
    if dir
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(std::io::Error::other(
            "the ledger accounts directory is a symlink",
        ));
    }
    std::fs::create_dir_all(dir)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let lock = options.open(dir.join("LOCK"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "another node owns the ledger accounts directory",
            ));
        }
        Err(std::fs::TryLockError::Error(error)) => return Err(error),
    }
    let mut stale = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && is_accounts_name(&entry.file_name()) {
            // Older nodes did not take LOCK: refuse a live file owner before
            // removing any file. LOCK existing proves nothing, as a refused
            // start leaves it behind (RC7-FIXES). See `accounts_owner_is_running`.
            #[cfg(unix)]
            if accounts_owner_is_running(&entry.file_name(), entry.metadata()?.modified().ok()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "a live process owns an accounts file in the directory",
                ));
            }
            stale.push(entry.path());
        }
    }
    for path in stale {
        std::fs::remove_file(path)?;
    }
    GLOBAL_DIR
        .set(AccountsDirectory {
            path: dir.to_owned(),
            enabled,
            _lock: lock,
        })
        .map_err(|_| std::io::Error::other("the ledger accounts directory is already set"))
}

fn is_accounts_name(name: &std::ffi::OsStr) -> bool {
    let Some((pid, sequence)) = name
        .to_str()
        .and_then(|name| name.strip_suffix(".accounts"))
        .and_then(|name| name.split_once('-'))
    else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    digits(pid) && digits(sequence) && pid.parse::<u32>().is_ok() && sequence.parse::<u64>().is_ok()
}

/// How much later than an accounts file's last change its PID's process may
/// have started and still count as its owner. Boot time is in whole seconds,
/// so a start time read from `/proc` is up to a second early; the rest is
/// margin. Erring this way refuses a start rather than removing a live
/// owner's files; a refused start under a restart policy succeeds once the
/// new process starts later.
#[cfg(unix)]
const OWNER_START_TOLERANCE: std::time::Duration = std::time::Duration::from_secs(2);

/// Whether the process an accounts file names may own it (RC7-FIXES). Not if
/// the PID is ours: a container gives the node the same PID at every start.
/// Not if no such process runs. Not if that process started after the file
/// last changed: a writer runs before it writes, so a process with a reused
/// PID (after a container restart, a reboot or a refused start) started
/// after the dead owner's last write. Not if it is a kernel thread, which
/// no node is. Anything it can't tell counts as an owner.
#[cfg(unix)]
fn accounts_owner_is_running(
    name: &std::ffi::OsStr,
    modified: Option<std::time::SystemTime>,
) -> bool {
    let Some(pid) = name
        .to_str()
        .and_then(|name| name.split_once('-'))
        .and_then(|(pid, _)| pid.parse::<libc::pid_t>().ok())
        .filter(|pid| *pid > 0 && *pid as u32 != std::process::id())
    else {
        return false;
    };
    // SAFETY: signal 0 only tests whether this PID exists; it sends no signal.
    if unsafe { libc::kill(pid, 0) } != 0
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    {
        return false;
    }
    match (process_start_time(pid), modified) {
        (Some(started), Some(modified)) if started > modified + OWNER_START_TOLERANCE => false,
        _ => !is_kernel_thread(pid),
    }
}

/// When process `pid` started, from `/proc/<pid>/stat` field 22 (clock ticks
/// since boot) and `btime` in `/proc/stat`.
#[cfg(target_os = "linux")]
fn process_start_time(pid: libc::pid_t) -> Option<std::time::SystemTime> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_start_time(&stat, &std::fs::read_to_string("/proc/stat").ok()?, {
        // SAFETY: sysconf only reads a configuration value.
        unsafe { libc::sysconf(libc::_SC_CLK_TCK) }
    })
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_start_time(_pid: libc::pid_t) -> Option<std::time::SystemTime> {
    None
}

#[cfg(unix)]
fn parse_start_time(
    pid_stat: &str,
    proc_stat: &str,
    ticks_per_second: libc::c_long,
) -> Option<std::time::SystemTime> {
    // The command name, field 2, may hold spaces and parentheses: the fields
    // after it start after the last ')', with field 3.
    let fields = &pid_stat[pid_stat.rfind(')')? + 1..];
    let ticks: u64 = fields.split_whitespace().nth(22 - 3)?.parse().ok()?;
    let boot: u64 = proc_stat
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    let ticks_per_second = u64::try_from(ticks_per_second).ok().filter(|n| *n > 0)?;
    let since_boot = std::time::Duration::from_nanos(
        u64::try_from(u128::from(ticks) * 1_000_000_000 / u128::from(ticks_per_second)).ok()?,
    );
    std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(boot) + since_boot)
}

/// A kernel thread has an empty command line. One that can't be read is not
/// taken for one.
#[cfg(unix)]
fn is_kernel_thread(pid: libc::pid_t) -> bool {
    cfg!(target_os = "linux")
        && std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|cmdline| cmdline.is_empty())
}

/// The locked directory, if disk accounts are enabled for this run.
pub fn global_dir() -> Option<&'static Path> {
    GLOBAL_DIR
        .get()
        .filter(|dir| dir.enabled)
        .map(|dir| dir.path.as_path())
}

/// What the frozen databases of this process hold on disk, and how often they
/// were read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct DiskStats {
    /// Accounts files open.
    pub files: u64,
    /// Their bytes.
    pub file_bytes: u64,
    /// The accounts in them.
    pub accounts: u64,
    /// Single-account reads.
    pub reads: u64,
    /// Whole-file scans (iteration over a frozen database).
    pub scans: u64,
    /// Databases frozen, refreezes included.
    pub freezes: u64,
    /// Single-account reads answered by a cache, without a file read.
    pub cache_hits: u64,
    /// Records copied as they were by refreezes.
    pub raw_copies: u64,
    /// Record reads that failed and were tried again.
    pub read_retries: u64,
}

pub fn stats() -> DiskStats {
    DiskStats {
        files: FILES.load(Relaxed),
        file_bytes: FILE_BYTES.load(Relaxed),
        accounts: FILE_ACCOUNTS.load(Relaxed),
        reads: READS.load(Relaxed),
        scans: SCANS.load(Relaxed),
        freezes: FREEZES.load(Relaxed),
        cache_hits: CACHE_HITS.load(Relaxed),
        raw_copies: RAW_COPIES.load(Relaxed),
        read_retries: READ_RETRIES.load(Relaxed),
    }
}

/// Makes a record that stays unreadable end the process with status 1, after
/// logging why, instead of panicking ([`unreadable`]). Set by the node.
pub fn set_exit_on_unreadable(on: bool) {
    EXIT_ON_UNREADABLE.store(on, Relaxed);
}

/// What [`unreadable`] runs after logging and before the process exits.
static BEFORE_EXIT: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Runs `flush` in [`unreadable`], after the error is logged and before the
/// process exits: the node drains its file logger there, which writes from a
/// thread of its own, so that the log file keeps the line. `flush` runs on
/// the thread that read the record, the ledger thread in a node, so it must
/// not wait on anything that thread may hold. Set once; later calls are
/// ignored.
pub fn set_before_unreadable_exit(flush: impl Fn() + Send + Sync + 'static) {
    let _ = BEFORE_EXIT.set(Box::new(flush));
}

/// A frozen database's record could not be read back, [`READ_ATTEMPTS`] times:
/// the file held the only copy of an account, and there is no correct answer
/// to give instead. A panic would unwind through the ledger's masks, whose
/// locks it poisons, and leave the node running without its ledger thread
/// until its next request. With [`set_exit_on_unreadable`] the process logs
/// the error and exits at once instead, so that its supervisor restarts it
/// and the restart rebuilds the ledgers, from the frontier store or from
/// peers: the files are this process's, removed at the next start.
#[cold]
pub fn unreadable(path: &Path, error: &DiskError) -> ! {
    let summary = format!(
        "ledger accounts file {} unreadable: {error}",
        path.display()
    );
    if EXIT_ON_UNREADABLE.load(Relaxed) {
        use std::io::Write;
        mina_core::error!(
            mina_core::log::system_time();
            kind = "LedgerAccountsUnreadable",
            summary = summary.clone(),
            path = path.display().to_string(),
            error = error.to_string(),
        );
        // Visible even without a tracing subscriber.
        let _ = writeln!(
            std::io::stderr().lock(),
            "Fatal: {summary}; a restart rebuilds the ledgers; exiting with status 1"
        );
        if let Some(flush) = BEFORE_EXIT.get() {
            flush();
        }
        std::process::exit(1);
    }
    panic!("{summary}")
}

/// Why a record could not be read back.
#[derive(Debug, thiserror::Error)]
pub enum DiskError {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("record {index}: checksum mismatch")]
    Checksum { index: u64 },
    #[error("record {index}: {error}")]
    Decode { index: u64, error: String },
}

/// The accounts of a frozen database: a file of binprot records, one per
/// slot (an empty slot has none), and their offsets, checksums and an id index
/// in memory.
pub struct DiskAccounts {
    file: ReadAt,
    path: PathBuf,
    /// `offsets[i]..offsets[i + 1]` is slot `i`'s record; equal for an empty
    /// slot. One more than the slots.
    offsets: Box<[u64]>,
    /// CRC-32 of each slot's record.
    checksums: Box<[u32]>,
    /// The keyed hashes of the accounts' ids, sorted, and in `id_indexes` the
    /// slot of each. A hash only finds candidates; the id is compared with the
    /// account read back, so a collision costs a read, never a wrong answer.
    id_hashes: Box<[u64]>,
    id_indexes: Box<[u32]>,
    hasher: RandomState,
    accounts: u64,
    cache: Mutex<AccountCache>,
    /// Tests: how many of the next record reads come back with a changed
    /// byte, as a read the disk got wrong.
    #[cfg(test)]
    pub(crate) fail_reads: std::sync::atomic::AtomicU32,
}

/// Recently read accounts of one file, in two generations: a hit in the old
/// one moves the account to the new one, and a full new generation becomes
/// the old one. So it keeps between [`CACHE_SLOTS`] and twice that, the most
/// recently used, for the cost of two hash maps. A file never changes, so
/// nothing in it goes stale.
#[derive(Default)]
struct AccountCache {
    new: std::collections::HashMap<u64, Account>,
    old: std::collections::HashMap<u64, Account>,
}

impl AccountCache {
    fn get(&mut self, index: u64) -> Option<Account> {
        if let Some(account) = self.new.get(&index) {
            return Some(account.clone());
        }
        let account = self.old.remove(&index)?;
        self.insert(index, account.clone());
        Some(account)
    }

    fn insert(&mut self, index: u64, account: Account) {
        if self.new.len() >= CACHE_SLOTS {
            self.old = std::mem::take(&mut self.new);
        }
        self.new.insert(index, account);
    }

    fn len(&self) -> usize {
        self.new.len() + self.old.len()
    }
}

impl std::fmt::Debug for DiskAccounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskAccounts")
            .field("path", &self.path)
            .field("slots", &self.len())
            .field("accounts", &self.accounts)
            .field("bytes", &self.file_len())
            .finish()
    }
}

impl Drop for DiskAccounts {
    fn drop(&mut self) {
        FILES.fetch_sub(1, Relaxed);
        FILE_BYTES.fetch_sub(self.file_len(), Relaxed);
        FILE_ACCOUNTS.fetch_sub(self.accounts, Relaxed);
        if let Err(error) = std::fs::remove_file(&self.path) {
            eprintln!(
                "could not remove ledger accounts file {}: {error}",
                self.path.display()
            );
        }
    }
}

impl DiskAccounts {
    /// The slots, empty ones included.
    pub fn len(&self) -> u64 {
        (self.offsets.len() - 1) as u64
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The accounts, that is the slots that aren't empty.
    pub fn num_accounts(&self) -> u64 {
        self.accounts
    }

    pub fn file_len(&self) -> u64 {
        self.offsets.last().copied().unwrap_or(0)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Accounts the read cache holds.
    pub fn cached(&self) -> usize {
        self.cache.lock().unwrap().len()
    }

    /// Bytes of memory this holds per slot, beside the file and the cache.
    pub fn heap_bytes(&self) -> usize {
        self.offsets.len() * 8
            + self.checksums.len() * 4
            + self.id_hashes.len() * 8
            + self.id_indexes.len() * 4
    }

    fn id_hash(&self, id: &AccountId) -> u64 {
        self.hasher.hash_one(id)
    }

    /// The account in slot `index`, if there is one.
    pub fn get(&self, index: u64) -> Result<Option<Account>, DiskError> {
        let Some(range) = self.range(index) else {
            return Ok(None);
        };
        if range.0 == range.1 {
            return Ok(None);
        }
        if let Some(account) = self.cache.lock().unwrap().get(index) {
            CACHE_HITS.fetch_add(1, Relaxed);
            return Ok(Some(account));
        }
        READS.fetch_add(1, Relaxed);
        let buf = self.read_record(index, range)?;
        let account = self.decode(index, &buf)?;
        self.cache.lock().unwrap().insert(index, account.clone());
        Ok(Some(account))
    }

    fn range(&self, index: u64) -> Option<(u64, u64)> {
        let index = usize::try_from(index).ok()?;
        Some((*self.offsets.get(index)?, *self.offsets.get(index + 1)?))
    }

    /// The bytes of slot `index`'s record, `start..end` of the file, checked
    /// against its checksum, read again as [`Self::read_again`] says if the
    /// first read fails.
    fn read_record(&self, index: u64, range: (u64, u64)) -> Result<Vec<u8>, DiskError> {
        let mut buf = vec![0; (range.1 - range.0) as usize];
        match self.read_once(index, range.0, &mut buf) {
            Ok(()) => Ok(buf),
            Err(error) => self.read_again(index, range, buf, error),
        }
    }

    /// One positional read of slot `index`'s record into `buf`, checked.
    fn read_once(&self, index: u64, start: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        self.file
            .read_exact_at(buf, start)
            .map_err(DiskError::from)
            .and_then(|()| {
                self.inject_fault(buf);
                self.check(index, buf)
            })
    }

    /// Slot `index`'s record after a first read of it failed with `error`: a
    /// single read's, or a scan's or a merge's sequential read. A read that
    /// fails, comes back short or with other bytes than were written is tried
    /// again, so that every path reads a record [`READ_ATTEMPTS`] times in
    /// all, the first included, after a pause and with the range dropped from
    /// the page cache, so that the next read is of the disk: a disk can fail
    /// a read once and not the next time. Dropping the range is advisory
    /// (`posix_fadvise`), so a retry may still be answered from the page
    /// cache; the pauses give a failing device time either way. The last
    /// error is returned; the caller makes it [`unreadable`].
    fn read_again(
        &self,
        index: u64,
        (start, end): (u64, u64),
        mut buf: Vec<u8>,
        mut error: DiskError,
    ) -> Result<Vec<u8>, DiskError> {
        buf.resize((end - start) as usize, 0);
        let mut attempt = 1;
        loop {
            if attempt >= READ_ATTEMPTS {
                return Err(error);
            }
            READ_RETRIES.fetch_add(1, Relaxed);
            mina_core::warn!(
                mina_core::log::system_time();
                kind = "LedgerAccountsReadRetry",
                summary = "a ledger accounts record could not be read; reading it again",
                path = self.path.display().to_string(),
                record = index,
                attempt = attempt,
                error = error.to_string(),
            );
            self.file.drop_cached(start, end - start);
            #[cfg(not(target_family = "wasm"))]
            std::thread::sleep(std::time::Duration::from_millis(10u64.pow(attempt)));
            attempt += 1;
            match self.read_once(index, start, &mut buf) {
                Ok(()) => return Ok(buf),
                Err(next) => error = next,
            }
        }
    }

    /// Tests: a record read the disk got wrong, while [`Self::fail_reads`]
    /// says so.
    fn inject_fault(&self, buf: &mut [u8]) {
        #[cfg(test)]
        if !buf.is_empty()
            && self
                .fail_reads
                .fetch_update(Relaxed, Relaxed, |n| n.checked_sub(1))
                .is_ok()
        {
            buf[0] ^= 0xff;
        }
        #[cfg(not(test))]
        let _ = buf;
    }

    /// Whether `bytes` are slot `index`'s record as it was written.
    fn check(&self, index: u64, bytes: &[u8]) -> Result<(), DiskError> {
        if crc32fast::hash(bytes) != self.checksums[index as usize] {
            return Err(DiskError::Checksum { index });
        }
        Ok(())
    }

    /// Slot `index`'s account, from its record's bytes, already checked.
    fn decode(&self, index: u64, bytes: &[u8]) -> Result<Account, DiskError> {
        let decode_error = |error: String| DiskError::Decode { index, error };
        let mut reader = bytes;
        let account = MinaBaseAccountBinableArgStableV2::binprot_read(&mut reader)
            .map_err(|e| decode_error(e.to_string()))?;
        if !reader.is_empty() {
            return Err(decode_error("trailing bytes".into()));
        }
        Account::try_from(&account).map_err(|e| decode_error(e.to_string()))
    }

    /// The slots that may hold `id`, by its hash; the caller compares ids.
    pub fn candidates(&self, id: &AccountId) -> impl Iterator<Item = u64> + '_ {
        let hash = self.id_hash(id);
        let start = self.id_hashes.partition_point(|h| *h < hash);
        self.id_hashes[start..]
            .iter()
            .zip(&self.id_indexes[start..])
            .take_while(move |(h, _)| **h == hash)
            .map(|(_, index)| *index as u64)
    }

    /// The slot holding `id` in the file, if any.
    pub fn find(&self, id: &AccountId) -> Result<Option<u64>, DiskError> {
        for index in self.candidates(id) {
            if self.get(index)?.is_some_and(|account| &account.id() == id) {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    /// Calls `fun` with every slot in order, reading the file once from the
    /// start. `want(index)` false skips decoding a slot (`fun` gets `None`).
    /// `fun` returning false stops the scan.
    pub fn scan<W, F>(&self, mut want: W, mut fun: F) -> Result<(), DiskError>
    where
        W: FnMut(u64) -> bool,
        F: FnMut(u64, Option<Account>) -> bool,
    {
        SCANS.fetch_add(1, Relaxed);
        let mut reader = BufReader::with_capacity(1 << 20, self.file.reader());
        let mut buf = Vec::new();
        for index in 0..self.len() {
            let range = self.range(index).expect("slot in range");
            let len = (range.1 - range.0) as usize;
            buf.resize(len, 0);
            let read = reader.read_exact(&mut buf);
            if read.is_err() {
                // Where the reader stopped is unknown: go on after this record.
                reader = BufReader::with_capacity(1 << 20, self.file.reader_at(range.1));
            }
            let account = if len > 0 && want(index) {
                let checked = read.map_err(DiskError::from).and_then(|()| {
                    self.inject_fault(&mut buf);
                    self.check(index, &buf)
                });
                if let Err(error) = checked {
                    // Read it again on its own, as a single read is.
                    buf = self.read_again(index, range, std::mem::take(&mut buf), error)?;
                }
                Some(self.decode(index, &buf)?)
            } else {
                None
            };
            if !fun(index, account) {
                break;
            }
        }
        Ok(())
    }

    /// A new file in `dir` holding this one's records with `changes` over
    /// them, for `slots` slots: what a frozen database with those changes in
    /// its overlay holds. The records of the slots `changes` doesn't name are
    /// copied as they are, checksums and id hashes included (the new file
    /// shares this one's id hasher), and only the changed accounts are
    /// encoded, so a refreeze costs about a sequential copy of the file. A
    /// record that fails its checksum is read again as a single read is. One
    /// that stays unreadable is the loss of the only copy of that account, as
    /// for any other read: the new file is removed and the process stops
    /// ([`unreadable`]). An error writing the new file is returned, and
    /// nothing is written: the database keeps its file and its overlay.
    pub fn merge(
        &self,
        dir: &Path,
        slots: u64,
        changes: &std::collections::BTreeMap<u64, Option<Account>>,
    ) -> std::io::Result<DiskAccounts> {
        let mut slot_hashes = vec![0u64; self.len() as usize];
        for (hash, index) in self.id_hashes.iter().zip(&*self.id_indexes) {
            slot_hashes[*index as usize] = *hash;
        }
        let mut writer = DiskWriter::create_with_hasher(dir, slots as usize, self.hasher.clone())?;
        let result = (|| -> Result<u64, MergeError> {
            let mut reader = BufReader::with_capacity(1 << 20, self.file.reader());
            let mut buf = Vec::new();
            let mut copied = 0;
            for index in 0..slots {
                let record = match self.range(index) {
                    Some(range) => {
                        buf.resize((range.1 - range.0) as usize, 0);
                        let read = reader.read_exact(&mut buf);
                        if read.is_err() {
                            reader =
                                BufReader::with_capacity(1 << 20, self.file.reader_at(range.1));
                        }
                        Some((range, read))
                    }
                    None => None,
                };
                match changes.get(&index) {
                    Some(account) => writer.push(account.as_ref())?,
                    None => match record {
                        Some((range, read)) => {
                            let checked = read.map_err(DiskError::from).and_then(|()| {
                                self.inject_fault(&mut buf);
                                self.check(index, &buf)
                            });
                            if let Err(error) = checked {
                                buf = self
                                    .read_again(index, range, std::mem::take(&mut buf), error)
                                    .map_err(MergeError::Unreadable)?;
                            }
                            let checksum = self.checksums[index as usize];
                            let id_hash = (!buf.is_empty()).then(|| slot_hashes[index as usize]);
                            writer.push_raw(&buf, checksum, id_hash)?;
                            copied += 1;
                        }
                        None => writer.push(None)?,
                    },
                }
            }
            Ok(copied)
        })();
        match result {
            Ok(copied) => {
                RAW_COPIES.fetch_add(copied, Relaxed);
                writer.finish()
            }
            Err(MergeError::Unreadable(error)) => {
                writer.abandon();
                unreadable(&self.path, &error)
            }
            Err(MergeError::Write(error)) => {
                writer.abandon();
                Err(error)
            }
        }
    }
}

/// Why [`DiskAccounts::merge`] stopped: a source record that stayed
/// unreadable, or an error writing the new file.
enum MergeError {
    Unreadable(DiskError),
    Write(std::io::Error),
}

impl From<std::io::Error> for MergeError {
    fn from(error: std::io::Error) -> Self {
        Self::Write(error)
    }
}

/// Writes a [`DiskAccounts`]: slots in order, then [`Self::finish`].
pub struct DiskWriter {
    file: BufWriter<File>,
    path: PathBuf,
    offsets: Vec<u64>,
    checksums: Vec<u32>,
    ids: Vec<(u64, u32)>,
    hasher: RandomState,
    record: Vec<u8>,
    accounts: u64,
    /// The slot at which [`kill_point`] `freeze-write` is reached.
    half: usize,
}

impl DiskWriter {
    /// A new file in `dir`, for about `slots` slots.
    pub fn create(dir: &Path, slots: usize) -> std::io::Result<Self> {
        Self::create_with_hasher(dir, slots, RandomState::new())
    }

    /// A new file whose id index uses `hasher`, so that records copied from
    /// another file keep their id hashes.
    fn create_with_hasher(dir: &Path, slots: usize, hasher: RandomState) -> std::io::Result<Self> {
        let path = dir.join(format!(
            "{}-{}.{SUFFIX}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Relaxed)
        ));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut offsets = Vec::with_capacity(slots + 1);
        offsets.push(0);
        Ok(Self {
            file: BufWriter::with_capacity(1 << 20, file),
            path,
            offsets,
            checksums: Vec::with_capacity(slots),
            ids: Vec::with_capacity(slots),
            hasher,
            record: Vec::with_capacity(512),
            accounts: 0,
            half: slots / 2,
        })
    }

    /// The next slot.
    pub fn push(&mut self, account: Option<&Account>) -> std::io::Result<()> {
        let index = self.checksums.len();
        if index == self.half {
            kill_point("freeze-write");
        }
        let index32 = u32::try_from(index).map_err(std::io::Error::other)?;
        self.record.clear();
        if let Some(account) = account {
            MinaBaseAccountBinableArgStableV2::from(account).binprot_write(&mut self.record)?;
            self.ids.push((self.hasher.hash_one(account.id()), index32));
            self.accounts += 1;
        }
        self.file.write_all(&self.record)?;
        self.checksums.push(crc32fast::hash(&self.record));
        let end = self.offsets.last().unwrap() + self.record.len() as u64;
        self.offsets.push(end);
        Ok(())
    }

    /// The next slot, as `record`'s bytes with its checksum and, for an
    /// account, its id hash under this writer's hasher, as another file of
    /// the same hasher holds it.
    fn push_raw(
        &mut self,
        record: &[u8],
        checksum: u32,
        id_hash: Option<u64>,
    ) -> std::io::Result<()> {
        let index = self.checksums.len();
        if index == self.half {
            kill_point("freeze-write");
        }
        let index32 = u32::try_from(index).map_err(std::io::Error::other)?;
        if let Some(hash) = id_hash {
            self.ids.push((hash, index32));
            self.accounts += 1;
        }
        self.file.write_all(record)?;
        self.checksums.push(checksum);
        let end = self.offsets.last().unwrap() + record.len() as u64;
        self.offsets.push(end);
        Ok(())
    }

    /// Gives up: the file goes.
    pub fn abandon(self) {
        let path = self.path.clone();
        drop(self);
        let _ = std::fs::remove_file(path);
    }

    pub fn finish(self) -> std::io::Result<DiskAccounts> {
        let Self {
            file,
            path,
            offsets,
            checksums,
            mut ids,
            hasher,
            accounts,
            record: _,
            half: _,
        } = self;
        let result = (|| {
            let file = file.into_inner().map_err(|e| e.into_error())?;
            ids.sort_unstable();
            let (id_hashes, id_indexes): (Vec<_>, Vec<_>) = ids.into_iter().unzip();
            Ok(DiskAccounts {
                file: ReadAt::new(file),
                path: path.clone(),
                offsets: offsets.into_boxed_slice(),
                checksums: checksums.into_boxed_slice(),
                id_hashes: id_hashes.into_boxed_slice(),
                id_indexes: id_indexes.into_boxed_slice(),
                hasher,
                accounts,
                cache: Mutex::new(AccountCache::default()),
                #[cfg(test)]
                fail_reads: Default::default(),
            })
        })();
        match result {
            Ok(disk) => {
                FILES.fetch_add(1, Relaxed);
                FILE_BYTES.fetch_add(disk.file_len(), Relaxed);
                FILE_ACCOUNTS.fetch_add(disk.accounts, Relaxed);
                FREEZES.fetch_add(1, Relaxed);
                Ok(disk)
            }
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                Err(error)
            }
        }
    }
}

/// A file read at offsets: positional reads where the platform has them, so
/// that clones of a frozen database on different threads need no lock.
struct ReadAt {
    #[cfg(unix)]
    file: File,
    #[cfg(not(unix))]
    file: std::sync::Mutex<File>,
}

impl ReadAt {
    fn new(file: File) -> Self {
        Self {
            #[cfg(unix)]
            file,
            #[cfg(not(unix))]
            file: std::sync::Mutex::new(file),
        }
    }

    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::FileExt::read_exact_at(&self.file, buf, offset)
        }
        #[cfg(not(unix))]
        {
            use std::io::Seek;
            let mut file = self.file.lock().unwrap();
            file.seek(std::io::SeekFrom::Start(offset))?;
            file.read_exact(buf)
        }
    }

    fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
        #[cfg(unix)]
        {
            std::os::unix::fs::FileExt::read_at(&self.file, buf, offset)
        }
        #[cfg(not(unix))]
        {
            use std::io::Seek;
            let mut file = self.file.lock().unwrap();
            file.seek(std::io::SeekFrom::Start(offset))?;
            file.read(buf)
        }
    }

    /// A sequential reader from the start, on the descriptor opened when the
    /// file was written, by positional reads: it has an offset of its own,
    /// and needs nothing at the file's path, so a file removed or renamed
    /// while the node runs is still read (PC3c: no read the block producer
    /// relies on may fail on a healthy node).
    fn reader(&self) -> ReadAtCursor<'_> {
        ReadAtCursor {
            file: self,
            offset: 0,
        }
    }

    /// A sequential reader from `offset`, as [`Self::reader`].
    fn reader_at(&self, offset: u64) -> ReadAtCursor<'_> {
        ReadAtCursor { file: self, offset }
    }

    /// Drops `len` bytes from `offset` from the page cache, so that the next
    /// read of them is of the disk. Only a hint.
    fn drop_cached(&self, offset: u64, len: u64) {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: a hint about a descriptor this holds open.
            unsafe {
                libc::posix_fadvise(
                    self.file.as_raw_fd(),
                    offset as libc::off_t,
                    len as libc::off_t,
                    libc::POSIX_FADV_DONTNEED,
                );
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (offset, len);
    }
}

struct ReadAtCursor<'a> {
    file: &'a ReadAt,
    offset: u64,
}

impl Read for ReadAtCursor<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.file.read_at(buf, self.offset)?;
        self.offset += n as u64;
        Ok(n)
    }
}

#[cfg(all(test, unix))]
mod directory_tests {
    use super::*;

    const CHILD: &str = "MINA_LEDGER_0201_DIRECTORY";
    const MODE: &str = "MINA_LEDGER_0201_DIRECTORY_MODE";

    fn child(dir: &Path, mode: &str, test: &str) -> std::process::Output {
        let name = format!("{}::{test}", module_path!().split_once("::").unwrap().1);
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture", "--test-threads", "1"])
            .env(CHILD, dir)
            .env(MODE, mode)
            .output()
            .unwrap()
    }

    fn passed(output: std::process::Output) {
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn ledger_0201_accounts_lock_precedes_cleanup_in_both_modes() {
        let test = "ledger_0201_accounts_lock_precedes_cleanup_in_both_modes";
        if let Some(dir) = std::env::var_os(CHILD) {
            let dir = PathBuf::from(dir);
            let mode = std::env::var(MODE).unwrap();
            if mode.starts_with("contender") {
                let error = configure_global_dir(&dir, mode == "contender-disk").unwrap_err();
                assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
                return;
            }
            configure_global_dir(&dir, mode == "owner-disk").unwrap();
            assert_eq!(global_dir().is_some(), mode == "owner-disk");
            let live = dir.join(format!("{}-0.accounts", std::process::id()));
            std::fs::write(&live, b"live ledger").unwrap();
            for mode in ["contender-disk", "contender-memory"] {
                passed(child(&dir, mode, test));
                assert_eq!(std::fs::read(&live).unwrap(), b"live ledger");
            }
            return;
        }
        for mode in ["owner-disk", "owner-memory"] {
            let dir = tempfile::tempdir().unwrap();
            passed(child(dir.path(), mode, test));
            // Process exit releases the lock without deleting its inode.
            passed(child(dir.path(), mode, test));
        }
    }

    #[test]
    fn ledger_0201_memory_mode_removes_only_owned_stale_accounts() {
        let test = "ledger_0201_memory_mode_removes_only_owned_stale_accounts";
        if let Some(dir) = std::env::var_os(CHILD) {
            let dir = PathBuf::from(dir);
            configure_global_dir(&dir, false).unwrap();
            assert!(global_dir().is_none());
            assert!(!dir.join("2147483646-0.accounts").exists());
            assert!(!dir.join("2147483645-12.accounts").exists());
            for keep in [
                "notes.accounts",
                "2147483646-x.accounts",
                "notes.txt",
                "2147483646-2.accounts",
                "2147483646-3.accounts",
            ] {
                assert!(dir.join(keep).symlink_metadata().is_ok(), "removed {keep}");
            }
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("ledger-accounts");
        std::fs::create_dir(&dir).unwrap();
        for file in [
            "2147483646-0.accounts",
            "2147483645-12.accounts",
            "notes.accounts",
            "2147483646-x.accounts",
            "notes.txt",
        ] {
            std::fs::write(dir.join(file), b"stale or unrelated").unwrap();
        }
        std::fs::create_dir(dir.join("2147483646-2.accounts")).unwrap();
        let outside = root.path().join("outside");
        std::fs::write(&outside, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("2147483646-3.accounts")).unwrap();
        passed(child(&dir, "cleanup", test));
        assert_eq!(std::fs::read(outside).unwrap(), b"keep");
    }

    #[test]
    fn ledger_0201_cleanup_refuses_a_live_legacy_owner_without_a_lock() {
        let test = "ledger_0201_cleanup_refuses_a_live_legacy_owner_without_a_lock";
        if let Some(dir) = std::env::var_os(CHILD) {
            let enabled = std::env::var(MODE).unwrap() == "disk";
            let error = configure_global_dir(Path::new(&dir), enabled).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
            return;
        }
        for mode in ["disk", "memory"] {
            let dir = tempfile::tempdir().unwrap();
            let live = dir
                .path()
                .join(format!("{}-0.accounts", std::process::id()));
            let stale = dir.path().join("2147483646-0.accounts");
            std::fs::write(&live, b"legacy live").unwrap();
            std::fs::write(&stale, b"stale").unwrap();
            // RC7-FIXES review: the first refused attempt leaves a LOCK
            // file; a second attempt must refuse as well.
            for attempt in 1..=2 {
                passed(child(dir.path(), mode, test));
                assert_eq!(
                    std::fs::read(&live).unwrap(),
                    b"legacy live",
                    "{mode}, attempt {attempt}"
                );
                assert_eq!(std::fs::read(&stale).unwrap(), b"stale");
                assert!(dir.path().join("LOCK").exists());
            }
        }
    }

    /// RC7-FIXES: a container gives the node the same PID on every start, so
    /// the file the last run left names the starting node itself. That file
    /// is stale with or without a LOCK file.
    #[test]
    fn ledger_0201_cleanup_ignores_accounts_named_with_our_own_pid() {
        let test = "ledger_0201_cleanup_ignores_accounts_named_with_our_own_pid";
        if let Some(dir) = std::env::var_os(CHILD) {
            let dir = PathBuf::from(dir);
            let own = dir.join(format!("{}-0.accounts", std::process::id()));
            std::fs::write(&own, b"left by the last run").unwrap();
            configure_global_dir(&dir, true).unwrap();
            assert!(!own.exists());
            return;
        }
        for with_lock in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            if with_lock {
                std::fs::write(dir.path().join("LOCK"), b"").unwrap();
            }
            passed(child(dir.path(), "disk", test));
        }
    }

    /// RC7-FIXES: after a container restart, a reboot or a refused start, a
    /// live process may hold the PID a stale file names. It started after
    /// the file last changed, so it isn't the file's writer.
    #[test]
    fn ledger_0201_cleanup_removes_a_file_older_than_its_pids_process() {
        let test = "ledger_0201_cleanup_removes_a_file_older_than_its_pids_process";
        if let Some(dir) = std::env::var_os(CHILD) {
            configure_global_dir(Path::new(&dir), true).unwrap();
            return;
        }
        for with_lock in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            if with_lock {
                std::fs::write(dir.path().join("LOCK"), b"").unwrap();
            }
            // This test process is alive, is not the child, and started
            // well after the file's (backdated) last change.
            let reused = dir
                .path()
                .join(format!("{}-0.accounts", std::process::id()));
            std::fs::write(&reused, b"left by a dead owner").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&reused)
                .unwrap()
                .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
                .unwrap();
            passed(child(dir.path(), "disk", test));
            assert!(!reused.exists(), "with LOCK: {with_lock}");
        }
    }

    #[test]
    fn ledger_0201_owner_start_time_is_read_from_proc_stat() {
        // The command name may hold spaces and parentheses.
        let pid_stat = "4242 (mina (x) y) S 1 4242 4242 0 -1 4194560 100 0 0 0 \
                        5 6 0 0 20 0 9 0 12345 1000000 200 18446744073709551615";
        let proc_stat = "cpu  1 2 3\nbtime 1793130000\nprocesses 99\n";
        assert_eq!(
            parse_start_time(pid_stat, proc_stat, 100),
            Some(
                std::time::UNIX_EPOCH
                    + std::time::Duration::from_secs(1793130000)
                    + std::time::Duration::from_millis(123450)
            )
        );
        assert_eq!(parse_start_time("4242 (mina) S 1", proc_stat, 100), None);
        assert_eq!(parse_start_time(pid_stat, "cpu 1\n", 100), None);
        assert_eq!(parse_start_time(pid_stat, proc_stat, 0), None);
        #[cfg(target_os = "linux")]
        {
            let started = process_start_time(std::process::id() as libc::pid_t).unwrap();
            let now = std::time::SystemTime::now();
            assert!(
                started <= now + OWNER_START_TOLERANCE,
                "{started:?} {now:?}"
            );
            assert!(!is_kernel_thread(std::process::id() as libc::pid_t));
        }
    }

    #[test]
    fn ledger_0201_cleanup_refuses_a_symlink_directory() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let dir = root.path().join("ledger-accounts");
        std::os::unix::fs::symlink(outside.path(), &dir).unwrap();
        std::fs::write(outside.path().join("2147483646-0.accounts"), b"keep").unwrap();
        assert!(configure_global_dir(&dir, false).is_err());
        assert!(outside.path().join("2147483646-0.accounts").exists());
    }
}
