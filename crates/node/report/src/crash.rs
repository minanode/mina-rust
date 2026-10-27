//! What the node leaves in its work directory for `mina report`: a start
//! record (`last-start.json`) written at startup, and a crash file in
//! `crashes/` for each panic, written by the panic hook.

use std::{
    fs,
    io::{self, Read, Write},
    panic::Location,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        LazyLock, Mutex, OnceLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use backtrace::Backtrace;
use regex::Regex;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::redact::Redactor;

pub const CRASH_DIR: &str = "crashes";
pub const LAST_START: &str = "last-start.json";

/// A panicking node writes at most this many crash files, so that a panic in
/// a loop can't fill the disk.
const MAX_CRASH_FILES_PER_RUN: usize = 16;
const MAX_BACKTRACE_FRAMES: usize = 80;
const MAX_BACKTRACE_BYTES: usize = 48 * 1024;
const MAX_MESSAGE_BYTES: usize = 8 * 1024;
/// Records bigger than this are not read back.
const MAX_RECORD_BYTES: u64 = 1024 * 1024;

/// How the node was started, written to `<work dir>/last-start.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StartRecord {
    pub started_at: String,
    pub unix_ms: u64,
    pub pid: u32,
    pub version: String,
    pub commit: String,
    pub network: String,
    pub http_port: Option<u16>,
    /// Where the node writes `mina.log.*`; `None` without filesystem logging.
    pub log_dir: Option<String>,
    /// The frontier store's base directory, when the store is on.
    pub frontier_store_dir: Option<String>,
    /// The effective worker setting, including one restored from the dashboard.
    #[serde(default)]
    pub snark_worker: Option<StartWorker>,
    /// The command line, already redacted by
    /// [`redact_argv`](crate::redact::redact_argv).
    pub args: Vec<String>,
    /// Names, never values, of the environment variables the node reads that
    /// were set.
    pub env_set: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StartWorker {
    pub public_key: String,
    pub fee: u64,
    pub source: String,
}

/// One panic, written to `<work dir>/crashes/crash-<unix ms>-<pid>-<n>.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrashRecord {
    pub time: String,
    pub unix_ms: u64,
    pub pid: u32,
    pub version: String,
    pub commit: String,
    pub thread: String,
    pub message: String,
    /// `file:line:column` of the panic.
    pub location: Option<String>,
    /// The function that panicked, from the backtrace, when it has symbols.
    pub function: Option<String>,
    pub backtrace: String,
    /// The node's command line, as in [`StartRecord::args`].
    pub args: Vec<String>,
}

struct Crashes {
    dir: PathBuf,
    args: Vec<String>,
    /// Scrubs everything a crash file holds with every secret rule, the
    /// node's own secrets and those in its environment. Network ids stay in
    /// the local file: `mina report` leaves them out unless asked.
    redactor: Mutex<Redactor>,
}

static CRASHES: OnceLock<Crashes> = OnceLock::new();
static WRITTEN: AtomicUsize = AtomicUsize::new(0);

/// Never writes `value` (a password, a key, a key file's path) into a crash
/// file, once [`enable`] has been called.
pub fn register_secret(value: &str) {
    if let Some(crashes) = CRASHES.get() {
        crashes
            .redactor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .add_secret(value);
    }
}

/// From now on, panics are recorded in `<work_dir>/crashes`, scrubbed of
/// `secrets` (passwords, keys, key file paths) and of every other secret the
/// redactor knows; and `start` is written to `<work_dir>/last-start.json`.
/// Both are readable by this user only.
pub fn enable(work_dir: &Path, start: &StartRecord, secrets: &[&str]) -> io::Result<()> {
    let mut redactor = Redactor::from_env(true);
    for secret in secrets {
        redactor.add_secret(secret);
    }
    let mut start = start.clone();
    start.args = start.args.iter().map(|arg| redactor.text(arg)).collect();
    let _ = CRASHES.set(Crashes {
        dir: work_dir.join(CRASH_DIR),
        args: start.args.clone(),
        redactor: Mutex::new(redactor),
    });
    fs::create_dir_all(work_dir)?;
    write_json(&work_dir.join(LAST_START), &start)
}

