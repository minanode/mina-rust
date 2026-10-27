//! `mina report` and `mina triage`: one command to file a bug with what the
//! developers need to fix it.

pub mod issue;
#[cfg(test)]
mod tests;
pub mod triage;

// Collection, redaction and the issue link are shared with the node's own
// `POST /report` (the dashboard's Report page).
pub use mina_node_report::{bundle, crash, logs, redact, system, StoreCheckMode};

use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Context};
use reqwest::{blocking::Client, redirect::Policy, Url};
use serde_json::Value;

use bundle::{Inputs, NodeInfo, Report};
use redact::Redactor;
use triage::Agent;

/// What `mina report` and `mina triage` read, and how they file.
#[derive(Debug, Clone, clap::Args)]
pub struct ReportArgs {
    /// The node's work directory: `last-start.json`, crash files, logs and
    /// the frontier store are read from it.
    #[arg(
        long,
        short = 'd',
        default_value = "~/.mina",
        env = "MINA_HOME",
        value_name = "DIR"
    )]
    pub work_dir: String,

    /// The running node's HTTP base URL. Default: the port the node last
    /// started with (from `last-start.json`), else http://127.0.0.1:3000.
    #[arg(long, env = "MINA_NODE_URL", value_name = "URL")]
    pub node: Option<String>,

    /// A log file to scan (repeatable); `-` reads standard input, as in
    /// `journalctl --user -u mina | mina report --log -`. By default, scan
    /// `mina.log` and its numbered rotations, newest first, within 8 MiB.
    #[arg(long, value_name = "FILE")]
    pub log: Vec<PathBuf>,

    /// How many warning, error and panic lines to include (1 to 1000).
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=1000), value_name = "N")]
    pub log_lines: u32,

    /// How to check the frontier store: `root` (fast), `sample`, `full`
    /// (rehashes every ledger) or `off`.
    #[arg(long, value_enum, default_value_t = StoreCheckMode::Root)]
    pub store_check: StoreCheckMode,

    /// Keep IP addresses and peer ids in the report (left out by default).
    #[arg(long)]
    pub include_network_ids: bool,

    /// What happened, in a sentence or two. Asked for when not given and the
    /// terminal is interactive.
    #[arg(long, short = 'm', value_name = "TEXT")]
    pub message: Option<String>,

    /// Only save the report to FILE: nothing is sent.
    #[arg(long, short = 'o', value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// File without asking first.
    #[arg(long, short = 'y')]
    pub yes: bool,

    /// The GitHub repository to file the issue in.
    #[arg(long, default_value = mina_node_report::issue::DEFAULT_REPO, value_name = "OWNER/NAME")]
    pub repo: String,

    /// Create the issue with the GitHub CLI (`gh`), if installed, instead of
    /// opening a pre-filled issue page.
    #[arg(long)]
    pub gh: bool,

    /// Print the pre-filled issue link instead of opening a browser.
    #[arg(long)]
    pub no_browser: bool,

    /// Seconds to wait for each answer from the node (1 to 120).
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..=120), value_name = "SECONDS")]
    pub timeout: u64,
}

/// Collect a bug report and file it as a GitHub issue.
///
/// Gathers, without AI: this binary's version and build; the OS, kernel,
/// CPU (and whether it has BMI2 and ADX), memory and free disk; what the
/// running node says (network, sync, best and highest peer tip, peers,
/// producer and worker); the newest crash file the node wrote; warnings,
/// errors and panics from the end of its log; a check of its frontier store;
/// and the command line it was started with.
///
/// Keys, passwords, key files and their paths, and secret environment
/// variables are never included; IP addresses and peer ids only with
/// `--include-network-ids`.
///
/// The report is printed first. Once you confirm (or with `--yes`), a
/// pre-filled issue opens in your browser, where you review and submit it;
/// with `--gh`, `gh issue create` files it. `--output FILE` only saves it.
///
/// Examples:
///
///   mina report
///   mina report -d ~/.mina-devnet -m "stops syncing after an hour"
///   journalctl --user -u mina | mina report --log -
///   mina report --output report.md
#[derive(Debug, clap::Args)]
#[command(verbatim_doc_comment)]
pub struct ReportCommand {
    #[command(flatten)]
    pub args: ReportArgs,
}

