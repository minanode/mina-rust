//! `mina triage`: an AI agent CLI (`claude` or `codex`) drafts the issue from
//! the redacted report.
//!
//! What the agent can read is decided here, not asked of it. It runs in a
//! fresh private temporary directory that holds only the redacted report and
//! the redacted end of each log ([`Stage`]), with an environment cut down to
//! what the agent CLI needs to run and sign in ([`agent_env`]). Claude Code
//! runs in its `--restricted` mode with only Read, Grep and Glob: no shell, no
//! web, its file tools confined to that directory, the tester's settings,
//! hooks and MCP servers ignored. Codex has no mode that limits what it reads
//! (its read-only sandbox limits writes), so it runs only when the tester
//! opts in to an agent that can read any file they can.

use std::{
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

use std::sync::LazyLock;

use anyhow::{bail, Context};
use regex::Regex;

use super::{
    crash::{private_dir, write_private},
    logs::LogExcerpt,
    redact::Redactor,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub const ALL: [Agent; 2] = [Agent::Claude, Agent::Codex];

    pub fn program(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn vendor(self) -> &'static str {
        match self {
            Self::Claude => "Anthropic",
            Self::Codex => "OpenAI",
        }
    }

    /// Whether the agent can be kept to the staged files.
    pub fn confined(self) -> bool {
        matches!(self, Self::Claude)
    }
}

/// What a tester is told before an agent that can't be confined runs.
pub const UNCONFINED_WARNING: &str = "codex can't be kept to the redacted report: its \
read-only sandbox limits what it writes, not what it reads. It could read any file you can \
read, key files included, and send it to OpenAI.";

/// The flag that runs such an agent anyway.
pub const UNCONFINED_FLAG: &str = "--allow-unconfined-agent";

/// Whether `agent` may run: a confined one always, another only with
/// [`UNCONFINED_FLAG`].
pub fn check_allowed(agent: Agent, allow_unconfined: bool) -> anyhow::Result<()> {
    if agent.confined() || allow_unconfined {
        Ok(())
    } else {
        bail!("{UNCONFINED_WARNING} Pass {UNCONFINED_FLAG} to run it anyway, or use claude.")
    }
}

/// The redacted report and log lines, in a fresh temporary directory only
/// this user can read: the agent's working directory and, for a confined
/// agent, all it can read. Deleted when dropped.
pub struct Stage {
    dir: tempfile::TempDir,
}

pub const STAGED_PROMPT: &str = "prompt.md";
pub const STAGED_REPORT: &str = "report.md";
pub const STAGED_LOGS: &str = "logs";

impl Stage {
    /// Stages the prompt, `report` and each excerpt's lines, which must
    /// already be redacted, as the prompt, the report and
    /// [`LogExcerpt::context`] are. The prompt is there for the tester to
    /// read: it goes to the agent on its standard input.
    pub fn new(report: &str, logs: &[LogExcerpt], prompt: &str) -> anyhow::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("mina-triage-")
            .tempdir()
            .context("creating the staging directory")?;
        private_dir(dir.path())?;
        write_private(&dir.path().join(STAGED_PROMPT), prompt.as_bytes())?;
        write_private(&dir.path().join(STAGED_REPORT), report.as_bytes())?;
        let logs_dir = dir.path().join(STAGED_LOGS);
        private_dir(&logs_dir)?;
        for (index, log) in logs.iter().enumerate() {
            let text = format!(
                "# The last {} lines of {}, redacted.\n{}\n",
                log.context.len(),
                log.source,
                log.context.join("\n")
            );
            write_private(
                &logs_dir.join(format!("log-{}.txt", index + 1)),
                text.as_bytes(),
            )?;
        }
        Ok(Self { dir })
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The staged files, relative to the stage, in order.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut logs: Vec<PathBuf> = std::fs::read_dir(self.path().join(STAGED_LOGS))
            .into_iter()
            .flatten()
            .filter_map(|entry| Some(Path::new(STAGED_LOGS).join(entry.ok()?.file_name())))
            .collect();
        logs.sort();
        let mut files = vec![PathBuf::from(STAGED_PROMPT), PathBuf::from(STAGED_REPORT)];
        files.extend(logs);
        files
    }

    /// What the tester is shown before anything is sent: where the stage
    /// is, and each file's lines and bytes.
    pub fn describe(&self) -> String {
        let mut out = format!(
            "Staged in {} (read it now if you like; it is deleted afterwards):\n",
            self.path().display()
        );
        for file in self.files() {
            let text = std::fs::read_to_string(self.path().join(&file)).unwrap_or_default();
            out.push_str(&format!(
                "  {:<16} {:>6} lines {:>9} bytes\n",
                file.display(),
                text.lines().count(),
                text.len()
            ));
        }
        out
    }
}

