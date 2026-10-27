//! Warnings, errors and panics from the end of a node log.

use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::LazyLock,
};

use regex::Regex;

use crate::redact::Redactor;

/// Only the end of a log is read: the last 8 MiB.
pub const MAX_SCAN_BYTES: u64 = 8 << 20;
/// Longer lines are cut.
pub const MAX_LINE_CHARS: usize = 400;
/// The part of a line redacted and looked at; ten times what is shown.
const MAX_REDACTED_CHARS: usize = 10 * MAX_LINE_CHARS;
/// Lines kept for an agent to read, from the end of the log.
pub const CONTEXT_LINES: usize = 2000;
/// Lines of any level shown from the very end of the log.
pub const TAIL_LINES: usize = 20;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LogExcerpt {
    /// Where the lines come from (a path, or standard input).
    pub source: String,
    pub scanned_bytes: u64,
    /// Whether only the end of a longer log was read.
    pub truncated: bool,
    pub lines: usize,
    pub warnings: usize,
    pub errors: usize,
    pub panics: usize,
    /// The last warning, error and panic lines.
    pub matched: Vec<String>,
    /// The last lines, whatever their level.
    pub tail: Vec<String>,
    /// The last [`CONTEXT_LINES`] lines, redacted and cut: what `mina
    /// triage` gives the agent besides the report.
    pub context: Vec<String>,
    /// Kinds of the error lines, most frequent first.
    pub error_kinds: Vec<(String, usize)>,
}

static KIND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bkind=(?:"([^"]+)"|([^\s"]+))"#).expect("valid regex"));
static SUMMARY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bsummary="([^"]+)""#).expect("valid regex"));
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[0-9]+\b").expect("valid regex"));
static LONG_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9A-Za-z]{16,}").expect("valid regex"));

#[derive(Debug, Clone, Copy, PartialEq)]
enum Level {
    Warn,
    Error,
    Panic,
    Other,
}

fn level(line: &str) -> Level {
    if line.contains("panicked at") || line.contains("PANIC") {
        return Level::Panic;
    }
    // tracing's format: `<time> <LEVEL> <target>: <message>`.
    match line.split_whitespace().nth(1) {
        Some("ERROR") => Level::Error,
        Some("WARN") => Level::Warn,
        _ => Level::Other,
    }
}

/// The kind of an error line a healthy node logged at every start (RC7-FIXES):
/// up to 0.20.0's release candidates, the transaction pool's account fetch
/// before the ledger was loaded. It is counted and listed, but never a
/// report's title. Only a line between a run's start and its first sync is
/// of this kind ([`scan`]); after the sync the same line is a real failure.
pub const STARTUP_LEDGER_KIND: &str = "Error: tx pool accounts fetched before the ledger loaded";

/// The kind of the same line once the node has synced.
const TX_POOL_FETCH_KIND: &str = "Error: failed to fetch accounts for tx pool";

/// Whether an error kind is no sign of a problem, so not a report's title.
pub fn is_benign_kind(kind: &str) -> bool {
    kind == STARTUP_LEDGER_KIND
}