/// A redactor for a report made inside the node: the secrets of its
/// environment, as `mina report` scrubs them, and those the node registered
/// when it started ([`enable`], [`register_secret`]: passwords, keys, key
/// file paths).
pub fn node_redactor(include_network_ids: bool) -> Redactor {
    let mut redactor = Redactor::from_env(include_network_ids);
    if let Some(crashes) = CRASHES.get() {
        redactor.add_secrets_from(&crashes.redactor.lock().unwrap_or_else(|e| e.into_inner()));
    }
    redactor
}

/// A crash shown on the dashboard's overview counts if it happened at most
/// this long before the node's current start: a restart after the panic.
pub const RECENT_CRASH_BEFORE_START_MS: u64 = 24 * 60 * 60 * 1000;
/// The panic message a notice shows: its first line, at most this long.
pub const NOTICE_MESSAGE_CHARS: usize = 160;

/// That the node crashed: only what the dashboard's notice needs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrashNotice {
    /// When it panicked, in milliseconds since the Unix epoch.
    pub crashed_ms: u64,
    /// The first line of the panic message, short and redacted (network ids
    /// too, which the local crash file keeps).
    pub message: String,
    /// Whether it panicked in this run, after its current start; otherwise
    /// in the run before it, which ended at most
    /// [`RECENT_CRASH_BEFORE_START_MS`] before this one began.
    pub this_run: bool,
    /// When the node last started.
    pub started_ms: Option<u64>,
}

/// The newest crash in `work_dir`, if it is from this run or from just
/// before the node's current start ([`RECENT_CRASH_BEFORE_START_MS`]).
pub fn crash_notice(work_dir: &Path, redactor: &Redactor) -> Option<CrashNotice> {
    let (_, record, _) = newest_crash(work_dir)?;
    let record = record.ok()?;
    let started_ms = read_last_start(work_dir)
        .and_then(Result::ok)
        .map(|start| start.unix_ms);
    let this_run = started_ms.is_none_or(|start| record.unix_ms >= start);
    let recent = this_run
        || started_ms.is_some_and(|start| {
            start.saturating_sub(record.unix_ms) <= RECENT_CRASH_BEFORE_START_MS
        });
    if !recent {
        return None;
    }
    let first = record.message.lines().next().unwrap_or("").trim();
    let mut message: String = redactor
        .text(first)
        .chars()
        .take(NOTICE_MESSAGE_CHARS)
        .collect();
    if redactor.text(first).chars().count() > NOTICE_MESSAGE_CHARS {
        message.push_str("...");
    }
    Some(CrashNotice {
        crashed_ms: record.unix_ms,
        message,
        this_run,
        started_ms,
    })
}

/// Creates `dir` and its missing parents, for this user only (0700); an
/// existing `dir` is made so.
pub fn private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// Writes `bytes` to `path` for this user only (0600), through a new
/// temporary file and a rename, so that no reader sees it half written and an
/// existing file's looser mode doesn't carry over.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .map_or("file".into(), |name| name.to_string_lossy());
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    let _ = fs::remove_file(&tmp);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(tmp, path)
}

/// Names of the environment variables `cli` reads (and every `MINA_*` one)
/// that are set. Values are never read.
pub fn env_names_set(cli: &clap::Command) -> Vec<String> {
    let mut names: Vec<String> = std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .filter(|name| name.starts_with("MINA_") || name.starts_with("RUST_"))
        .collect();
    fn walk(command: &clap::Command, names: &mut Vec<String>) {
        for arg in command.get_arguments() {
            if let Some(name) = arg.get_env().and_then(|n| n.to_str()) {
                if std::env::var_os(name).is_some() {
                    names.push(name.to_owned());
                }
            }
        }
        for sub in command.get_subcommands() {
            walk(sub, names);
        }
    }
    walk(cli, &mut names);
    names.sort();
    names.dedup();
    names
}