/// Let an AI agent draft the bug report issue from the redacted report.
///
/// Collects the report `mina report` would, then runs the `claude` CLI, if
/// installed, non-interactively in a new temporary directory that holds only
/// the redacted report and the redacted last 2000 lines of each log. Claude
/// runs in its --restricted mode with only Read, Grep and Glob: it can't run
/// commands, use the web, or read anything outside that directory, and gets
/// only the environment variables it needs to sign in. Its draft is shown,
/// and filed as `mina report` files, only once you confirm.
///
/// What it is given goes to Anthropic: you are asked before it runs. codex
/// can't be confined that way (it could read any file you can read), so it
/// runs only with --agent codex --allow-unconfined-agent. Without an agent,
/// `mina triage` says so and files the plain report.
///
/// Examples:
///
///   mina triage
///   mina triage --model claude-opus-5-5
#[derive(Debug, clap::Args)]
#[command(verbatim_doc_comment)]
pub struct TriageCommand {
    #[command(flatten)]
    pub args: ReportArgs,

    /// The agent to run; asked for when both are installed.
    #[arg(long, value_enum)]
    pub agent: Option<Agent>,

    /// The model, passed to the agent's `--model`.
    #[arg(long, value_name = "MODEL")]
    pub model: Option<String>,

    /// Minutes the agent may take (1 to 120).
    #[arg(long, default_value_t = 15, value_parser = clap::value_parser!(u64).range(1..=120), value_name = "MINUTES")]
    pub agent_timeout: u64,

    /// Run an agent that can't be kept to the redacted report: codex, whose
    /// read-only sandbox limits writes, not reads. It can read any file you
    /// can read, key files included, and send it to OpenAI.
    #[arg(long)]
    pub allow_unconfined_agent: bool,
}

impl ReportCommand {
    pub fn run(self) -> anyhow::Result<()> {
        let collected = Collected::new(&self.args)?;
        collected.file_report(&self.args)
    }
}