/// What kind of error a line is: its `kind` field, else its target and the
/// start of its message, with numbers and long tokens taken out.
pub fn error_kind(line: &str) -> String {
    if let Some(c) = KIND.captures(line) {
        let kind = c.get(1).or_else(|| c.get(2)).map_or("", |m| m.as_str());
        if kind == "Error"
            && line.contains(r#"summary="failed to fetch accounts for tx pool""#)
            && line.contains("Ledger unavailable")
        {
            return STARTUP_LEDGER_KIND.to_owned();
        }
        // `kind="Error"` says little: its summary says more.
        return match SUMMARY.captures(line) {
            Some(summary) if kind == "Error" => template(&format!("{kind}: {}", &summary[1])),
            _ => kind.to_owned(),
        };
    }
    let mut words = line.split_whitespace().skip(2);
    // The module, not its whole path: shorter, and it survives moves.
    let target = words.next().unwrap_or("").trim_end_matches(':');
    let target = target.rsplit("::").next().unwrap_or(target);
    let message: Vec<&str> = words.take(6).collect();
    template(&format!("{target}: {}", message.join(" ")))
}

/// `text` with the parts that vary between occurrences of the same problem
/// (numbers, hashes, ids) replaced, so that occurrences compare equal.
pub fn template(text: &str) -> String {
    // Hashes and ids: long, with digits; names like `P2pNetworkError` stay.
    let text = LONG_TOKEN.replace_all(text, |c: &regex::Captures| {
        let digits = c[0].chars().filter(char::is_ascii_digit).count();
        if digits >= 3 {
            "H".to_owned()
        } else {
            c[0].to_owned()
        }
    });
    let text = NUMBER.replace_all(&text, "N");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    text.chars().take(120).collect()
}

fn prefix(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

fn cut(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        line.to_owned()
    } else {
        let mut cut: String = line.chars().take(MAX_LINE_CHARS).collect();
        cut.push_str(" [...]");
        cut
    }
}

/// Scans `text`, the end of a log, keeping the last `max_lines` warning,
/// error and panic lines. A panic's message, on the line after it, is kept
/// too. Each line is redacted before anything is taken from it or it is cut:
/// a cut key or password would no longer be recognised.
pub fn scan(
    source: &str,
    text: &str,
    scanned_bytes: u64,
    truncated: bool,
    max_lines: usize,
    redactor: &Redactor,
) -> LogExcerpt {
    let mut excerpt = LogExcerpt {
        source: redactor.text(source),
        scanned_bytes,
        truncated,
        ..Default::default()
    };
    let mut matched = std::collections::VecDeque::new();
    let mut tail = std::collections::VecDeque::new();
    let mut context = std::collections::VecDeque::new();
    let mut kinds = BTreeMap::<String, (usize, usize)>::new();
    let mut after_panic = false;
    // Whether the node synced since its latest start in the text; a text
    // that starts mid-run counts as synced, so that nothing real is hidden.
    let mut synced_since_start = true;
    let lines = text.lines();
    // A cut-off first line is not a whole line.
    let lines = lines.skip(usize::from(truncated));
    for (index, raw) in lines.enumerate() {
        // Only the start of a line is ever shown: don't redact megabytes of
        // a dump for it. The cut happens before redaction, so it must be well
        // past what is shown.
        let line = redactor.text(prefix(raw, MAX_REDACTED_CHARS));
        let line = line.trim_end();
        excerpt.lines += 1;
        if tail.len() == TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(cut(line));
        if context.len() == CONTEXT_LINES {
            context.pop_front();
        }
        context.push_back(cut(line));
        if line.contains(r#"kind="NodeStart""#) {
            synced_since_start = false;
        } else if line.contains(r#"kind="NodeSynced""#) {
            synced_since_start = true;
        }
        let level = level(line);
        match level {
            Level::Warn => excerpt.warnings += 1,
            Level::Error => {
                excerpt.errors += 1;
                let mut kind = error_kind(line);
                if synced_since_start && kind == STARTUP_LEDGER_KIND {
                    kind = TX_POOL_FETCH_KIND.to_owned();
                }
                let entry = kinds.entry(kind).or_default();
                entry.0 += 1;
                entry.1 = index;
            }
            Level::Panic => excerpt.panics += 1,
            Level::Other => {}
        }
        if level != Level::Other || (after_panic && !line.is_empty()) {
            if matched.len() == max_lines {
                matched.pop_front();
            }
            matched.push_back(cut(line));
        }
        after_panic = level == Level::Panic;
    }
    excerpt.matched = matched.into();
    excerpt.tail = tail.into();
    excerpt.context = context.into();
    let mut kinds: Vec<(String, (usize, usize))> = kinds.into_iter().collect();
    // Most frequent first; on a tie, the latest.
    kinds.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(b.1 .1.cmp(&a.1 .1)));
    excerpt.error_kinds = kinds.into_iter().map(|(kind, (n, _))| (kind, n)).collect();
    excerpt
}

/// The last `budget` bytes of a file: the text, bytes read, and whether
/// the start was cut off.
pub fn read_tail_with_budget(path: &Path, budget: u64) -> io::Result<(String, u64, bool)> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(budget);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(budget).read_to_end(&mut bytes)?;
    Ok((
        String::from_utf8_lossy(&bytes).into_owned(),
        bytes.len() as u64,
        start > 0,
    ))
}