/// Called by the panic hook: writes a crash file once [`enable`] has been
/// called. Returns its path.
pub fn record_panic(
    thread: &str,
    message: &str,
    location: Option<&Location<'_>>,
    backtrace: &Backtrace,
) -> Option<PathBuf> {
    let crashes = CRASHES.get()?;
    let n = WRITTEN.fetch_add(1, Ordering::Relaxed);
    if n >= MAX_CRASH_FILES_PER_RUN {
        return None;
    }
    let (frames, function) = format_backtrace(backtrace);
    let redactor = crashes.redactor.lock().unwrap_or_else(|e| e.into_inner());
    let scrub = |text: &str| redactor.text(text);
    let build = mina_node::BuildEnv::get();
    let unix_ms = now_ms();
    let record = CrashRecord {
        time: now_rfc3339(),
        unix_ms,
        pid: std::process::id(),
        version: build.version,
        commit: build.git.commit_hash,
        thread: thread.to_owned(),
        message: truncate(&scrub(message), MAX_MESSAGE_BYTES).to_owned(),
        location: location.map(|l| scrub(&l.to_string())),
        function: function.map(|f| scrub(&f)),
        backtrace: scrub(&frames),
        args: crashes.args.clone(),
    };
    let path = crashes
        .dir
        .join(format!("crash-{unix_ms}-{}-{n}.json", record.pid));
    drop(redactor);
    private_dir(&crashes.dir).ok()?;
    write_json(&path, &record).ok()?;
    Some(path)
}

/// Frames that belong to the panic machinery or this hook, not to the code
/// that panicked.
fn is_machinery(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "backtrace::",
        "<backtrace::",
        "mina_cli::panic_hook",
        "mina::panic_hook",
        "std::panicking::",
        "core::panicking::",
        "std::sys::backtrace::",
        "std::sys_common::backtrace::",
        "core::option::unwrap_failed",
        "core::option::expect_failed",
        "core::result::unwrap_failed",
        "core::cell::panic_",
        "core::slice::index::slice_",
        "core::str::slice_error_fail",
        "core::ops::function::Fn::call",
        "<alloc::boxed::Box<F,A> as core::ops::function::Fn",
    ];
    // `rust_begin_unwind` is `__rustc[<hash>]::rust_begin_unwind` in newer
    // toolchains.
    PREFIXES.iter().any(|p| name.starts_with(p)) || name.contains("rust_begin_unwind")
}

static SYMBOL_HASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"::h[0-9a-f]{16}$").expect("valid regex"));

/// The backtrace from the panicking frame on, one line per frame, and the
/// panicking function's name.
pub fn format_backtrace(backtrace: &Backtrace) -> (String, Option<String>) {
    let mut out = String::new();
    let mut function = None;
    let mut in_hook = true;
    let mut shown = 0;
    for frame in backtrace.frames() {
        for symbol in frame.symbols() {
            let name = symbol
                .name()
                .map(|n| SYMBOL_HASH.replace(&n.to_string(), "").into_owned())
                .unwrap_or_else(|| "<unknown>".to_owned());
            if in_hook {
                // Everything up to the end of the panic machinery is the
                // hook's own stack.
                if is_machinery(&name) || name == "<unknown>" {
                    continue;
                }
                in_hook = false;
            }
            if function.is_none() && !is_machinery(&name) {
                function = Some(name.clone());
            }
            if shown >= MAX_BACKTRACE_FRAMES || out.len() >= MAX_BACKTRACE_BYTES {
                out.push_str("  ...\n");
                return (out, function);
            }
            out.push_str(&format!("{shown:4}: {name}\n"));
            if let (Some(file), Some(line)) = (symbol.filename(), symbol.lineno()) {
                out.push_str(&format!("          at {}:{line}\n", file.display()));
            }
            shown += 1;
        }
    }
    (out, function)
}

/// The newest crash file in `work_dir`, and how many there are.
pub fn newest_crash(work_dir: &Path) -> Option<(PathBuf, Result<CrashRecord, String>, usize)> {
    let mut files: Vec<(u64, PathBuf)> = fs::read_dir(work_dir.join(CRASH_DIR))
        .ok()?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?;
            let ms = name
                .strip_prefix("crash-")?
                .strip_suffix(".json")?
                .split('-')
                .next()?
                .parse()
                .ok()?;
            Some((ms, path))
        })
        .collect();
    files.sort();
    let count = files.len();
    let (_, path) = files.pop()?;
    let record = read_json(&path);
    Some((path, record, count))
}

