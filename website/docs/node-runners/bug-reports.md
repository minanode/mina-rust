---
title: Reporting a bug
sidebar_position: 3
description:
  File a bug with one command, mina report, which collects what the developers
  need and leaves out keys, passwords, credentials and network ids; or let mina
  triage have an AI agent draft the issue from the redacted report.
---

# Reporting a bug

When the node crashes, stops syncing or misbehaves, run:

```bash
mina report
```

It collects what the developers need to fix the problem, prints it, and once you
confirm opens a pre-filled GitHub issue in your browser. You review it there and
submit it; nothing is posted before you do.

Use the same work directory as the node: `mina report` reads `~/.mina` (or
`MINA_HOME`) by default, `-d DIR` otherwise.

## What it collects

- **this binary**: version, build commit, branch, build flags and rustc;
- **the machine**: OS, kernel, CPU model and whether it has BMI2 and ADX, RAM,
  and free disk for the work directory;
- **the running node**, if it answers at `--node` (default: the port it last
  started with, else `http://127.0.0.1:3000`): network and chain id, its build,
  sync status and phase, best tip and the highest tip its peers report, peer
  counts, whether the block producer and the SNARK worker are on, and uptime;
- **the last crash**: the node writes a crash file into `<work dir>/crashes` for
  each panic (time, version, thread, message, location and backtrace), and the
  newest one goes into the report;
- **the logs**: warnings, errors and panics from the end of the newest
  `mina.log.*` in the work directory (the last 100 lines by default,
  `--log-lines N` for more), counts by kind, and the last 20 lines;
- **the frontier store**: the result of `mina node frontier-store-check` on the
  store in the work directory, if there is one (`--store-check off` skips it,
  `sample` or `full` checks more);
- **how the node was started**: its command line and the names of the
  environment variables it read, from `last-start.json`, which the node writes
  into its work directory when it starts.

## What it never includes

- private keys, p2p secret keys and the contents of key files;
- passwords, and the values of secret environment variables such as
  `MINA_PRIVKEY_PASS`, `MINA_LIBP2P_PASS` and `MINA_P2P_SEC_KEY` (only their
  names);
- anything in a URL after its host: a URL keeps only its scheme, host and port
  (`https://seeds.example.test/<redacted>`), so the user, password, path, query
  and fragment all go;
- tokens: `Authorization` headers, `Bearer` and `Basic` credentials, JSON web
  tokens, tokens with a known prefix (`sk_…`, `ghp_…`, `AKIA…`), and any long
  random-looking value in hex, base64 or base58, labelled or not. Public Mina
  identifiers stay, checked by their exact format: B62 public keys, state,
  ledger, epoch seed and transaction hashes, the devnet and mainnet chain ids,
  the node's chain id and the build commits;
- the value of anything named with `key`, `token`, `secret`, `pass`, `pwd`,
  `jwt`, `auth`, `cred`, `cookie` or `session`, in any case, as in `FOO_TOKEN=…`
  or `session_id: …`;
- the paths of key files (`--producer-key` and `--libp2p-keypair` show as
  `<key-file>`), and your user name in paths (`~` instead);
- by default, IP addresses, host names in peer addresses and peer ids. Add
  `--include-network-ids` if the developers ask for them.

These rules also apply to values hidden by JSON escapes (`https:\/\/…`,
`\u002F`) or percent-encoding, which are undone first.

The command line the node records keeps even less: a path option keeps only a
plain file name (`--genesis-config <path>/mesa-devnet-without-ledgers.json`), or
none (`<path>`) if the name could be a key or a token, and a URL option keeps
only its scheme, host and port (`--peer-list-url` shows
`https://seeds.example.test/<redacted>`). The node writes `last-start.json`, its
crash files and saved reports readable by you only (mode 600, in directories of
mode 700), and scrubs crash files with the same rules before writing them.

Read the report before you submit it all the same: it is printed in full first.

## Describing what happened

`mina report` asks what happened. A sentence on what you did, what you saw and
what you expected helps more than anything else. Pass it with
`-m "stops syncing after an hour"` to skip the question.

## Filing

- **in a browser** (the default): after you confirm, a pre-filled issue page
  opens. A long report doesn't fit in a link: the link then carries a summary,
  and the full report is saved under `<work dir>/bug-reports/`. Drag that file
  into the issue before you submit it.
- **on a server without a browser**: the link is printed instead. Open it on any
  machine. `--no-browser` prints it even when a browser is available. Without a
  terminal to confirm on (`docker exec` without `-it`, a script), add `--yes`:
  otherwise no link is printed and the report is only saved under
  `<work dir>/bug-reports/`.
- **with the GitHub CLI**: `--gh` creates the issue with `gh issue create`, if
  `gh` is installed and logged in.
- **only saving it**: `--output report.md` writes the report to a file, prints
  its full path and sends nothing.