/// The last [`MAX_SCAN_BYTES`] of one explicitly selected file.
pub fn read_tail(path: &Path) -> io::Result<(String, u64, bool)> {
    read_tail_with_budget(path, MAX_SCAN_BYTES)
}

/// The last [`MAX_SCAN_BYTES`] of standard input.
pub fn read_stdin_tail() -> io::Result<(String, u64, bool)> {
    let mut kept = Vec::new();
    let mut truncated = false;
    let mut buffer = vec![0; 1 << 20];
    let mut stdin = io::stdin().lock();
    loop {
        let n = stdin.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        kept.extend_from_slice(&buffer[..n]);
        let excess = kept.len().saturating_sub(MAX_SCAN_BYTES as usize);
        if excess > 0 {
            kept.drain(..excess);
            truncated = true;
        }
    }
    Ok((
        String::from_utf8_lossy(&kept).into_owned(),
        kept.len() as u64,
        truncated,
    ))
}

/// O1's current log and numbered rotations, newest first. An unrelated
/// `mina.log.*` file is not part of the rotation set.
pub fn find_logs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let name = name.to_str()?;
            let index = if name == "mina.log" {
                0
            } else {
                name.strip_prefix("mina.log.")?
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)?
            };
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            Some((index, entry.path()))
        })
        .collect();
    paths.sort_by_key(|(index, _)| *index);
    paths.into_iter().map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