pub fn read_last_start(work_dir: &Path) -> Option<Result<StartRecord, String>> {
    let path = work_dir.join(LAST_START);
    path.exists().then(|| read_json(&path))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_RECORD_BYTES).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_private(path, &bytes)
}

fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn private_files_replace_looser_ones() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.md");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, b"new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let sub = dir.path().join("a/b");
        private_dir(&sub).unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
        private_dir(&sub).unwrap();
        assert_eq!(
            fs::metadata(&sub).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn newest_crash_is_found_by_time() {
        let dir = tempfile::tempdir().unwrap();
        let crashes = dir.path().join(CRASH_DIR);
        fs::create_dir_all(&crashes).unwrap();
        for (ms, message) in [(1000, "old"), (3000, "new"), (2000, "middle")] {
            let record = CrashRecord {
                time: String::new(),
                unix_ms: ms,
                pid: 1,
                version: String::new(),
                commit: String::new(),
                thread: "main".into(),
                message: message.into(),
                location: None,
                function: None,
                backtrace: String::new(),
                args: vec![],
            };
            write_json(&crashes.join(format!("crash-{ms}-1-0.json")), &record).unwrap();
        }
        fs::write(crashes.join("notes.txt"), "ignored").unwrap();
        let (_, record, count) = newest_crash(dir.path()).unwrap();
        assert_eq!((record.unwrap().message.as_str(), count), ("new", 3));
        assert!(newest_crash(&dir.path().join("none")).is_none());
    }

    #[test]
    fn a_crash_notice_is_short_redacted_and_recent() {
        let dir = tempfile::tempdir().unwrap();
        let crashes = dir.path().join(CRASH_DIR);
        fs::create_dir_all(&crashes).unwrap();
        let crash = |ms: u64, message: &str| {
            let record = CrashRecord {
                time: String::new(),
                unix_ms: ms,
                pid: 1,
                version: String::new(),
                commit: String::new(),
                thread: "main".into(),
                message: message.into(),
                location: None,
                function: None,
                backtrace: String::new(),
                args: vec![],
            };
            write_json(&crashes.join(format!("crash-{ms}-1-0.json")), &record).unwrap();
        };
        let started = |ms: u64| {
            let start = StartRecord {
                started_at: String::new(),
                unix_ms: ms,
                pid: 2,
                version: String::new(),
                commit: String::new(),
                network: "devnet".into(),
                http_port: None,
                log_dir: None,
                frontier_store_dir: None,
                snark_worker: None,
                args: vec![],
                env_set: vec![],
            };
            write_json(&dir.path().join(LAST_START), &start).unwrap();
        };
        let redactor = Redactor::new(false, [], None);
        assert_eq!(crash_notice(dir.path(), &redactor), None, "no crash");

        // The run before this one panicked an hour before this start.
        let day = RECENT_CRASH_BEFORE_START_MS;
        crash(
            10 * day,
            &format!("peer 34.136.105.213 broke {}\nsecond line", "x".repeat(300)),
        );
        started(10 * day + 3_600_000);
        let notice = crash_notice(dir.path(), &redactor).unwrap();
        assert!(!notice.this_run);
        assert_eq!(notice.crashed_ms, 10 * day);
        assert_eq!(notice.started_ms, Some(10 * day + 3_600_000));
        assert!(
            notice.message.starts_with("peer <ip> broke xxx"),
            "{}",
            notice.message
        );
        assert!(!notice.message.contains("second line"));
        assert!(notice.message.ends_with("..."));
        assert!(notice.message.chars().count() <= NOTICE_MESSAGE_CHARS + 3);

        // An old crash, more than a day before this start: no notice.
        started(12 * day);
        assert_eq!(crash_notice(dir.path(), &redactor), None);

        // A panic in this run.
        crash(12 * day + 5, "thread panicked");
        let notice = crash_notice(dir.path(), &redactor).unwrap();
        assert!(notice.this_run);
        assert_eq!(notice.message, "thread panicked");
    }
}
