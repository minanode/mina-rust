//! Bug reports for a Mina node, shared by `mina report` (the CLI) and the
//! node's own `POST /report` (the dashboard's Report page): one way to
//! collect, one set of redaction rules, one pre-filled issue link.
//!
//! - [`collect`] gathers a report's [`bundle::Inputs`] from the node's work
//!   directory (its start record, crash files and logs), this machine, the
//!   frontier store and what the node says about itself;
//! - [`bundle::build`] turns them into a redacted [`bundle::Report`];
//! - [`issue::link`] makes the pre-filled GitHub issue link, within GitHub's
//!   URL limit.
//!
//! Only the CLI asks questions, runs `gh` or a browser, and drafts with an AI
//! agent (`mina triage`).

pub mod bundle;
pub mod crash;
pub mod issue;
pub mod logs;
pub mod redact;
pub mod system;

use std::path::{Path, PathBuf};

use bundle::{Inputs, NodeInfo, Report, StoreCheck};
use redact::Redactor;

/// How to check the frontier store: `root` (fast), `sample`, `full`
/// (rehashes every ledger) or `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum StoreCheckMode {
    Root,
    Sample,
    Full,
    Off,
}

/// What [`collect`] reads, besides the node's work directory.
pub struct Sources<'a> {
    /// The node's work directory: `last-start.json`, crash files, logs and
    /// the frontier store are read from it.
    pub work_dir: &'a Path,
    /// The node's address, as the report shows it.
    pub node_url: String,
    /// What the node says about itself ([`NodeInfo::from_json`] of its
    /// `/status`, `/build_env` and `/snarker/config`), or why it couldn't.
    pub node: Result<NodeInfo, String>,
    /// Log files to scan (`-` is standard input); none: the node's own
    /// `mina.log` and its rotations, newest first, within
    /// [`logs::MAX_SCAN_BYTES`].
    pub logs: &'a [PathBuf],
    /// How many warning, error and panic lines to include.
    pub log_lines: usize,
    pub store_check: StoreCheckMode,
    /// Why, when `store_check` is [`StoreCheckMode::Off`], for the report.
    pub store_off_reason: &'a str,
    /// What happened, in the reporter's words.
    pub description: Option<String>,
    /// Told what takes a while (the store check), for a terminal.
    pub progress: &'a dyn Fn(&str),
}

/// Gathers a report's inputs. Reads files and runs the store check: call it
/// off any thread that must stay responsive.
pub fn collect(sources: Sources<'_>, redactor: &Redactor) -> Inputs {
    let work_dir = sources.work_dir;
    let start = crash::read_last_start(work_dir);
    let start_ok = start.as_ref().and_then(|s| s.as_ref().ok());

    let mut log_notes = Vec::new();
    let mut excerpts = Vec::new();
    let max_lines = sources.log_lines;
    if sources.logs.is_empty() {
        let dir = match start_ok {
            Some(start) => start.log_dir.as_ref().map(PathBuf::from),
            None => Some(work_dir.to_owned()),
        };
        let paths = dir.as_deref().map(logs::find_logs).unwrap_or_default();
        if paths.is_empty() {
            log_notes.push(
                "No log file found: the node writes `mina.log.*` in its work dir (or \
                 `--log-path`) unless started with `--disable-filesystem-logging`. Pass its log \
                 with `--log FILE`, or `--log -` to read it from standard input."
                    .to_owned(),
            );
        }
        let mut remaining = logs::MAX_SCAN_BYTES;
        for path in paths {
            if remaining == 0 {
                break;
            }
            match logs::read_tail_with_budget(&path, remaining) {
                Ok((text, len, truncated)) => {
                    remaining -= len;
                    excerpts.push(logs::scan(
                        &path.display().to_string(),
                        &text,
                        len,
                        truncated,
                        max_lines,
                        redactor,
                    ));
                }
                Err(error) => {
                    log_notes.push(format!("Could not read `{}`: {error}.", path.display()))
                }
            }
        }
    }
    for path in sources.logs {
        let (source, read) = if path == Path::new("-") {
            ("standard input".to_owned(), logs::read_stdin_tail())
        } else {
            (path.display().to_string(), logs::read_tail(path))
        };
        match read {
            Ok((text, len, truncated)) => excerpts.push(logs::scan(
                &source, &text, len, truncated, max_lines, redactor,
            )),
            Err(error) => log_notes.push(format!("Could not read `{source}`: {error}.")),
        }
    }

    let chain_id = sources.node.as_ref().ok().and_then(|n| n.chain_id.clone());
    let (store, store_skipped) = match sources.store_check {
        StoreCheckMode::Off => (None, Some(sources.store_off_reason.to_owned())),
        mode => {
            let base = start_ok
                .and_then(|s| s.frontier_store_dir.as_ref())
                .map(PathBuf::from)
                .unwrap_or_else(|| work_dir.to_owned());
            let store = find_store(&base, chain_id.as_deref()).map(|dir| {
                (sources.progress)(&format!(
                    "Checking the frontier store {} (--store-check off skips this)...",
                    redactor.display_path(&dir)
                ));
                check_store(&dir, mode)
            });
            (store, None)
        }
    };

    let start_process = start_ok.and_then(|s| system::process(s.pid));
    let crash = crash::newest_crash(work_dir).map(|(path, record, count)| {
        let name = path
            .file_name()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        (name, record, count)
    });
    Inputs {
        generated_at: crash::now_rfc3339(),
        description: sources.description,
        build: mina_node::BuildEnv::get(),
        system: system::collect(work_dir),
        work_dir: work_dir.display().to_string(),
        node_url: sources.node_url,
        node: sources.node,
        start,
        start_process,
        crash,
        logs: excerpts,
        log_notes,
        store,
        store_skipped,
    }
}