2026-10-18T19:14:52.961651Z  INFO mina_tree::proofs::circuit_blobs: time=\"1\" kind=\"ledger proofs\" fetching
2026-10-18T19:14:53.401738Z  WARN mina_tree::proofs::verifiers: Cannot load verifier index time=\"2\"
2026-10-18T19:14:58.753928Z \x1b[31mERROR\x1b[0m mina_core::log::action: time=\"3\" kind=\"P2pNetworkSchedulerSelectError\" addr=1.2.3.4:10003
2026-10-18T19:14:59.000000Z ERROR mina_node::ledger: failed to apply block 12345 hash 3NKjZ5fjms6BMaH4aq7DopPGyMY7PbG6vhRsX5XnYRxih8i9G7dj
2026-10-18T19:15:00.000000Z ERROR mina_core::log::action: time=\"4\" kind=\"P2pNetworkSchedulerSelectError\" addr=5.6.7.8:10003

thread 'main' panicked at crates/node/src/foo.rs:12:5:
called `Option::unwrap()` on a `None` value
2026-10-18T19:15:01.000000Z  INFO mina_node: done
";

    #[test]
    fn levels_kinds_and_panics_are_counted() {
        let e = scan(
            "mina.log",
            LOG,
            LOG.len() as u64,
            false,
            100,
            &Redactor::new(true, [], None),
        );
        assert_eq!((e.lines, e.warnings, e.errors, e.panics), (9, 1, 3, 1));
        assert_eq!(e.matched.len(), 6, "{:#?}", e.matched);
        assert!(
            e.matched[1].contains("Z ERROR mina_core"),
            "ANSI stripped: {}",
            e.matched[1]
        );
        assert_eq!(e.matched[5], "called `Option::unwrap()` on a `None` value");
        assert_eq!(
            e.error_kinds[0],
            ("P2pNetworkSchedulerSelectError".into(), 2)
        );
        assert_eq!(e.error_kinds[1].0, "ledger: failed to apply block N hash");
        assert_eq!(
            e.tail.last().unwrap(),
            "2026-10-18T19:15:01.000000Z  INFO mina_node: done"
        );
    }

    #[test]
    fn generic_kinds_take_their_summary() {
        let line = "2026-10-22T21:15:17Z ERROR mina_node::transaction_pool: time=\"1\" \
                    kind=\"Error\" summary=\"failed to fetch accounts for tx pool\" error=\"x\"";
        assert_eq!(
            error_kind(line),
            "Error: failed to fetch accounts for tx pool"
        );
        let line = "2026-10-22T21:15:17Z ERROR m: kind=\"LedgerError\" summary=\"y\"";
        assert_eq!(error_kind(line), "LedgerError");
    }

    /// RC7-FIXES: the line every start of an RC6 node logged, as the Mac
    /// install test's `mina.log` had it.
    #[test]
    fn rc7_the_startup_ledger_error_has_its_own_benign_kind() {
        let line = "2026-10-27T19:06:57.600000Z ERROR mina_node::transaction_pool::transaction_pool_effects: \
                    time=\"1\" kind=\"Error\" summary=\"failed to fetch accounts for tx pool\" \
                    error=\"ledger jwrCSCRNpNx3ZBh6D4bUAnZtqPacKXXe6veGC9kQfHrxWqmEbJS, error: \\\"Ledger unavailable\\\"\"";
        assert_eq!(error_kind(line), STARTUP_LEDGER_KIND);
        assert!(is_benign_kind(&error_kind(line)));
        let real = line.replace("Ledger unavailable", "account lookup failed");
        assert_eq!(error_kind(&real), TX_POOL_FETCH_KIND);
        assert!(!is_benign_kind(&error_kind(&real)));
    }

    /// RC7-FIXES review: only the line before a run's first sync is benign.
    /// The same line after the sync, or in a text that starts mid-run, is a
    /// real failure and may be a report's title.
    #[test]
    fn rc7_the_tx_pool_ledger_error_after_sync_is_a_real_error() {
        let fetch = "ERROR mina_node::transaction_pool: kind=\"Error\" \
                     summary=\"failed to fetch accounts for tx pool\" \
                     error=\"ledger jx, error: \\\"Ledger unavailable\\\"\"";
        let start = "INFO mina::commands::node: kind=\"NodeStart\" summary=\"starting the node\"";
        let synced = "INFO mina_node::logger::status: kind=\"NodeSynced\" summary=\"synced, 40s after the start\"";
        let at = |time: u32, line: &str| format!("2026-10-27T20:00:{time:02}Z {line}\n");
        let r = Redactor::new(false, [], None);
        let kinds = |text: String| scan("mina.log", &text, 1, false, 10, &r).error_kinds;
        // At a start: benign.
        let text = at(1, start) + &at(2, fetch) + &at(3, synced);
        assert_eq!(kinds(text), vec![(STARTUP_LEDGER_KIND.to_owned(), 1)]);
        // After the sync, twice, beside the startup one: the real one leads.
        let text = at(1, start) + &at(2, fetch) + &at(3, synced) + &at(4, fetch) + &at(5, fetch);
        assert_eq!(
            kinds(text),
            vec![
                (TX_POOL_FETCH_KIND.to_owned(), 2),
                (STARTUP_LEDGER_KIND.to_owned(), 1)
            ]
        );
        // A text with no start in it: real.
        assert_eq!(
            kinds(at(4, fetch)),
            vec![(TX_POOL_FETCH_KIND.to_owned(), 1)]
        );
    }

    #[test]
    fn bounds_hold() {
        let long = format!("2026-10-18T19:14:53Z  WARN t: {}\n", "x".repeat(10_000));
        let text = long.repeat(500);
        let e = scan(
            "big",
            &text,
            text.len() as u64,
            true,
            50,
            &Redactor::new(false, [], None),
        );
        assert_eq!(e.lines, 499, "the cut-off first line is dropped");
        assert_eq!(e.matched.len(), 50);
        assert!(e
            .matched
            .iter()
            .all(|l| l.chars().count() <= MAX_LINE_CHARS + 6));
        assert_eq!(e.tail.len(), TAIL_LINES);
        assert_eq!(e.context.len(), 499);
    }

    #[test]
    fn the_tail_of_a_file_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mina.log.1");
        fs::write(&path, LOG).unwrap();
        fs::write(dir.path().join("other.txt"), "x").unwrap();
        fs::write(dir.path().join("mina.log.2026-10-18"), "old log").unwrap();
        let current = dir.path().join("mina.log");
        fs::write(&current, "new log").unwrap();
        assert_eq!(find_logs(dir.path()), vec![current, path.clone()]);
        let (text, len, truncated) = read_tail(&path).unwrap();
        assert_eq!(
            (text.as_str(), len, truncated),
            (LOG, LOG.len() as u64, false)
        );
    }
}