impl TriageCommand {
    pub fn run(self) -> anyhow::Result<()> {
        let args = &self.args;
        if let Some(agent) = self.agent {
            triage::check_allowed(agent, self.allow_unconfined_agent)?;
        }
        let collected = Collected::new(args)?;
        let installed = triage::installed();
        let eligible: Vec<(Agent, PathBuf)> = installed
            .iter()
            .filter(|(agent, _)| agent.confined() || self.allow_unconfined_agent)
            .cloned()
            .collect();
        let chosen = match self.agent {
            Some(agent) => installed
                .iter()
                .find(|(a, _)| *a == agent)
                .cloned()
                .or_else(|| {
                    eprintln!(
                        "The {} CLI is not installed (not found in PATH).",
                        agent.program()
                    );
                    None
                }),
            None => match eligible.as_slice() {
                [] => {
                    if installed.is_empty() {
                        eprintln!(
                            "Neither the claude nor the codex CLI is installed (not found in PATH)."
                        );
                    } else {
                        eprintln!(
                            "Only codex is installed. {} Pass --agent codex {} to run it anyway.",
                            triage::UNCONFINED_WARNING,
                            triage::UNCONFINED_FLAG
                        );
                    }
                    None
                }
                [only] => Some(only.clone()),
                [..] if issue::interactive() && !args.yes => {
                    let names: Vec<&str> = eligible.iter().map(|(a, _)| a.program()).collect();
                    let index = dialoguer::Select::new()
                        .with_prompt("Which agent should draft the issue?")
                        .items(&names)
                        .default(0)
                        .interact()?;
                    Some(eligible[index].clone())
                }
                [first, ..] => {
                    eprintln!(
                        "Both claude and codex are installed: using {} (pick one with --agent).",
                        first.0.program()
                    );
                    Some(first.clone())
                }
            },
        };
        let Some((agent, program)) = chosen else {
            eprintln!("Filing the report `mina report` collects instead.\n");
            return collected.file_report(args);
        };

        if args.output.is_none() || !args.yes {
            println!(
                "# {}\n\n{}",
                collected.report.title, collected.report.markdown
            );
        }
        // Everything the agent gets is staged before the tester decides.
        let prompt = triage::prompt(&collected.report.markdown, &collected.redactor);
        let stage = triage::Stage::new(&collected.report.markdown, &collected.logs, &prompt)?;
        let env = triage::agent_env(agent, std::env::vars_os());
        eprintln!(
            "\n`mina triage` gives {} the report above and the last lines of the node's logs, \
             redacted like it, and runs it in the directory they are staged in. All of it goes \
             to {}.\n{}",
            agent.program(),
            agent.vendor(),
            stage.describe()
        );
        if agent.confined() {
            eprintln!(
                "claude runs in its --restricted mode with only Read, Grep and Glob: it can't run \
                 commands, use the web, or read files outside that directory, and gets none of \
                 your environment variables but what it needs to run and sign in."
            );
        } else {
            eprintln!(
                "{} You passed {}.",
                triage::UNCONFINED_WARNING,
                triage::UNCONFINED_FLAG
            );
        }
        for note in &env.notes {
            eprintln!("{note}");
        }
        if !args.yes {
            if !issue::interactive() {
                bail!(
                    "no terminal to confirm on: run again with --yes to start {}",
                    agent.program()
                );
            }
            let go = dialoguer::Confirm::new()
                .with_prompt(format!(
                    "Send these files to {} and run {}?",
                    agent.vendor(),
                    agent.program()
                ))
                .default(false)
                .interact()?;
            if !go {
                eprintln!("Nothing was sent.");
                return Ok(());
            }
        }
        eprintln!(
            "Running {} (this can take a few minutes)...",
            agent.program()
        );
        let answer = triage::run(
            agent,
            &program,
            self.model.as_deref(),
            &stage,
            &prompt,
            Duration::from_secs(self.agent_timeout * 60),
            env.vars,
        )?;
        drop(stage);
        let answer = collected.redactor.text(&answer);
        let (title, draft) = triage::parse_draft(&answer, &collected.report.title);
        let title = triage::with_fingerprint(&title, &collected.report.fingerprint);
        let body = format!(
            "{draft}\n\n---\n_Drafted by `mina triage` with {}._\n\n<details><summary>The `mina \
             report` it worked from</summary>\n\n{}\n\n</details>\n",
            agent.program(),
            collected.report.markdown
        );
        let summary = |place: &str| {
            let draft: String = draft.chars().take(3000).collect();
            (issue::attach_note(place), draft)
        };
        collected.filing(args)?.file(&title, &body, summary)
    }
}

/// A collected and built report.
struct Collected {
    work_dir: PathBuf,
    redactor: Redactor,
    report: Report,
    /// Redacted when they were read.
    logs: Vec<logs::LogExcerpt>,
}

impl Collected {
    fn new(args: &ReportArgs) -> anyhow::Result<Self> {
        issue::check_repo(&args.repo)?;
        let work_dir = PathBuf::from(
            shellexpand::full(&args.work_dir)
                .context("--work-dir")?
                .into_owned(),
        );
        let mut redactor = Redactor::from_env(args.include_network_ids);
        let description = match &args.message {
            Some(message) => Some(message.clone()),
            None if issue::interactive()
                && !args.yes
                && !args.log.iter().any(|l| l == Path::new("-")) =>
            {
                let text: String = dialoguer::Input::new()
                    .with_prompt("What happened? (a sentence or two; Enter to skip)")
                    .allow_empty(true)
                    .interact_text()?;
                Some(text)
            }
            None => None,
        };
        let inputs = collect(args, &work_dir, description, &redactor)?;
        let report = mina_node_report::build(&inputs, &mut redactor, args.log_lines as usize);
        Ok(Self {
            work_dir,
            redactor,
            report,
            logs: inputs.logs,
        })
    }