/// Variables any agent CLI may need to run: where things are, the locale,
/// proxies and certificates.
const COMMON_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "TMPDIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "all_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
];

/// The agent's own configuration and sign-in.
fn own_env(agent: Agent) -> &'static [&'static str] {
    match agent {
        Agent::Claude => &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ],
        Agent::Codex => &["OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_HOME"],
    }
}

/// Proxy variables: passed only without a user and password.
const PROXY_ENV: &[&str] = &[
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "https_proxy",
    "http_proxy",
    "all_proxy",
];

/// A proxy setting's scheme, host and port, without a user, password or
/// anything after the host; as it is when it has none of them; `None` when a
/// user and password can't be told apart from the rest (no scheme).
pub fn proxy_without_userinfo(value: &str) -> Option<String> {
    let Some((scheme, rest)) = value.split_once("://") else {
        return (!value.contains('@')).then(|| value.to_owned());
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let valid = !scheme.is_empty()
        && !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._:[]".contains(c));
    valid.then(|| format!("{scheme}://{host}"))
}

/// The environment an agent gets, and what the tester should know about it.
#[derive(Debug, Default)]
pub struct AgentEnv {
    pub vars: Vec<(OsString, OsString)>,
    /// Proxy settings changed or left out.
    pub notes: Vec<String>,
}

/// From `vars` (the tester's environment), only what `agent` needs to run
/// and sign in, proxies without their user and password. Everything else,
/// `MINA_PRIVKEY_PASS` and the other secrets included, is left out.
pub fn agent_env(agent: Agent, vars: impl IntoIterator<Item = (OsString, OsString)>) -> AgentEnv {
    let mut env = AgentEnv::default();
    for (name, value) in vars {
        let Some(key) = name.to_str() else {
            continue;
        };
        if !(COMMON_ENV.contains(&key) || own_env(agent).contains(&key)) {
            continue;
        }
        if !PROXY_ENV.contains(&key) {
            env.vars.push((name, value));
            continue;
        }
        match value.to_str().and_then(proxy_without_userinfo) {
            Some(proxy) if proxy == value.to_str().unwrap_or("").trim_end_matches('/') => {
                env.vars.push((name, value));
            }
            Some(proxy) => {
                env.notes.push(format!(
                    "{key} is passed without its user, password or path: if the proxy needs \
                     them, {} may not reach {}.",
                    agent.program(),
                    agent.vendor()
                ));
                env.vars.push((name, proxy.into()));
            }
            None => env.notes.push(format!(
                "{key} is left out: its user and password can't be taken out of it safely."
            )),
        }
    }
    env
}

/// The instructions for the agent, with the report. Everything in it passes
/// `redactor`.
pub fn prompt(report: &str, redactor: &Redactor) -> String {
    let prompt = format!(
        "\
You are helping a tester of mina-rust, a Rust implementation of a Mina \
Protocol node, draft a GitHub issue for the developers about a problem the \
tester's node has.

Your working directory holds all you have to go on: `{STAGED_REPORT}`, the \
report `mina report` collected on the tester's machine (also below), and \
`{STAGED_LOGS}/`, the last lines of the node's logs. Both are redacted: keys, \
passwords, key file paths, credentials and network ids are replaced by \
placeholders such as `<redacted>`, `<key-file>`, `<ip>` or `<peer-id>`.

Rules:
- Work only from these files: don't read other files, run commands or use \
the network.
- Leave the placeholders as they are; don't guess what they hide.
- Don't file the issue yourself: the tester reviews and files your draft.

Answer with the draft only. The first line is `TITLE: ` and a one-line \
title. Then a blank line and the body in GitHub Markdown, with the sections \
Summary, What happened, Evidence (quote the relevant log lines, trimmed), \
Likely cause, Steps to reproduce, Suggested next steps, and What else would \
help (what the developers should ask the tester for). Say what you could not \
tell.

The report:

<report>
{report}
</report>
"
    );
    redactor.text(&prompt)
}

/// The installed agents, in [`Agent::ALL`] order.
pub fn installed() -> Vec<(Agent, PathBuf)> {
    Agent::ALL
        .into_iter()
        .filter_map(|agent| super::issue::find_in_path(agent.program()).map(|path| (agent, path)))
        .collect()
}

/// Claude Code's flags: no tools but reading, `--restricted` (no command or
/// web tools, file tools confined to the working directory, user, project
/// and local settings ignored), no MCP servers, and nothing allowed beyond
/// that without a person to ask.
pub const CLAUDE_FLAGS: &[&str] = &[
    "-p",
    "--output-format",
    "text",
    "--restricted",
    "--tools",
    "Read,Grep,Glob",
    "--permission-mode",
    "dontAsk",
    "--strict-mcp-config",
    "--no-session-persistence",
];

/// The command that runs `agent` non-interactively in `stage`, reading the
/// prompt from standard input, with only `env` for its environment. Codex
/// writes its final message to `last_message`.
pub fn command(
    agent: Agent,
    program: &Path,
    model: Option<&str>,
    stage: &Path,
    env: &[(OsString, OsString)],
    last_message: &Path,
) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(stage)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)));
    match agent {
        Agent::Claude => {
            command.args(CLAUDE_FLAGS);
        }
        Agent::Codex => {
            command
                .args([
                    "exec",
                    "--sandbox",
                    "read-only",
                    "--skip-git-repo-check",
                    "-C",
                ])
                .arg(stage)
                .arg("--output-last-message")
                .arg(last_message);
        }
    }
    if let Some(model) = model {
        command.args(["--model", model]);
    }
    if agent == Agent::Codex {
        // The prompt comes from standard input.
        command.arg("-");
    }
    command
}

