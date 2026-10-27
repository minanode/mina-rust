//! A bug report from what was collected: a title with a fingerprint, the
//! full Markdown body, and a short summary for when the body doesn't fit in
//! a link. Everything shown passes the [`Redactor`].

use std::fmt::Write;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    crash::{CrashRecord, StartRecord},
    logs::{self, template, LogExcerpt},
    redact::Redactor,
    system::{gib, ProcessInfo, SystemInfo},
};

/// GitHub refuses issue bodies over 65536 characters.
pub const MAX_BODY_CHARS: usize = 60_000;
const MAX_BACKTRACE_LINES: usize = 120;

/// What the running node said about itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeInfo {
    pub network: Option<String>,
    pub chain_id: Option<String>,
    pub uptime_secs: Option<u64>,
    pub sync_status: Option<String>,
    pub sync_phase: Option<String>,
    pub best_tip_height: Option<u64>,
    pub best_tip_age_secs: Option<u64>,
    pub highest_peer_tip: Option<u64>,
    pub peers_connected: usize,
    pub peers_libp2p: usize,
    pub peers_webrtc: usize,
    pub producer: Option<bool>,
    pub worker: Option<bool>,
    pub build_commit: Option<String>,
}

impl NodeInfo {
    /// From the node's `/status`, `/build_env` and `/snarker/config`
    /// answers; the last two may be missing.
    pub fn from_json(
        status: &Value,
        build: Option<&Value>,
        snarker: Option<&Value>,
        now_ms: u64,
    ) -> Self {
        let text = |pointer: &str| {
            status
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let peers = status
            .get("peers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let connected: Vec<&Value> = peers
            .iter()
            .filter(|p| p.get("connection_status").and_then(Value::as_str) == Some("Connected"))
            .collect();
        let libp2p = connected
            .iter()
            .filter(|p| p.get("is_libp2p").and_then(Value::as_bool) == Some(true))
            .count();
        Self {
            network: text("/network"),
            chain_id: text("/chain_id"),
            uptime_secs: status.get("uptime_secs").and_then(Value::as_u64),
            sync_status: text("/transition_frontier/sync/status"),
            sync_phase: text("/transition_frontier/sync/phase"),
            best_tip_height: status
                .pointer("/transition_frontier/best_tip/height")
                .and_then(Value::as_u64),
            best_tip_age_secs: status
                .pointer("/transition_frontier/best_tip/timestamp_ms")
                .and_then(Value::as_u64)
                .and_then(|ms| now_ms.checked_sub(ms))
                .map(|ms| ms / 1000),
            highest_peer_tip: connected
                .iter()
                .filter_map(|p| p.get("best_tip_height").and_then(Value::as_u64))
                .max(),
            peers_connected: connected.len(),
            peers_libp2p: libp2p,
            peers_webrtc: connected.len() - libp2p,
            producer: status.get("block_producer").map(|p| !p.is_null()),
            worker: snarker.map(|s| !s.is_null()),
            build_commit: build
                .and_then(|b| b.pointer("/git/commit_hash"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        }
    }
}

/// The frontier store check's result.
#[derive(Debug, Clone, PartialEq)]
pub struct StoreCheck {
    pub dir: String,
    /// `None` when the check could not run.
    pub passed: Option<bool>,
    pub output: String,
    pub note: Option<String>,
}

/// Everything `mina report` collected, before redaction.
pub struct Inputs {
    pub generated_at: String,
    pub description: Option<String>,
    pub build: mina_node::BuildEnv,
    pub system: SystemInfo,
    pub work_dir: String,
    pub node_url: String,
    pub node: Result<NodeInfo, String>,
    pub start: Option<Result<StartRecord, String>>,
    /// For the start record's pid: `None` when it isn't running.
    pub start_process: Option<ProcessInfo>,
    /// The newest crash file's name, its record, and how many there are.
    pub crash: Option<(String, Result<CrashRecord, String>, usize)>,
    pub logs: Vec<LogExcerpt>,
    pub log_notes: Vec<String>,
    pub store: Option<StoreCheck>,
    /// Why the store wasn't checked, when it wasn't on purpose.
    pub store_skipped: Option<String>,
}

/// A finished report. All of it is redacted.
#[derive(Debug, Clone)]
pub struct Report {
    pub title: String,
    pub fingerprint: String,
    pub markdown: String,
    /// The essentials, for a link when the full report is too long for one.
    pub summary: String,
}

/// What the report is about, for its title and fingerprint.
struct Subject {
    fingerprint: String,
    title: String,
    what: String,
}

fn short(text: &str, max: usize) -> String {
    let text = text.lines().next().unwrap_or("").trim();
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut cut: String = text.chars().take(max.saturating_sub(3)).collect();
        cut.push_str("...");
        cut
    }
}

/// The first `n` characters of a commit hash.
fn abbrev(hash: &str, n: usize) -> &str {
    hash.get(..n).unwrap_or(hash)
}

fn hash8(input: &str) -> String {
    hex::encode(&Sha256::digest(input.as_bytes())[..4])
}

/// A location's file, the same for every checkout, build and version: from
/// `crates/` in this repository, from the crate's name for a dependency.
pub fn normalize_file(file: &str) -> String {
    let file = file.replace('\\', "/");
    if let Some(at) = file.find("/registry/src/") {
        // .../registry/src/<index>/<crate>-<version>/src/lib.rs
        let mut parts = file[at + "/registry/src/".len()..].splitn(3, '/').skip(1);
        let krate = parts.next().unwrap_or("");
        let krate = match krate.rfind('-') {
            Some(dash) if krate[dash + 1..].starts_with(|c: char| c.is_ascii_digit()) => {
                &krate[..dash]
            }
            _ => krate,
        };
        return format!("{krate}/{}", parts.next().unwrap_or(""));
    }
    for marker in ["/crates/", "/library/"] {
        if let Some(at) = file.rfind(marker) {
            return file[at + 1..].to_owned();
        }
    }
    file
}

/// `(file, line)` of a `file:line:column` location.
fn split_location(location: &str) -> (String, Option<String>) {
    let mut parts = location.rsplitn(3, ':');
    let (column, line, file) = (parts.next(), parts.next(), parts.next());
    match (file, line, column) {
        (Some(file), Some(line), Some(_)) => (normalize_file(file), Some(line.to_owned())),
        _ => (normalize_file(location), None),
    }
}

/// The fingerprint of a panic: where it happened and what it said, without
/// the line number when the function is known (lines move between versions)
/// or the numbers and hashes in the message.
pub fn panic_fingerprint(crash: &CrashRecord) -> String {
    let (file, line) = crash
        .location
        .as_deref()
        .map(split_location)
        .unwrap_or_default();
    let place = crash.function.clone().or(line).unwrap_or_default();
    hash8(&format!(
        "panic\n{file}\n{place}\n{}",
        template(&crash.message)
    ))
}

pub fn error_fingerprint(kind: &str) -> String {
    hash8(&format!("error\n{kind}"))
}

fn subject(inputs: &Inputs, crash: Option<&CrashRecord>, r: &Redactor) -> Subject {
    if let Some(crash) = crash {
        let (file, line) = crash
            .location
            .as_deref()
            .map(split_location)
            .unwrap_or_default();
        let name = file.rsplit('/').next().unwrap_or("");
        let at = match line {
            Some(line) => format!("{name}:{line}"),
            None => name.to_owned(),
        };
        let fingerprint = panic_fingerprint(crash);
        // Redacted before it's cut: a cut key is no longer recognisable.
        let message = short(&r.text(&crash.message), 70);
        return Subject {
            title: format!("Crash: {message} at {at} [fp:{fingerprint}]"),
            what: format!(
                "panic at {}",
                r.text(crash.location.as_deref().unwrap_or("?"))
            ),
            fingerprint,
        };
    }
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for log in &inputs.logs {
        for (kind, n) in &log.error_kinds {
            if logs::is_benign_kind(kind) {
                continue;
            }
            match kinds.iter_mut().find(|(k, _)| k == kind) {
                Some((_, total)) => *total += n,
                None => kinds.push((kind.clone(), *n)),
            }
        }
    }
    // Stable sort: on a tie, the order of the first log (most frequent, then
    // latest) wins.
    kinds.sort_by(|a, b| b.1.cmp(&a.1));
    if let Some((kind, n)) = kinds.first() {
        let kind = r.text(kind);
        let fingerprint = error_fingerprint(&kind);
        return Subject {
            title: format!("Error: {} [fp:{fingerprint}]", short(&kind, 90)),
            what: format!("most frequent error in the logs: `{kind}` ({n} lines)"),
            fingerprint,
        };
    }
    let (state, about) = match &inputs.node {
        Ok(node) => {
            let status = node.sync_status.as_deref().unwrap_or("unknown");
            let network = node.network.as_deref().unwrap_or("Mina");
            (
                format!("state\n{status}"),
                format!("{network} node {status}"),
            )
        }
        Err(_) => ("unreachable".to_owned(), "node not reachable".to_owned()),
    };
    let fingerprint = hash8(&state);
    let about = match &inputs.description {
        Some(text) if !text.trim().is_empty() => short(&r.text(text), 80),
        _ => about,
    };
    Subject {
        title: format!("Report: {about} [fp:{fingerprint}]"),
        what: "no crash and no errors in the logs".to_owned(),
        fingerprint,
    }
}

/// The crash the title is about: one from the latest start of the node, not
/// one from an earlier run.
fn current_crash(inputs: &Inputs) -> Option<&CrashRecord> {
    let crash = inputs.crash.as_ref()?.1.as_ref().ok()?;
    match &inputs.start {
        Some(Ok(start)) => {
            (crash.pid == start.pid || crash.unix_ms >= start.unix_ms).then_some(crash)
        }
        _ => Some(crash),
    }
}

fn fence(text: &str) -> String {
    // A log line must not close the code block.
    text.replace("```", "'''")
}

fn yes_no(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

fn on_off(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "on",
        Some(false) => "off",
        None => "unknown",
    }
}

fn row(out: &mut String, key: &str, value: impl std::fmt::Display) {
    let value = value.to_string().replace('|', "\\|").replace('\n', " ");
    let _ = writeln!(out, "| {key} | {value} |");
}

fn environment(out: &mut String, inputs: &Inputs) {
    let b = &inputs.build;
    let s = &inputs.system;
    let _ = writeln!(out, "### Environment\n\n| | |\n|---|---|");
    row(
        out,
        "Version",
        format!(
            "{} (commit `{}`, branch {}, built {})",
            b.version,
            abbrev(&b.git.commit_hash, 12),
            b.git.branch,
            b.time
        ),
    );
    row(
        out,
        "Build",
        format!(
            "{} opt-level {}, {}, features `{}`, rustc {} ({})",
            if b.cargo.is_debug { "debug" } else { "release" },
            b.cargo.opt_level,
            b.cargo.target,
            if b.cargo.features.is_empty() {
                "-"
            } else {
                &b.cargo.features
            },
            b.rustc.version,
            b.rustc.channel
        ),
    );
    row(
        out,
        "OS",
        format!(
            "{}, kernel {}, {}",
            s.os,
            s.kernel.as_deref().unwrap_or("unknown"),
            s.arch
        ),
    );
    let features = match (s.bmi2, s.adx) {
        (None, None) => String::new(),
        (bmi2, adx) => format!(", BMI2 {}, ADX {}", yes_no(bmi2), yes_no(adx)),
    };
    row(
        out,
        "CPU",
        format!(
            "{}, {} threads{features}",
            s.cpu_model.as_deref().unwrap_or("unknown"),
            s.cpu_threads
        ),
    );
    row(
        out,
        "RAM",
        match (s.ram_total_bytes, s.ram_available_bytes) {
            (Some(total), Some(available)) => {
                format!("{} total, {} available", gib(total), gib(available))
            }
            (Some(total), None) => format!("{} total", gib(total)),
            _ => "unknown".to_owned(),
        },
    );
    row(
        out,
        "Disk",
        match s.disk_free_bytes {
            Some(free) => format!("{} free for the work dir `{}`", gib(free), inputs.work_dir),
            None => format!("unknown for the work dir `{}`", inputs.work_dir),
        },
    );
    out.push('\n');
}

fn node(out: &mut String, inputs: &Inputs) {
    let _ = writeln!(out, "### Node\n");
    let node = match &inputs.node {
        Ok(node) => node,
        Err(error) => {
            let _ = writeln!(out, "Not reachable at {}: {error}\n", inputs.node_url);
            process(out, inputs);
            return;
        }
    };
    let _ = writeln!(out, "| | |\n|---|---|");
    row(out, "Address", &inputs.node_url);
    row(
        out,
        "Network",
        format!(
            "{} (chain id `{}`)",
            node.network.as_deref().unwrap_or("unknown"),
            node.chain_id
                .as_deref()
                .map_or("unknown".to_owned(), |c| short(c, 16))
        ),
    );
    let ours = &inputs.build.git.commit_hash;
    row(
        out,
        "Build",
        match node.build_commit.as_deref() {
            Some(commit) if commit == ours => format!("`{}`, as this `mina`", abbrev(commit, 12)),
            Some(commit) => format!(
                "`{}`, **not** this `mina`'s `{}`",
                abbrev(commit, 12),
                abbrev(ours, 12)
            ),
            None => "unknown".to_owned(),
        },
    );
    row(
        out,
        "Sync",
        format!(
            "{} ({})",
            node.sync_status.as_deref().unwrap_or("unknown"),
            node.sync_phase.as_deref().unwrap_or("unknown phase")
        ),
    );
    row(
        out,
        "Best tip",
        match (node.best_tip_height, node.best_tip_age_secs) {
            (Some(height), Some(age)) => format!("height {height}, {age} s old"),
            (Some(height), None) => format!("height {height}"),
            _ => "none".to_owned(),
        },
    );
    row(
        out,
        "Highest peer tip",
        node.highest_peer_tip
            .map_or("unknown".to_owned(), |h| format!("height {h}")),
    );
    row(
        out,
        "Peers",
        format!(
            "{} connected ({} libp2p, {} WebRTC)",
            node.peers_connected, node.peers_libp2p, node.peers_webrtc
        ),
    );
    row(out, "Block producer", on_off(node.producer));
    row(out, "SNARK worker", on_off(node.worker));
    row(
        out,
        "Uptime",
        node.uptime_secs
            .map_or("unknown".to_owned(), |s| format!("{s} s")),
    );
    out.push('\n');
    process(out, inputs);
}

fn process(out: &mut String, inputs: &Inputs) {
    let Some(Ok(start)) = &inputs.start else {
        return;
    };
    let _ = match inputs.start_process {
        Some(ProcessInfo { rss_bytes }) => writeln!(
            out,
            "Process {} (from `last-start.json`): running{}.\n",
            start.pid,
            rss_bytes.map_or(String::new(), |rss| format!(", {} resident", gib(rss)))
        ),
        None => writeln!(
            out,
            "Process {} (from `last-start.json`): not running.\n",
            start.pid
        ),
    };
}

fn started_with(out: &mut String, inputs: &Inputs) {
    let _ = writeln!(out, "### Started with\n");
    match &inputs.start {
        Some(Ok(start)) => {
            let _ = writeln!(
                out,
                "At {} (pid {}, version {}, commit `{}`, network {}):\n\n```\n{}\n```\n",
                start.started_at,
                start.pid,
                start.version,
                abbrev(&start.commit, 12),
                start.network,
                fence(&start.args.join(" "))
            );
            if let Some(worker) = &start.snark_worker {
                let _ = writeln!(
                    out,
                    "SNARK worker: {} (fee {}, {}).\n",
                    worker.public_key, worker.fee, worker.source
                );
            }
            if !start.env_set.is_empty() {
                let _ = writeln!(
                    out,
                    "Environment variables set (values not included): {}\n",
                    start
                        .env_set
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        Some(Err(error)) => {
            let _ = writeln!(out, "`last-start.json` is unreadable: {error}\n");
        }
        None => {
            let _ = writeln!(
                out,
                "No `last-start.json` in the work dir: the node has not started there with this \
                 version.\n"
            );
        }
    }
}

fn crash(out: &mut String, inputs: &Inputs, backtrace_lines: usize) {
    let _ = writeln!(out, "### Last crash\n");
    let Some((name, record, count)) = &inputs.crash else {
        let _ = writeln!(out, "No crash files in the work dir.\n");
        return;
    };
    let record = match record {
        Ok(record) => record,
        Err(error) => {
            let _ = writeln!(out, "`crashes/{name}` is unreadable: {error}\n");
            return;
        }
    };
    let when = if current_crash(inputs).is_some() {
        "the latest run"
    } else {
        "an earlier run, not the latest"
    };
    let _ = writeln!(
        out,
        "`crashes/{name}` ({count} crash file{} in all), from {when}:\n",
        if *count == 1 { "" } else { "s" }
    );
    let _ = writeln!(out, "| | |\n|---|---|");
    row(out, "Time", &record.time);
    row(
        out,
        "Version",
        format!(
            "{} (commit `{}`)",
            record.version,
            abbrev(&record.commit, 12)
        ),
    );
    row(out, "Thread", &record.thread);
    row(
        out,
        "Location",
        record.location.as_deref().unwrap_or("unknown"),
    );
    row(
        out,
        "Function",
        record.function.as_deref().unwrap_or("unknown"),
    );
    let _ = writeln!(out, "\nMessage:\n\n```\n{}\n```\n", fence(&record.message));
    let lines: Vec<&str> = record.backtrace.lines().collect();
    if !lines.is_empty() {
        let shown = lines.len().min(backtrace_lines);
        let _ = writeln!(
            out,
            "<details><summary>Backtrace ({shown} of {} lines)</summary>\n\n```\n{}\n```\n\n</details>\n",
            lines.len(),
            fence(&lines[..shown].join("\n"))
        );
    }
}

fn logs(out: &mut String, inputs: &Inputs, max_lines: usize) {
    let _ = writeln!(out, "### Recent warnings and errors\n");
    for note in &inputs.log_notes {
        let _ = writeln!(out, "{note}\n");
    }
    for log in &inputs.logs {
        let _ = writeln!(
            out,
            "`{}`: {} warnings, {} errors, {} panics in the last {} lines ({} KiB{}).\n",
            log.source,
            log.warnings,
            log.errors,
            log.panics,
            log.lines,
            log.scanned_bytes / 1024,
            if log.truncated {
                ", the end of a longer log"
            } else {
                ""
            }
        );
        if !log.error_kinds.is_empty() {
            let _ = writeln!(out, "Errors by kind:\n");
            for (kind, n) in log.error_kinds.iter().take(10) {
                let _ = writeln!(out, "- `{}`: {n}", kind.replace('`', "'"));
            }
            out.push('\n');
        }
        let matched = &log.matched[log.matched.len().saturating_sub(max_lines)..];
        if !matched.is_empty() {
            let _ = writeln!(
                out,
                "<details><summary>The last {} warning, error and panic lines</summary>\n\n```\n{}\n```\n\n</details>\n",
                matched.len(),
                fence(&matched.join("\n"))
            );
        }
        if !log.tail.is_empty() {
            let _ = writeln!(
                out,
                "<details><summary>The last {} lines</summary>\n\n```\n{}\n```\n\n</details>\n",
                log.tail.len(),
                fence(&log.tail.join("\n"))
            );
        }
    }
}

fn store(out: &mut String, inputs: &Inputs) {
    let _ = writeln!(out, "### Frontier store\n");
    let Some(store) = &inputs.store else {
        match &inputs.store_skipped {
            Some(reason) => {
                let _ = writeln!(out, "Not checked: {reason}.\n");
            }
            None => {
                let _ = writeln!(out, "No frontier store in the work dir.\n");
            }
        }
        return;
    };
    let verdict = match store.passed {
        Some(true) => "passed",
        Some(false) => "**failed**",
        None => "did not run",
    };
    let _ = writeln!(
        out,
        "`mina node frontier-store-check` on `{}`: {verdict}.\n",
        store.dir
    );
    if let Some(note) = &store.note {
        let _ = writeln!(out, "{note}\n");
    }
    if !store.output.trim().is_empty() {
        let _ = writeln!(
            out,
            "<details><summary>Output</summary>\n\n```\n{}\n```\n\n</details>\n",
            fence(store.output.trim_end())
        );
    }
}

fn header(out: &mut String, inputs: &Inputs, subject: &Subject) {
    let _ = writeln!(
        out,
        "**Fingerprint:** `fp:{}`: {}\n",
        subject.fingerprint, subject.what
    );
    let _ = match inputs.description.as_deref().map(str::trim) {
        Some(text) if !text.is_empty() => writeln!(out, "**What happened:** {text}\n"),
        _ => writeln!(
            out,
            "**What happened:** _(describe what you did, what you saw and what you expected)_\n"
        ),
    };
}

fn footer(out: &mut String, inputs: &Inputs, r: &Redactor) {
    let _ = writeln!(
        out,
        "---\n_Collected by `mina report` ({}) at {}. Keys, passwords, key files and their paths \
         and secret environment variables are left out{}._",
        inputs.build.version,
        inputs.generated_at,
        if r.include_network_ids() {
            ""
        } else {
            "; so are IP addresses and peer ids (`--include-network-ids` keeps them)"
        }
    );
}

fn render(
    inputs: &Inputs,
    subject: &Subject,
    log_lines: usize,
    backtrace_lines: usize,
    r: &Redactor,
) -> String {
    let mut out = String::new();
    header(&mut out, inputs, subject);
    environment(&mut out, inputs);
    node(&mut out, inputs);
    started_with(&mut out, inputs);
    crash(&mut out, inputs, backtrace_lines);
    logs(&mut out, inputs, log_lines);
    store(&mut out, inputs);
    footer(&mut out, inputs, r);
    r.text(&out)
}

fn render_summary(inputs: &Inputs, subject: &Subject, r: &Redactor) -> String {
    let mut out = String::new();
    header(&mut out, inputs, subject);
    environment(&mut out, inputs);
    node(&mut out, inputs);
    if let Some(crash) = current_crash(inputs) {
        let _ = writeln!(
            out,
            "**Crash:** `{}` at `{}`\n",
            short(&r.text(&crash.message), 200).replace('`', "'"),
            crash.location.as_deref().unwrap_or("?")
        );
    }
    for log in &inputs.logs {
        if let Some((kind, n)) = log.error_kinds.first() {
            let _ = writeln!(
                out,
                "**Logs:** {} warnings, {} errors (most: `{}`, {n}), {} panics\n",
                log.warnings,
                log.errors,
                kind.replace('`', "'"),
                log.panics
            );
        }
    }
    footer(&mut out, inputs, r);
    r.text(&out)
}

/// Builds the report, within [`MAX_BODY_CHARS`]: fewer log and backtrace
/// lines when it would be longer.
pub fn build(inputs: &Inputs, r: &Redactor, log_lines: usize) -> Report {
    let crash = current_crash(inputs);
    let subject = subject(inputs, crash, r);
    let (mut log_lines, mut backtrace_lines) = (log_lines, MAX_BACKTRACE_LINES);
    let markdown = loop {
        let markdown = render(inputs, &subject, log_lines, backtrace_lines, r);
        if markdown.chars().count() <= MAX_BODY_CHARS || (log_lines == 0 && backtrace_lines == 0) {
            break markdown;
        }
        if log_lines > 0 {
            log_lines /= 2;
        } else {
            backtrace_lines /= 2;
        }
    };
    let markdown: String = markdown.chars().take(MAX_BODY_CHARS).collect();
    Report {
        title: short(&subject.title, 200),
        fingerprint: subject.fingerprint.clone(),
        summary: render_summary(inputs, &subject, r),
        markdown,
    }
}

/// Inputs for the tests here and in `mina-cli` (feature `test-support`).
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use super::*;

    pub fn crash_record(message: &str, location: &str, function: Option<&str>) -> CrashRecord {
        CrashRecord {
            time: "2026-10-22T20:00:00Z".into(),
            unix_ms: 2_000,
            pid: 7,
            version: "v0.1".into(),
            commit: "a".repeat(40),
            thread: "main".into(),
            message: message.into(),
            location: Some(location.into()),
            function: function.map(str::to_owned),
            backtrace: "   0: mina_node::foo::bar\n".into(),
            args: vec![],
        }
    }

    pub fn start(args: Vec<String>) -> StartRecord {
        StartRecord {
            started_at: "2026-10-22T19:00:00Z".into(),
            unix_ms: 1_000,
            pid: 7,
            version: "v0.1".into(),
            commit: "a".repeat(40),
            network: "devnet".into(),
            http_port: Some(3650),
            log_dir: None,
            frontier_store_dir: None,
            snark_worker: None,
            args,
            env_set: vec!["MINA_PRIVKEY_PASS".into()],
        }
    }

    pub fn inputs() -> Inputs {
        Inputs {
            generated_at: "2026-10-22T20:01:00Z".into(),
            description: None,
            build: mina_node::BuildEnv::get(),
            system: SystemInfo {
                os: "Ubuntu 24.04 LTS".into(),
                kernel: Some("6.8.0".into()),
                arch: "x86_64",
                cpu_model: Some("Test CPU".into()),
                cpu_threads: 8,
                bmi2: Some(true),
                adx: Some(false),
                ram_total_bytes: Some(32 << 30),
                ram_available_bytes: Some(16 << 30),
                disk_free_bytes: Some(100 << 30),
            },
            work_dir: "/home/tester/.mina".into(),
            node_url: "http://127.0.0.1:3650".into(),
            node: Err("connection refused".into()),
            start: None,
            start_process: None,
            crash: None,
            logs: vec![],
            log_notes: vec![],
            store: None,
            store_skipped: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{test_support::*, *};
    use crate::logs;
    use serde_json::json;

    #[test]
    fn node_status_is_read() {
        let status = json!({
            "network": "devnet",
            "chain_id": "ebfce0d570bc22eb041e1a7b0a46bbc3ed5d0a1030ef2ab5e36eef908b93eba8",
            "uptime_secs": 60,
            "transition_frontier": {
                "best_tip": {"hash": "3N", "height": 100, "timestamp_ms": 10_000},
                "sync": {"status": "Synced", "phase": "Synced"}
            },
            "peers": [
                {"connection_status": "Connected", "is_libp2p": true, "best_tip_height": 101},
                {"connection_status": "Connected", "is_libp2p": false, "best_tip_height": 99},
                {"connection_status": "Connecting", "is_libp2p": true, "best_tip_height": 500}
            ],
            "block_producer": null
        });
        let node = NodeInfo::from_json(
            &status,
            Some(&json!({"git": {"commit_hash": "abc"}})),
            Some(&json!({"public_key": "B62", "fee": "1"})),
            25_000,
        );
        assert_eq!(node.best_tip_age_secs, Some(15));
        assert_eq!(node.highest_peer_tip, Some(101));
        assert_eq!(
            (node.peers_connected, node.peers_libp2p, node.peers_webrtc),
            (2, 1, 1)
        );
        assert_eq!((node.producer, node.worker), (Some(false), Some(true)));
        assert_eq!(node.build_commit.as_deref(), Some("abc"));
        let old = NodeInfo::from_json(&json!({"peers": []}), None, None, 0);
        assert_eq!(
            (old.producer, old.worker, old.sync_status),
            (None, None, None)
        );
    }

    #[test]
    fn panic_fingerprint_is_stable() {
        let a = crash_record(
            "index out of bounds: the len is 3 but the index is 5",
            "crates/node/src/foo.rs:120:9",
            Some("mina_node::foo::apply"),
        );
        // The same panic from another checkout, version and run.
        let b = crash_record(
            "index out of bounds: the len is 7 but the index is 12",
            "/home/other/src/mina-rust/crates/node/src/foo.rs:131:13",
            Some("mina_node::foo::apply"),
        );
        assert_eq!(panic_fingerprint(&a), panic_fingerprint(&b));
        // Without symbols, the line tells panics in one file apart.
        let c = crash_record("boom", "crates/node/src/foo.rs:1:1", None);
        let d = crash_record("boom", "crates/node/src/foo.rs:2:1", None);
        assert_ne!(panic_fingerprint(&c), panic_fingerprint(&d));
        let e = crash_record("boom", "crates/node/src/bar.rs:1:1", None);
        assert_ne!(panic_fingerprint(&c), panic_fingerprint(&e));
        let f = crash_record("other", "crates/node/src/foo.rs:1:1", None);
        assert_ne!(panic_fingerprint(&c), panic_fingerprint(&f));
        // Fixed values: a change here regroups every filed issue.
        assert_eq!(
            panic_fingerprint(&c),
            hash8("panic\ncrates/node/src/foo.rs\n1\nboom")
        );
        assert_eq!(error_fingerprint("P2pNetworkSchedulerSelectError").len(), 8);
        assert_eq!(
            normalize_file(
                "/home/u/.cargo/registry/src/index.crates.io-6f17d22bba15001f/tokio-1.40.0/src/runtime/park.rs"
            ),
            "tokio/src/runtime/park.rs"
        );
    }

    #[test]
    fn titles_follow_the_crash_then_the_errors() {
        let mut i = inputs();
        let crash = crash_record(
            "called `Option::unwrap()` on a `None` value",
            "crates/node/src/foo.rs:12:5",
            Some("mina_node::foo::bar"),
        );
        let fp = panic_fingerprint(&crash);
        i.crash = Some(("crash-2000-7-0.json".into(), Ok(crash.clone()), 1));
        let r = Redactor::new(false, [], None);
        let report = build(&i, &r, 100);
        assert_eq!(
            report.title,
            format!("Crash: called `Option::unwrap()` on a `None` value at foo.rs:12 [fp:{fp}]")
        );
        assert_eq!(report.fingerprint, fp);

        // A crash from an earlier run doesn't make the title.
        i.start = Some(Ok(StartRecord {
            pid: 8,
            unix_ms: 5_000,
            ..start(vec![])
        }));
        let log = "2026-10-22T20:00:00Z ERROR mina_node: kind=\"LedgerApplyError\" boom\n";
        i.logs = vec![logs::scan("mina.log", log, 1, false, 10, &r)];
        let report = build(&i, &r, 100);
        assert_eq!(
            report.title,
            format!(
                "Error: LedgerApplyError [fp:{}]",
                error_fingerprint("LedgerApplyError")
            )
        );
        assert!(report.markdown.contains("an earlier run, not the latest"));

        // RC7-FIXES: a healthy RC6 node's startup line doesn't make the
        // title, however often it is there, but a real error still does.
        let startup = "2026-10-27T19:06:57Z ERROR mina_node::transaction_pool: kind=\"Error\" \
                       summary=\"failed to fetch accounts for tx pool\" \
                       error=\"ledger jx, error: \\\"Ledger unavailable\\\"\"\n";
        // Each after a start, as at every start of a node.
        let startup = format!(
            "2026-10-27T19:06:56Z INFO mina::commands::node: kind=\"NodeStart\" \
             summary=\"starting the node\"\n{startup}"
        );
        i.logs = vec![logs::scan("mina.log", &startup.repeat(3), 1, false, 10, &r)];
        i.description = Some("node stops syncing after an hour".into());
        let report = build(&i, &r, 100);
        assert!(
            report
                .title
                .starts_with("Report: node stops syncing after an hour [fp:"),
            "{}",
            report.title
        );
        assert!(!report.title.contains("1411fe67"));
        i.logs = vec![logs::scan(
            "mina.log",
            &format!("{}{log}", startup.repeat(3)),
            1,
            false,
            10,
            &r,
        )];
        let report = build(&i, &r, 100);
        assert!(report.title.starts_with("Error: LedgerApplyError [fp:"));
        // RC7-FIXES review: the same line after the node synced is real.
        let synced = "2026-10-27T19:07:40Z INFO mina_node::logger::status: kind=\"NodeSynced\" \
                      summary=\"synced, 40s after the start\"\n";
        let after = "2026-10-27T20:00:00Z ERROR mina_node::transaction_pool: kind=\"Error\" \
                     summary=\"failed to fetch accounts for tx pool\" \
                     error=\"ledger jy, error: \\\"Ledger unavailable\\\"\"\n";
        i.logs = vec![logs::scan(
            "mina.log",
            &format!("{startup}{synced}{after}"),
            1,
            false,
            10,
            &r,
        )];
        let report = build(&i, &r, 100);
        assert!(
            report
                .title
                .starts_with("Error: Error: failed to fetch accounts for tx pool [fp:1411fe67]"),
            "{}",
            report.title
        );

        i.logs.clear();
        i.description = Some("node stops syncing after an hour".into());
        let report = build(&i, &r, 100);
        assert!(report
            .title
            .starts_with("Report: node stops syncing after an hour [fp:"));
    }

    #[test]
    fn long_logs_are_cut_to_fit() {
        let mut i = inputs();
        let line = format!("2026-10-22T20:00:00Z  WARN t: {}\n", "w".repeat(390));
        let log = line.repeat(1000);
        i.logs = vec![logs::scan(
            "mina.log",
            &log,
            log.len() as u64,
            false,
            1000,
            &Redactor::new(false, [], None),
        )];
        let report = build(&i, &Redactor::new(false, [], None), 1000);
        assert!(report.markdown.chars().count() <= MAX_BODY_CHARS);
        assert!(
            report.markdown.contains("### Frontier store"),
            "the end survives"
        );
    }
}