/// The redacted report of `inputs`. The node's chain id and the builds'
/// commits look opaque, but the report needs them: `redactor` lets them
/// through from now on.
pub fn build(inputs: &Inputs, redactor: &mut Redactor, log_lines: usize) -> Report {
    let node = inputs.node.as_ref().ok();
    let start = inputs.start.as_ref().and_then(|start| start.as_ref().ok());
    let crash = inputs
        .crash
        .as_ref()
        .and_then(|(_, crash, _)| crash.as_ref().ok());
    for value in [
        node.and_then(|node| node.chain_id.as_deref()),
        node.and_then(|node| node.build_commit.as_deref()),
        start.map(|start| start.commit.as_str()),
        crash.map(|crash| crash.commit.as_str()),
    ]
    .into_iter()
    .flatten()
    {
        redactor.allow(value);
    }
    bundle::build(inputs, redactor, log_lines)
}

/// `<base>/<chain id>/frontier`: the node's chain's, else the newest.
pub fn find_store(base: &Path, chain_id: Option<&str>) -> Option<PathBuf> {
    let mut stores: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(base)
        .ok()?
        .filter_map(|entry| {
            let dir = entry.ok()?.path().join("frontier");
            let modified = dir
                .metadata()
                .ok()
                .filter(|m| m.is_dir())?
                .modified()
                .ok()?;
            Some((modified, dir))
        })
        .collect();
    if let Some(chain_id) = chain_id {
        let ours = base.join(chain_id).join("frontier");
        if let Some(found) = stores.iter().find(|(_, dir)| *dir == ours) {
            return Some(found.1.clone());
        }
    }
    stores.sort();
    stores.pop().map(|(_, dir)| dir)
}

const MAX_STORE_OUTPUT_LINES: usize = 200;