/// Runs `command` with `input` on its standard input for at most `timeout`:
/// its status and standard output. A command that takes longer is killed:
/// it is the child we started, stopped by its handle.
fn run_bounded(
    mut command: Command,
    input: &str,
    timeout: Duration,
    name: &str,
) -> anyhow::Result<(ExitStatus, Vec<u8>)> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("starting {name}"))?;
    let mut stdin = child.stdin.take().expect("piped");
    let input = input.to_owned();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{name} took longer than {} s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = writer.join();
    Ok((status, reader.join().unwrap_or_default()))
}

/// `--restricted` as an entry of the options table, not just a mention.
static RESTRICTED_OPTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*--restricted(?:[\s,]|$)").expect("valid regex"));

/// Whether this `claude` lists `--restricted`, which confines it, among its
/// options. An older one is not run.
fn has_restricted_mode(program: &Path, stage: &Path, env: &[(OsString, OsString)]) -> bool {
    let mut command = Command::new(program);
    command
        .arg("--help")
        .current_dir(stage)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)));
    run_bounded(command, "", Duration::from_secs(30), "claude --help").is_ok_and(|(status, out)| {
        status.success() && RESTRICTED_OPTION.is_match(&String::from_utf8_lossy(&out))
    })
}

/// Runs the agent in `stage` with `prompt` on its standard input and `env`
/// for its environment, for at most `timeout`, and returns its answer.
pub fn run(
    agent: Agent,
    program: &Path,
    model: Option<&str>,
    stage: &Stage,
    prompt: &str,
    timeout: Duration,
    env: Vec<(OsString, OsString)>,
) -> anyhow::Result<String> {
    if agent == Agent::Claude && !has_restricted_mode(program, stage.path(), &env) {
        bail!(
            "this claude has no --restricted mode, which mina triage needs to keep it to the \
             redacted report: update Claude Code"
        );
    }
    // Outside the stage: only codex itself writes here.
    let scratch = tempfile::tempdir().context("creating a scratch directory")?;
    let last_message = scratch.path().join("last-message.md");
    let command = command(agent, program, model, stage.path(), &env, &last_message);
    let (status, stdout) = run_bounded(command, prompt, timeout, agent.program())?;
    if !status.success() {
        bail!("{} failed ({status})", agent.program());
    }
    let answer = match agent {
        Agent::Claude => String::from_utf8_lossy(&stdout).into_owned(),
        Agent::Codex => std::fs::read_to_string(&last_message)
            .unwrap_or_else(|_| String::from_utf8_lossy(&stdout).into_owned()),
    };
    if answer.trim().is_empty() {
        bail!("{} answered nothing", agent.program());
    }
    Ok(answer)
}