- **without asking**: `--yes` skips the confirmation.

`--repo OWNER/NAME` files somewhere other than `minanode/mina-rust`.

The title ends with a fingerprint such as `[fp:1a2b3c4d]`, from where the node
panicked or its most frequent error. Search the issues for it before filing: if
it is there, add your report to that issue instead.

## From the dashboard

The node's dashboard has the same report: open **Report a problem** at the
bottom of its menu (`http://<node>:<port>/report-a-problem`). It is collected by
the same code as `mina report`, with the same rules for what is never included.

- **Prepare report** collects it; nothing is collected before you press it. Say
  what happened in the box above it, and tick **Keep IP addresses and peer ids
  in the report** only if the developers ask for them (they are left out by
  default, as without `--include-network-ids`).
- The report is shown in full, with **Copy**, **Download (.md)** and **Open
  GitHub issue**. The issue link is pre-filled as `mina report`'s is. A report
  too long for a link opens an issue with its summary: download the report and
  attach the file (or paste it) before you submit.
- The dashboard's report doesn't check the frontier store (`mina report` does),
  and reads the node's own `mina.log.*`; for a node that logs to standard
  output, use `mina report --log -` as below.
- When the node crashed in this run, or restarted after a panic up to a day ago,
  the overview says so, with the time and the panic's first line, and links to
  the report.

The node serves the report only to its operator, like worker control:
`POST /report` and `POST /report/crash` need an `X-Mina-Report` header and a
JSON body, and a page may ask only from this machine, from the node's own
dashboard opened at an IP address, or from an origin listed with
`--worker-control-origin`. No other site's page can read it, and a node started
with `--public-graphql` doesn't serve it. One report is collected at a time,
within 20 seconds, shared by the requests for it and reused for 10 seconds; a
request for a different report meanwhile gets HTTP 503, and one that takes too
long gets 504. A developer build that serves GraphiQL, Scalar or Stoplight
(pages that load scripts from CDNs) turns the report off while it serves them,
as worker control turns those pages off.

`mina triage`, which has an AI agent draft the issue, stays on the command line.

## When the node logs to standard output

A node started with `--disable-filesystem-logging`, as under systemd or Docker,
writes no `mina.log.*`. Give its log to `mina report` with `--log`:

```bash
journalctl --user -u mina --since "1 hour ago" | mina report --log -
docker logs mina-node 2>&1 | mina report --log -
mina report --log /var/log/mina/node.log
```

`--log` can be repeated. Only the last 8 MiB of each log is read.

## Letting an agent draft the issue

```bash
mina triage
```

If the `claude` command (Claude Code) is installed, `mina triage` collects the
same report and lets Claude draft the issue from it. Claude gets exactly this:

- the report, as printed (and redacted as above);
- the last 2000 lines of each log the report read, redacted the same way;
- a prompt asking for a draft from those files, which holds the report too.

These are written to a new temporary directory only you can read, deleted
afterwards. Before anything is sent, `mina triage` lists them with their sizes
and the directory's path, so you can read them, and asks whether to send them to
Anthropic and run Claude. Claude then runs there in its `--restricted` mode with
only the Read, Grep and Glob tools: it can't run commands, use the web, or read
files outside that directory, and your Claude Code settings, hooks and MCP
servers are not loaded. It gets none of your environment variables except what
it needs to run and sign in (`PATH`, `HOME`, the locale, certificates, and its
own `ANTHROPIC_*` or `CLAUDE_*` sign-in). Proxy variables (`HTTPS_PROXY` and the
like) are passed without a user and password, or left out when those can't be
separated (`user:pw@proxy:3128` has no scheme); `mina triage` says when it does
either. It never sees the work directory, the node or `mina` itself. A Claude
Code whose help doesn't list `--restricted` is not run.

The draft is shown, and filed as `mina report` files, once you confirm again.
`--yes` answers both questions: it sends the staged files and files the draft
without asking.

<!-- prettier-ignore-start -->

:::caution

What Claude is given goes to Anthropic under your account with them.
`mina triage` asks before it starts Claude.

:::

<!-- prettier-ignore-stop -->

- `--model NAME` is passed to Claude's `--model`.
- `--agent-timeout MINUTES` (default 15) stops an agent that takes too long.

### Codex

`codex` (Codex CLI) can't be confined the same way: its read-only sandbox limits
what it writes, not what it reads. It could read any file you can read, key
files included, and send it to OpenAI. So `mina triage` runs it only when you
ask for it and opt in, with `--agent codex --allow-unconfined-agent`. It still
starts in the temporary directory with the same files and the same cut-down
environment (with its own `OPENAI_*` and `CODEX_HOME` instead), and is told to
work only from those files, but nothing enforces that.

Without an agent it may run, `mina triage` says so and files the plain report.

All of `mina report`'s options work with `mina triage` too. See
`mina report --help` and `mina triage --help`.