pub fn check_store(dir: &Path, mode: StoreCheckMode) -> StoreCheck {
    use mina_node::persist::{
        check::{check_store, CheckOptions},
        PersistError, VerifyMode,
    };
    let verify = match mode {
        StoreCheckMode::Full => VerifyMode::Full,
        StoreCheckMode::Sample => VerifyMode::default(),
        _ => VerifyMode::RootOnly,
    };
    let run = |lock: bool| {
        let mut out = Vec::new();
        let options = CheckOptions {
            verify,
            rebuild_staged: false,
            lock,
        };
        let result = check_store(dir, &options, &mut out);
        (result, out)
    };
    let mut note = None;
    let (mut result, mut out) = run(true);
    let locked = result.as_ref().err().is_some_and(|error| {
        error
            .chain()
            .any(|e| matches!(e.downcast_ref::<PersistError>(), Some(PersistError::Locked)))
    });
    if locked {
        note = Some(
            "The running node holds the store's lock: it was checked without the lock, so a \
             failure may be a race with the node's writes."
                .to_owned(),
        );
        (result, out) = run(false);
    }
    let mut output = String::from_utf8_lossy(&out).into_owned();
    let passed = match result {
        Ok(summary) => {
            for warning in &summary.warnings {
                output.push_str(&format!("warning: {warning}\n"));
            }
            for problem in &summary.problems {
                output.push_str(&format!("FAILED: {problem}\n"));
            }
            Some(summary.passed())
        }
        Err(error) => {
            note = Some(format!("The check could not run: {error:#}."));
            None
        }
    };
    let lines: Vec<&str> = output.lines().collect();
    let output = if lines.len() > MAX_STORE_OUTPUT_LINES {
        let mut cut = lines[..MAX_STORE_OUTPUT_LINES].join("\n");
        cut.push_str(&format!(
            "\n[... {} more lines]",
            lines.len() - MAX_STORE_OUTPUT_LINES
        ));
        cut
    } else {
        output
    };
    StoreCheck {
        dir: dir.display().to_string(),
        passed,
        output,
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_progress_abbreviates_home_and_the_body_redacts_it() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("report-test-user");
        let work = home.join(".mina");
        std::fs::create_dir_all(work.join("chain/frontier")).unwrap();
        let mut redactor = Redactor::new(false, [], Some(home.display().to_string()));
        let messages = std::cell::RefCell::new(Vec::new());
        let inputs = collect(
            Sources {
                work_dir: &work,
                node_url: "http://127.0.0.1:3900".into(),
                node: Err("not running".into()),
                logs: &[],
                log_lines: 100,
                store_check: StoreCheckMode::Root,
                store_off_reason: "",
                description: Some(format!("stopped in {}", work.display())),
                progress: &|message| messages.borrow_mut().push(message.to_owned()),
            },
            &redactor,
        );
        assert_eq!(messages.into_inner(), ["Checking the frontier store ~/.mina/chain/frontier (--store-check off skips this)..."]);
        assert_eq!(
            inputs.work_dir,
            work.display().to_string(),
            "collection still uses the original path"
        );
        let report = build(&inputs, &mut redactor, 100);
        for text in [&report.markdown, &report.summary, &report.title] {
            assert!(!text.contains(&home.display().to_string()));
            assert!(!text.contains("report-test-user"));
        }
    }

    #[test]
    fn stores_are_found_by_chain() {
        let dir = tempfile::tempdir().unwrap();
        for chain in ["aaaa", "bbbb"] {
            std::fs::create_dir_all(dir.path().join(chain).join("frontier")).unwrap();
        }
        std::fs::create_dir_all(dir.path().join("genesis")).unwrap();
        assert_eq!(
            find_store(dir.path(), Some("aaaa")),
            Some(dir.path().join("aaaa/frontier"))
        );
        assert!(find_store(dir.path(), None).is_some());
        assert_eq!(find_store(&dir.path().join("none"), None), None);
    }

    #[test]
    fn an_empty_store_check_passes() {
        let dir = tempfile::tempdir().unwrap();
        let check = check_store(dir.path(), StoreCheckMode::Root);
        assert_eq!(check.passed, Some(true), "{check:?}");
        assert!(
            check.output.contains("no valid manifest"),
            "{}",
            check.output
        );
    }

    /// The node's own path: its answers given, its logs read from the work
    /// dir, the store left alone, and a network id kept out.
    #[test]
    fn a_report_is_collected_from_a_work_dir() {
        let work = tempfile::tempdir().unwrap();
        std::fs::write(
            work.path().join("mina.log.1"),
            "2026-10-22T20:00:00Z ERROR mina_node: kind=\"SomethingFailed\" at 10.0.0.9\n",
        )
        .unwrap();
        let status = serde_json::json!({
            "network": "devnet",
            "transition_frontier": {"sync": {"status": "Synced", "phase": "Synced"}, "best_tip": {"height": 7}},
            "peers": [],
            "block_producer": null,
        });
        let node = NodeInfo::from_json(&status, None, None, crash::now_ms());
        let mut redactor = Redactor::new(false, [], None);
        let checked = std::cell::Cell::new(false);
        let inputs = collect(
            Sources {
                work_dir: work.path(),
                node_url: "http://127.0.0.1:3000".into(),
                node: Ok(node),
                logs: &[],
                log_lines: 100,
                store_check: StoreCheckMode::Off,
                store_off_reason: "skipped for this test",
                description: Some("it failed".into()),
                progress: &|_| checked.set(true),
            },
            &redactor,
        );
        assert!(!checked.get(), "the store check is off");
        assert_eq!(inputs.logs.len(), 1);
        let report = build(&inputs, &mut redactor, 100);
        assert!(
            report.title.starts_with("Error: SomethingFailed [fp:"),
            "{}",
            report.title
        );
        assert!(report
            .markdown
            .contains("Not checked: skipped for this test."));
        assert!(!report.markdown.contains("10.0.0.9"));
    }
}