/// The draft's title and body: the `TITLE:` line, else `fallback_title`.
pub fn parse_draft(answer: &str, fallback_title: &str) -> (String, String) {
    let answer = answer.trim();
    let mut lines = answer.lines();
    let first = lines.next().unwrap_or("");
    let title = first
        .trim()
        .trim_start_matches('#')
        .trim()
        .strip_prefix("TITLE:")
        .map(|t| t.trim().trim_matches('`').trim().to_owned())
        .filter(|t| !t.is_empty());
    match title {
        Some(title) => (
            title,
            lines.collect::<Vec<_>>().join("\n").trim().to_owned(),
        ),
        None => (fallback_title.to_owned(), answer.to_owned()),
    }
}

/// `title`, with the report's fingerprint if the agent dropped it.
pub fn with_fingerprint(title: &str, fingerprint: &str) -> String {
    let tag = format!("[fp:{fingerprint}]");
    if title.contains(&tag) {
        title.to_owned()
    } else {
        format!("{} {tag}", title.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_are_parsed() {
        let (title, body) = parse_draft(
            "TITLE: Node stalls in catch-up\n\n## Summary\nIt stalls.\n",
            "fallback",
        );
        assert_eq!(
            (title.as_str(), body.as_str()),
            ("Node stalls in catch-up", "## Summary\nIt stalls.")
        );
        let (title, body) = parse_draft("## Summary\nNo title line.", "fallback");
        assert_eq!(
            (title.as_str(), body.as_str()),
            ("fallback", "## Summary\nNo title line.")
        );
        assert_eq!(
            with_fingerprint("Node stalls", "0a1b2c3d"),
            "Node stalls [fp:0a1b2c3d]"
        );
        assert_eq!(
            with_fingerprint("X [fp:0a1b2c3d]", "0a1b2c3d"),
            "X [fp:0a1b2c3d]"
        );
    }

    /// The prompt holds no home path, user name, non-loopback address or URL
    /// query, even from a report that wasn't redacted.
    #[test]
    fn the_prompt_is_redacted_and_names_no_local_paths() {
        let report = "Work dir /home/alice9/.mina, binary /home/alice9/bin/mina, \
                      node http://127.0.0.1:3650, peers at 34.136.105.213:8302 and \
                      [2001:db8::7]:8302, seeds https://seeds.example.org/list?auth=abc123";
        let prompt = prompt(
            report,
            &Redactor::new(false, [], Some("/home/alice9".into())),
        );
        for leak in [
            "/home/",
            "alice9",
            "34.136.105.213",
            "2001:db8::7",
            "auth=abc123",
            "abc123",
        ] {
            assert!(!prompt.contains(leak), "{leak} in the prompt:\n{prompt}");
        }
        assert!(!prompt.contains("?auth=abc"), "{prompt}");
        assert!(prompt.contains("<report>\nWork dir ~/.mina"));
        assert!(prompt.contains("Work only from these files"));
        assert!(prompt.contains("TITLE: "));
    }

    #[test]
    fn only_confined_agents_run_without_the_opt_in() {
        assert!(check_allowed(Agent::Claude, false).is_ok());
        let error = check_allowed(Agent::Codex, false).unwrap_err().to_string();
        assert!(error.contains("any file you can read"), "{error}");
        assert!(error.contains(UNCONFINED_FLAG), "{error}");
        assert!(check_allowed(Agent::Codex, true).is_ok());
    }

    #[test]
    fn the_environment_is_cut_down_to_the_agent_s_own() {
        let parent = [
            ("PATH", "/usr/bin"),
            ("HOME", "/home/tester"),
            ("MINA_PRIVKEY_PASS", "hunter22"),
            ("MINA_P2P_SEC_KEY", "5Kxyz"),
            ("AWS_SECRET_ACCESS_KEY", "aws-secret"),
            ("GITHUB_TOKEN", "ghp_x"),
            ("ANTHROPIC_API_KEY", "sk-ant-x"),
            ("OPENAI_API_KEY", "sk-openai-x"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let names = |agent| -> Vec<String> {
            agent_env(agent, parent.clone())
                .vars
                .into_iter()
                .map(|(k, _)| k.into_string().unwrap())
                .collect()
        };
        assert_eq!(names(Agent::Claude), ["PATH", "HOME", "ANTHROPIC_API_KEY"]);
        assert_eq!(names(Agent::Codex), ["PATH", "HOME", "OPENAI_API_KEY"]);
    }

    #[test]
    fn proxies_lose_their_user_and_password() {
        let parent = [
            (
                "HTTPS_PROXY",
                "http://proxy-user:proxy-pw@proxy.example:3128/x?t=1",
            ),
            ("http_proxy", "proxy-user:proxy-pw@proxy.example:3128"),
            ("ALL_PROXY", "socks5://proxy.example:1080"),
            ("NO_PROXY", "localhost,127.0.0.1"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let env = agent_env(Agent::Claude, parent);
        let vars: Vec<(String, String)> = env
            .vars
            .into_iter()
            .map(|(k, v)| (k.into_string().unwrap(), v.into_string().unwrap()))
            .collect();
        assert_eq!(
            vars,
            [
                ("HTTPS_PROXY".into(), "http://proxy.example:3128".into()),
                ("ALL_PROXY".into(), "socks5://proxy.example:1080".into()),
                ("NO_PROXY".into(), "localhost,127.0.0.1".into()),
            ]
        );
        assert_eq!(env.notes.len(), 2, "{:?}", env.notes);
        assert!(env.notes[0].starts_with("HTTPS_PROXY is passed without its user"));
        assert!(env.notes[1].starts_with("http_proxy is left out"));
        assert!(!env.notes.iter().any(|note| note.contains("proxy-pw")));
    }

    /// A mock agent: records its arguments and prompt, answers a draft.
    #[cfg(unix)]
    fn mock_agent(dir: &Path, name: &str, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// Says it has `--restricted` when asked for its help.
    #[cfg(unix)]
    const HELP: &str =
        "if [ \"$1\" = --help ]; then echo '  --restricted  Restricted mode'; exit 0; fi";

    #[cfg(unix)]
    fn path_env() -> (OsString, OsString) {
        ("PATH".into(), std::env::var_os("PATH").unwrap())
    }

    fn excerpt() -> LogExcerpt {
        LogExcerpt {
            source: "~/.mina/mina.log.2026-10-22".into(),
            context: vec!["line one".into(), "line two".into()],
            ..Default::default()
        }
    }

    /// The agent runs in the stage, sees only the staged files and gets none
    /// of the tester's variables: a mock that looks for a key file through
    /// the environment and lists what it can see finds nothing else.
    #[cfg(unix)]
    #[test]
    fn the_agent_sees_only_the_stage_and_a_cleared_environment() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("record");
        let key_file = dir.path().join("keys").join("producer-key");
        std::fs::create_dir_all(key_file.parent().unwrap()).unwrap();
        std::fs::write(&key_file, "KEY-FILE-CONTENTS").unwrap();
        let script = format!(
            "{HELP}\n\
             printf '%s\\n' \"$@\" > {r}.args; pwd > {r}.cwd; ls -AR > {r}.ls; env > {r}.env\n\
             cat \"$MINA_TEST_KEY_FILE\" > {r}.key 2>&1\n\
             cat > {r}.prompt\n\
             printf 'TITLE: Mock draft\\n\\nPeer at 10.1.2.3 misbehaves.\\n'",
            r = record.display()
        );
        let claude = mock_agent(dir.path(), "claude", &script);
        let stage = Stage::new("# the report", &[excerpt()], "the prompt").unwrap();
        let stage_path = std::fs::canonicalize(stage.path()).unwrap();
        // What the tester's shell would pass on.
        let parent = vec![
            path_env(),
            ("HOME".into(), "/home/tester".into()),
            ("MINA_PRIVKEY_PASS".into(), "hunter22".into()),
            ("MINA_TEST_KEY_FILE".into(), key_file.clone().into()),
            ("AWS_SECRET_ACCESS_KEY".into(), "aws-secret".into()),
            ("ANTHROPIC_API_KEY".into(), "sk-ant-test".into()),
            (
                "HTTPS_PROXY".into(),
                "http://proxy-user:proxy-pw@proxy.example:3128".into(),
            ),
        ];
        let answer = run(
            Agent::Claude,
            &claude,
            Some("claude-opus-5-5"),
            &stage,
            "the prompt",
            Duration::from_secs(30),
            agent_env(Agent::Claude, parent).vars,
        )
        .unwrap();
        drop(stage);
        assert!(!stage_path.exists(), "the stage is deleted");

        let read =
            |ext: &str| std::fs::read_to_string(format!("{}.{ext}", record.display())).unwrap();
        assert_eq!(parse_draft(&answer, "x").0, "Mock draft");
        assert_eq!(read("prompt"), "the prompt");
        assert_eq!(read("cwd").trim(), stage_path.to_str().unwrap());
        let listed: Vec<String> = read("ls").lines().map(str::to_owned).collect();
        for name in &listed {
            assert!(
                name.is_empty()
                    || name.ends_with(':')
                    || ["prompt.md", "report.md", "logs", "log-1.txt"].contains(&name.as_str()),
                "unexpected file {name} in the stage: {listed:?}"
            );
        }
        let env = read("env");
        for leak in [
            "hunter22",
            "MINA_PRIVKEY_PASS",
            "MINA_TEST_KEY_FILE",
            "aws-secret",
            "CARGO_",
            "proxy-pw",
        ] {
            assert!(!env.contains(leak), "{leak} reached the agent");
        }
        assert!(
            env.contains("HTTPS_PROXY=http://proxy.example:3128"),
            "{env}"
        );
        assert!(
            env.contains("ANTHROPIC_API_KEY=sk-ant-test"),
            "its own sign-in"
        );
        assert!(env.contains("HOME=/home/tester"));
        assert!(!read("key").contains("KEY-FILE-CONTENTS"));
        let args = read("args");
        let args: Vec<&str> = args.lines().collect();
        assert_eq!(&args[..CLAUDE_FLAGS.len()], CLAUDE_FLAGS);
        assert_eq!(&args[CLAUDE_FLAGS.len()..], ["--model", "claude-opus-5-5"]);
        assert!(!args.iter().any(|a| a.contains("Bash") || *a == "--add-dir"));
    }

    #[cfg(unix)]
    #[test]
    fn a_claude_without_restricted_mode_is_not_run() {
        let dir = tempfile::tempdir().unwrap();
        let ran = dir.path().join("ran");
        // Help texts that mention the flag without listing it as an option.
        for (index, help) in [
            "no such mode",
            "this build has no --restricted mode",
            "  --restricted-tools <tools>  Something else",
        ]
        .into_iter()
        .enumerate()
        {
            let script = format!(
                "if [ \"$1\" = --help ]; then echo '{help}'; exit 0; fi\ntouch {}",
                ran.display()
            );
            let claude = mock_agent(dir.path(), &format!("claude-{index}"), &script);
            let stage = Stage::new("# r", &[], "p").unwrap();
            let error = run(
                Agent::Claude,
                &claude,
                None,
                &stage,
                "p",
                Duration::from_secs(30),
                vec![path_env()],
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("--restricted"),
                "{help}: {error}"
            );
            assert!(!ran.exists(), "{help}: it ran with the prompt");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_mock_codex_answers_in_its_last_message_file() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("record");
        // Writes the draft to the file after --output-last-message.
        let script = format!(
            "out=''; prev=''\n\
             for a in \"$@\"; do [ \"$prev\" = --output-last-message ] && out=$a; prev=$a; done\n\
             printf '%s\\n' \"$@\" > {r}.args; env > {r}.env\n\
             cat > /dev/null\n\
             printf 'progress noise\\n'\n\
             printf 'TITLE: Codex draft\\n\\nBody.\\n' > \"$out\"",
            r = record.display()
        );
        let codex = mock_agent(dir.path(), "codex", &script);
        let stage = Stage::new("# r", &[excerpt()], "p").unwrap();
        let parent = vec![path_env(), ("MINA_PRIVKEY_PASS".into(), "hunter22".into())];
        let answer = run(
            Agent::Codex,
            &codex,
            None,
            &stage,
            "prompt",
            Duration::from_secs(30),
            agent_env(Agent::Codex, parent).vars,
        )
        .unwrap();
        assert_eq!(parse_draft(&answer, "x").0, "Codex draft");
        let args = std::fs::read_to_string(format!("{}.args", record.display())).unwrap();
        assert!(
            args.contains(&format!("-C\n{}\n", stage.path().display())),
            "{args}"
        );
        let env = std::fs::read_to_string(format!("{}.env", record.display())).unwrap();
        assert!(!env.contains("hunter22"));
    }

    #[cfg(unix)]
    #[test]
    fn the_stage_holds_the_prompt_the_report_and_the_logs_only() {
        use std::os::unix::fs::PermissionsExt;
        let stage = Stage::new("# the report", &[excerpt()], "the prompt").unwrap();
        let read = |p: &str| std::fs::read_to_string(stage.path().join(p)).unwrap();
        assert_eq!(read(STAGED_PROMPT), "the prompt");
        assert_eq!(read(STAGED_REPORT), "# the report");
        assert_eq!(
            stage.files(),
            ["prompt.md", "report.md", "logs/log-1.txt"].map(PathBuf::from)
        );
        // What the tester is shown before agreeing.
        let shown = stage.describe();
        assert!(
            shown.contains(&stage.path().display().to_string()),
            "{shown}"
        );
        for line in [
            "prompt.md             1 lines        10 bytes",
            "report.md             1 lines        12 bytes",
            "logs/log-1.txt        3 lines        79 bytes",
        ] {
            assert!(shown.contains(line), "{line} not in:\n{shown}");
        }
        assert_eq!(
            read("logs/log-1.txt"),
            "# The last 2 lines of ~/.mina/mina.log.2026-10-22, redacted.\nline one\nline two\n"
        );
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(stage.path()), 0o700);
        assert_eq!(mode(&stage.path().join(STAGED_REPORT)), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_or_slow_agent_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let failing = mock_agent(
            dir.path(),
            "claude",
            &format!("{HELP}\ncat > /dev/null; exit 3"),
        );
        let stage = Stage::new("# r", &[], "p").unwrap();
        let error = run(
            Agent::Claude,
            &failing,
            None,
            &stage,
            "p",
            Duration::from_secs(30),
            vec![path_env()],
        )
        .unwrap_err();
        assert!(error.to_string().contains("failed"), "{error}");
        let slow = mock_agent(dir.path(), "slow", &format!("{HELP}\nexec sleep 30"));
        let started = Instant::now();
        let error = run(
            Agent::Claude,
            &slow,
            None,
            &stage,
            "p",
            Duration::from_millis(500),
            vec![path_env()],
        )
        .unwrap_err();
        assert!(error.to_string().contains("longer than"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