    fn work_dir_or_current(&self) -> PathBuf {
        if self.work_dir.is_dir() {
            self.work_dir.clone()
        } else {
            PathBuf::from(".")
        }
    }

    fn filing<'a>(&'a self, args: &'a ReportArgs) -> anyhow::Result<issue::Filing<'a>> {
        Ok(issue::Filing {
            repo: &args.repo,
            gh: args.gh,
            yes: args.yes,
            no_browser: args.no_browser,
            output: args.output.as_deref(),
            save_dir: self.work_dir_or_current(),
            redactor: &self.redactor,
        })
    }

    fn file_report(&self, args: &ReportArgs) -> anyhow::Result<()> {
        let summary = |place: &str| (issue::attach_note(place), self.report.summary.clone());
        self.filing(args)?
            .file(&self.report.title, &self.report.markdown, summary)
    }
}

fn collect(
    args: &ReportArgs,
    work_dir: &Path,
    description: Option<String>,
    redactor: &Redactor,
) -> anyhow::Result<Inputs> {
    let start = crash::read_last_start(work_dir);
    let start_ok = start.as_ref().and_then(|s| s.as_ref().ok());
    let node_url = match &args.node {
        Some(node) => node.trim_end_matches('/').to_owned(),
        None => format!(
            "http://127.0.0.1:{}",
            start_ok.and_then(|s| s.http_port).unwrap_or(3000)
        ),
    };
    eprintln!(
        "Collecting a bug report: work dir {}, node {node_url}...",
        redactor.display_path(work_dir)
    );
    let node = fetch_node(&node_url, Duration::from_secs(args.timeout))
        // `connection refused`, not every layer that passed it on.
        .map_err(|e| e.root_cause().to_string());
    Ok(mina_node_report::collect(
        mina_node_report::Sources {
            work_dir,
            node_url,
            node,
            logs: &args.log,
            log_lines: args.log_lines as usize,
            store_check: args.store_check,
            store_off_reason: "turned off with `--store-check off`",
            description,
            progress: &|message| eprintln!("{message}"),
        },
        redactor,
    ))
}

/// Answers from the node are small; anything bigger is not what we asked.
const MAX_ANSWER_BYTES: u64 = 4 << 20;

/// `--node` as the base its routes join onto, normalised as `node status`
/// does it.
fn node_base(node: &str) -> anyhow::Result<Url> {
    let node: Url = node.parse().context("--node is not a URL")?;
    Ok(crate::commands::node::status::base_url(&node))
}

fn fetch_node(node: &str, timeout: Duration) -> anyhow::Result<NodeInfo> {
    let base = node_base(node)?;
    if !matches!(base.scheme(), "http" | "https") {
        bail!("--node must use http or https");
    }
    if !base.username().is_empty() || base.password().is_some() {
        bail!("--node must not contain credentials");
    }
    let client = Client::builder()
        .timeout(timeout)
        .redirect(Policy::none())
        .build()?;
    let get = |path: &str| -> anyhow::Result<Value> {
        let response = client
            .get(base.join(path)?)
            .send()
            .map_err(|e| e.without_url())?;
        if !response.status().is_success() {
            bail!("GET /{path}: HTTP {}", response.status());
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_ANSWER_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_ANSWER_BYTES {
            bail!("GET /{path}: the answer exceeds 4 MiB");
        }
        Ok(serde_json::from_slice(&bytes)?)
    };
    let status = get("status")?;
    let build = get("build_env").ok();
    let snarker = get("snarker/config").ok();
    Ok(NodeInfo::from_json(
        &status,
        build.as_ref(),
        snarker.as_ref(),
        crash::now_ms(),
    ))
}

/// What a node writes for `mina report` when it starts: the start record,
/// and from then on a crash file for each panic.
pub fn on_node_start(start: StartInfo<'_>) {
    let argv: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let cli = <super::MinaCli as clap::CommandFactory>::command();
    let env_set = crash::env_names_set(&cli);
    let build = mina_node::BuildEnv::get();
    let record = crash::StartRecord {
        started_at: crash::now_rfc3339(),
        unix_ms: crash::now_ms(),
        pid: std::process::id(),
        version: build.version,
        commit: build.git.commit_hash,
        network: mina_core::NetworkConfig::global().name.to_owned(),
        http_port: Some(start.http_port),
        log_dir: start.log_dir.map(str::to_owned),
        frontier_store_dir: start.frontier_store_dir.map(str::to_owned),
        snark_worker: start.snark_worker,
        args: redact::redact_argv(&argv, cli),
        env_set,
    };
    if let Err(error) = crash::enable(start.work_dir, &record, &start.secrets) {
        mina_node::core::warn!(
            mina_node::core::log::system_time();
            kind = "BugReport",
            summary = "could not write last-start.json: `mina report` will know less",
            error = error.to_string(),
        );
    }
}

/// What [`on_node_start`] records.
pub struct StartInfo<'a> {
    pub work_dir: &'a Path,
    pub http_port: u16,
    pub log_dir: Option<&'a str>,
    pub frontier_store_dir: Option<&'a str>,
    pub snark_worker: Option<crash::StartWorker>,
    /// Passwords, keys and key file paths, never written to a crash file.
    pub secrets: Vec<&'a str>,
}

#[cfg(test)]
mod node_url_tests {
    use super::{collect, node_base, ReportArgs};
    use clap::Parser;
    use mina_node_report::redact::Redactor;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: ReportArgs,
    }

    /// CLI-REF fix round 1: `report` and `triage` join the node's routes onto
    /// `--node` as `node status` does: one `/` after the path, no query.
    #[test]
    fn node_urls_are_normalised_as_node_status_does() {
        for (node, status) in [
            ("http://127.0.0.1:3000", "http://127.0.0.1:3000/status"),
            ("http://127.0.0.1:3000/", "http://127.0.0.1:3000/status"),
            ("http://x/prefix", "http://x/prefix/status"),
            ("http://x/prefix/", "http://x/prefix/status"),
            ("http://x/prefix//", "http://x/prefix/status"),
            ("http://x/prefix/?a=b#c", "http://x/prefix/status"),
        ] {
            let base = node_base(node).unwrap();
            assert_eq!(base.join("status").unwrap().as_str(), status, "{node}");
        }
    }

    /// The same through the collection path: a node behind a path prefix,
    /// given with a trailing slash, gets single-slash requests.
    #[test]
    fn a_node_behind_a_prefix_gets_single_slash_requests() {
        use std::{io::Write, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut paths = Vec::new();
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 2048];
                let n = std::io::Read::read(&mut stream, &mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..n]).into_owned();
                paths.push(request.split(' ').nth(1).unwrap_or_default().to_owned());
                let body = if request.starts_with("GET /prefix/status ") {
                    r#"{"network":"devnet","transition_frontier":{"sync":{"status":"Synced","phase":"Synced"},"best_tip":{"height":7}},"peers":[],"block_producer":null}"#
                } else {
                    "null"
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
            paths
        });
        let work = tempfile::tempdir().unwrap();
        let args = Cli::try_parse_from([
            "report",
            "--node",
            &format!("http://127.0.0.1:{port}/prefix/"),
            "--store-check",
            "off",
        ])
        .unwrap()
        .args;
        let redactor = Redactor::new(false, [], None);
        let inputs = collect(&args, work.path(), None, &redactor).unwrap();
        let paths = server.join().unwrap();
        assert_eq!(
            paths,
            [
                "/prefix/status",
                "/prefix/build_env",
                "/prefix/snarker/config"
            ]
        );
        let node = inputs.node.as_ref().unwrap();
        assert_eq!(node.sync_status.as_deref(), Some("Synced"));
    }
}
