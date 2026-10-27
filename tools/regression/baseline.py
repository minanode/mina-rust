#!/usr/bin/env python3
"""Print a baseline table from fresh measurements.

Measures what it can on this machine and reads what it is given:

- the full replay: runs mina-block-replay over every block in the replay
  directory (about 17 min), or only the fast subset, or reads the output of an
  earlier run;
- the staged ledger rebuild: runs the profile_reconstruct example on a dump;
- the web node bootstrap: reads the run.log and console.log a run.mjs wrote;
- the native bootstrap: reads a native node's log;
- the soak: reads a running node's /status.

Prints a markdown table with optional locally recorded reference values next to
the new ones, then how each new number was got. The public tree omits that
reference table; measurements still run without it. Only the standard library
is used.
See tools/regression/README.md.

    tools/regression/baseline.py --work "$ROOT/.work" --bin-dir "$ROOT/target/release" \\
        --browser-run /tmp/my-run
"""

import argparse
import datetime
import glob
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.request

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

# The fast replay subset ends here. Keep in step with `TO` in
# tools/block-replay/tests/replay_subset.rs.
SUBSET_TO = 551260
GENESIS_HEIGHT = 548147

# Rows of the optional reference table, by key and label prefix.
ROWS = [
    ("browser", "Web node bootstrap through a local relay, page to synced"),
    ("browser_phases", "— staking ledger, next epoch ledger, root ledgers, blocks"),
    ("browser_memory", "Web node WASM memory when synced"),
    ("native", "Native bootstrap from OCaml peers, genesis without ledgers"),
    ("reconstruct", "Native staged ledger rebuild (`profile_reconstruct`)"),
    ("replay", "Full replay of"),
    ("soak", "Native soak"),
]


def recorded_baseline():
    """Optional reference values by row key; missing tables supply no comparisons."""
    path = os.path.join(REPO, "docs", "backlog.md")
    try:
        text = open(path, encoding="utf-8").read()
    except OSError:
        return {}
    section = text.split("## Baseline", 1)[-1].split("\n## ", 1)[0]
    values = {}
    for line in section.splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) != 2 or set(cells[1]) <= set("-: "):
            continue
        for key, label in ROWS:
            if cells[0].startswith(label):
                values[key] = cells[1]
    return values


def minutes(seconds):
    return f"{seconds / 60:.1f} min"


# A Rust `Duration` printed with `{:?}`, e.g. `1000s`, `86ms` or `19.03s`.
DURATION = r"([\d.]+)(ns|µs|us|ms|s)\b"


def seconds_of(match, group=1):
    scale = {"ns": 1e-9, "µs": 1e-6, "us": 1e-6, "ms": 1e-3, "s": 1}[match.group(group + 1)]
    return float(match.group(group)) * scale


# Replay


def replay(binary, replay_dir, genesis, to, log_dir):
    """Runs mina-block-replay; returns (value, how) for the table."""
    blocks = os.path.join(replay_dir, "blocks")
    if not os.path.isdir(blocks):
        blocks = replay_dir
    command = [binary, "--network", "mainnet", "--genesis", genesis]
    command += ["--chain", os.path.join(replay_dir, "chain.txt"), "--blocks", blocks]
    if to is not None:
        command += ["--to", str(to)]
    log = os.path.join(log_dir, "replay.log")
    what = "every block" if to is None else f"blocks to {to}"
    print(f"replaying {what}, output in {log}", file=sys.stderr)
    started = time.monotonic()
    with open(log, "w") as out:
        status = subprocess.run(command, stdout=out, stderr=subprocess.STDOUT).returncode
    wall = time.monotonic() - started
    value, _ = read_replay_log(log)
    if status != 0:
        value = f"FAILED (exit {status}): {last_line(log)}"
    return value, f"`{' '.join(command)}`: {wall:.0f} s wall, genesis load included"


def read_replay_log(path):
    summary = None
    for line in open(path, errors="replace"):
        if line.startswith("applied "):
            summary = line.strip()
    if summary is None:
        return f"no summary line: {last_line(path)}", f"read from {path}"
    match = re.match(r"applied (\d+) blocks to (\S+) in " + DURATION + ", every staged ledger hash matching", summary)
    if not match:
        return summary, f"read from {path}"
    count, tip, seconds = int(match.group(1)), match.group(2), seconds_of(match, 3)
    took = minutes(seconds) if seconds >= 300 else f"{seconds:.0f} s"
    return f"{took} ({count:,} blocks to {tip[:12]}…, every hash matching)", f"read from {path}"


def last_line(path):
    lines = [l.strip() for l in open(path, errors="replace") if l.strip()]
    return lines[-1][:200] if lines else "(no output)"


# Staged ledger rebuild


def reconstruct(binary, dump, log_dir):
    log = os.path.join(log_dir, "reconstruct.log")
    print(f"rebuilding the staged ledger in {dump}, output in {log}", file=sys.stderr)
    with open(log, "w") as out:
        status = subprocess.run([binary, dump], stdout=out, stderr=subprocess.STDOUT).returncode
    text = open(log, errors="replace").read()
    match = re.search(r"reconstructed in " + DURATION, text)
    how = f"`{binary} {dump}`"
    if status != 0 or not match or "reconstruct failed" in text:
        return f"FAILED (exit {status}): {last_line(log)}", how
    accounts = re.search(r"^(\d+) accounts", text, re.M)
    extra = f", {int(accounts.group(1)):,} accounts" if accounts else ""
    return f"{seconds_of(match):.1f} s{extra}", how


# Bootstrap logs

ISO = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?)Z")
ELAPSED = re.compile(r"^(\d+)s ")
KIND = re.compile(r'\bkind\s*[:=]\s*"([\w:]+)"')
# `LedgerService::block_apply` entries start their summary with the height.
SUMMARY_HEIGHT = re.compile(r'\bsummary\s*[:=]\s*"(\d+), ')
BLOCK_APPLY = "LedgerService::block_apply"


def events(path):
    """(seconds since the log's start, action kind, height) for every logged
    action; the height only for applied blocks.

    Reads run.mjs's console.log, whose entries start with seconds since the page
    was opened and continue over several lines, and native node logs, whose
    lines start with a UTC timestamp.
    """
    found, now, first, entry = [], None, None, None
    for line in open(path, errors="replace"):
        match = ELAPSED.match(line)
        if match:
            now, entry = int(match.group(1)), None
        else:
            match = ISO.match(line)
            if match:
                stamp = datetime.datetime.fromisoformat(match.group(1)).timestamp()
                first = stamp if first is None else first
                now, entry = stamp - first, None
        kind = KIND.search(line)
        if kind and now is not None:
            entry = [now, kind.group(1), None]
            found.append(entry)
        height = SUMMARY_HEIGHT.search(line)
        if height and entry is not None and entry[1] == BLOCK_APPLY:
            entry[2] = int(height.group(1))
    return found


def phases(found):
    """When each bootstrap phase ended, in seconds since the log's start."""

    def first(kind, after=0):
        return next((t for t, k, _ in found if k == kind and t >= after), None)

    start = first("TransitionFrontierSyncBestTipUpdate")
    staking = first("TransitionFrontierSyncLedgerStakingSuccess")
    next_epoch = first("TransitionFrontierSyncLedgerNextEpochSuccess")
    root = first("TransitionFrontierSyncLedgerRootSuccess")
    # A rebuild restarts when the sync target moves; the first attempt to the
    # success counts the restarts, the last attempt doesn't.
    rebuild_start = first("TransitionFrontierSyncLedgerStagedReconstructPending")
    rebuild_end = first("TransitionFrontierSyncLedgerStagedReconstructSuccess", rebuild_start or 0)
    last_attempt = max(
        (t for t, k, _ in found if k == "TransitionFrontierSyncLedgerStagedReconstructPending" and t <= (rebuild_end or 0)),
        default=None,
    )
    synced = first("TransitionFrontierSynced")
    # The first block applied after the root ledgers is the root's child. The
    # rebuild re-applies the root's scan state, so its time depends on how many
    # zkApp commands that holds, which depends on the root's height.
    root_height = next(
        (h - 1 for t, k, h in found if k == BLOCK_APPLY and h is not None and root is not None and t >= root),
        None,
    )
    span = lambda a, b: None if a is None or b is None else b - a
    return {
        "sync_start": start,
        "staking": span(start, staking),
        "next_epoch": span(staking, next_epoch),
        "root": span(next_epoch, root),
        "rebuild": span(rebuild_start, rebuild_end),
        "rebuild_last": span(last_attempt, rebuild_end),
        "blocks": span(root, synced),
        "synced": synced,
        "root_height": root_height,
    }


def phase_summary(p):
    fmt = lambda s: "?" if s is None else f"{s / 60:.1f}"
    rebuild = "?" if p["rebuild"] is None else f"{p['rebuild']:.0f} s"
    if p["rebuild_last"] is not None and p["rebuild_last"] != p["rebuild"]:
        rebuild += f", last attempt {p['rebuild_last']:.0f} s"
    root = "?" if p["root_height"] is None else p["root_height"]
    return (
        f"{fmt(p['staking'])}, {fmt(p['next_epoch'])}, {fmt(p['root'])} (rebuild {rebuild}), "
        f"{fmt(p['blocks'])} min; root at {root}"
    )


STATUS_LINE = re.compile(r"^(\d+)s rss=(\d+)MB wasm=(\S+?)MB status=(.*)$")


def wasm_memory(run_log):
    """(MiB when first synced, peak MiB) from run.mjs's run.log."""
    at_synced, peak = None, 0
    for line in open(run_log, errors="replace"):
        match = STATUS_LINE.match(line.strip())
        if not match:
            continue
        try:
            sizes = json.loads(match.group(3)) or []
        except ValueError:
            sizes = []
        size = max(sizes, default=0)
        peak = max(peak, size)
        if at_synced is None and '"sync":"Synced"' in match.group(4):
            at_synced = size
    return at_synced, peak


def browser_rows(run_dir, console, status_log):
    """Rows from a run.mjs run. The files default to the run directory's only
    when one is given, so that a console.log is never paired with an unrelated
    run.log."""
    if run_dir:
        console = console or os.path.join(run_dir, "console.log")
        status_log = status_log or os.path.join(run_dir, "run.log")
    rows = {}
    if console and not os.path.exists(console):
        rows["browser"] = (f"not measured: {console} does not exist", None)
    elif console:
        how = f"read from {console}"
        p = phases(events(console))
        if p["synced"] is None:
            rows["browser"] = ("not synced by the end of the log", how)
        else:
            rows["browser"] = (f"{minutes(p['synced'])} ({p['synced']:.0f} s)", how)
        rows["browser_phases"] = (phase_summary(p), how)
    if not status_log:
        rows["browser_memory"] = ("not measured: no run.log given (--browser-status)", None)
    elif not os.path.exists(status_log):
        rows["browser_memory"] = (f"not measured: {status_log} does not exist", None)
    else:
        at_synced, peak = wasm_memory(status_log)
        value = "not synced by the end of the log" if at_synced is None else f"{at_synced / 1024:.2f} GiB ({at_synced} MiB) of 4 GiB"
        rows["browser_memory"] = (f"{value}; peak {peak} MiB", f"read from {status_log}")
    return rows


def native_row(log):
    # The block applications it times are logged at DEBUG since O1: the
    # node must run with `--verbosity debug`.
    if not os.path.exists(log):
        return f"not measured: {log} does not exist", None
    p = phases(events(log))
    how = f"read from {log}, from its first line"
    if p["synced"] is None:
        return "not synced by the end of the log", how
    return f"{minutes(p['synced'])}: {phase_summary(p)}", how


# Soak


def process_start(pidfile):
    """When the process in a PID file started (UTC), or None if it isn't running."""
    try:
        pid = int(open(pidfile).read().strip())
        with open(f"/proc/{pid}/stat") as f:
            # Fields after the command name start at field 3; starttime is 22.
            ticks = int(f.read().rsplit(")", 1)[1].split()[19])
        boot = next(int(l.split()[1]) for l in open("/proc/stat") if l.startswith("btime "))
    except (OSError, ValueError, IndexError, StopIteration):
        return None
    return datetime.datetime.fromtimestamp(boot + ticks / os.sysconf("SC_CLK_TCK"), datetime.timezone.utc)


def mismatch_dumps(data_dir, watch_dir):
    """(time, height) of every staged ledger hash mismatch dump, oldest first.

    On a mismatch the node writes `<data dir>/debug/failed_application_ctx_<height>.binprot`
    and panics; the private soak watchdog moves the dump to
    `<watch dir>/<height>/handled-…`, which keeps its time, and restarts the node.
    """
    paths = []
    if data_dir:
        paths += glob.glob(os.path.join(data_dir, "debug", "failed_application_ctx_*.binprot"))
    if watch_dir:
        paths += glob.glob(os.path.join(watch_dir, "*", "handled-failed_application_ctx_*.binprot"))
    dumps = []
    for path in paths:
        height = re.search(r"_(\d+)\.binprot$", path)
        when = datetime.datetime.fromtimestamp(os.path.getmtime(path), datetime.timezone.utc)
        dumps.append((when, int(height.group(1)) if height else None))
    return sorted(dumps)


def soak_row(url, data_dir, watch_dir, pidfile):
    how = f"`GET {url}/status`"
    try:
        with urllib.request.urlopen(f"{url}/status", timeout=10) as response:
            status = json.load(response)
    except Exception as error:  # noqa: BLE001, any failure is the answer
        return f"no answer: {error}", how
    frontier = status.get("transition_frontier") or {}
    tip = frontier.get("best_tip") or {}
    sync = (frontier.get("sync") or {}).get("status")
    peers = sum(1 for p in status.get("peers") or [] if p.get("connection_status") == "Connected")
    value = f"{sync} at {tip.get('height')} ({str(tip.get('hash'))[:12]}…), {peers} peers connected"

    stamp = lambda t: t.strftime("%Y-%m-%d %H:%M UTC")
    started = process_start(pidfile) if pidfile else None
    if started:
        value += f"; node process up since {stamp(started)}"
        how += f"; the start time of the PID in {pidfile}"
    elif pidfile:
        value += f"; start time unknown (no running process in {pidfile}), so every dump counts"
    dirs = [d for d in (data_dir and os.path.join(data_dir, "debug"), watch_dir) if d and os.path.isdir(d)]
    if not dirs:
        return value + "; mismatches not checked (no --soak-data-dir or --soak-watch-dir)", how
    how += f"; mismatch dumps in {' and '.join(dirs)}"
    dumps = mismatch_dumps(data_dir, watch_dir)
    recent = [(t, h) for t, h in dumps if started is None or t >= started]
    if recent:
        listed = ", ".join(f"{h} at {stamp(t)}" for t, h in recent)
        value += f"; {len(recent)} staged ledger mismatch dump(s){' since then' if started else ''}: {listed}"
    else:
        value += f"; no staged ledger mismatch dump{' since then' if started else ''}"
        if dumps:
            value += f" (last before: {dumps[-1][1]} at {stamp(dumps[-1][0])})"
    return value, how


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--work",
        default=os.environ.get("MINA_WORK"),
        help="a directory laid out like the main checkout's .work/: replay/, "
        "genesis-mainnet/mesa-mainnet.bin, reconstruct-current.binprot "
        "(default $MINA_WORK)",
    )
    parser.add_argument(
        "--bin-dir",
        default=os.environ.get("MINA_BIN_DIR", os.path.join(REPO, "target", "release")),
        help="where mina-block-replay and examples/profile_reconstruct are "
        "(default $MINA_BIN_DIR or target/release)",
    )
    parser.add_argument("--replay", choices=["full", "subset", "none"], default="full")
    parser.add_argument("--replay-dir", help="chain.txt and the blocks (default WORK/replay)")
    parser.add_argument("--genesis", help="prebuilt genesis (default WORK/genesis-mainnet/mesa-mainnet.bin)")
    parser.add_argument(
        "--replay-log",
        help="read an earlier replay's output instead of replaying; with --replay subset if it was the subset",
    )
    parser.add_argument("--reconstruct", help="a staged ledger rebuild dump (default WORK/reconstruct-current.binprot)")
    parser.add_argument("--no-reconstruct", action="store_true")
    parser.add_argument("--browser-run", help="the OUT directory of a run.mjs run")
    parser.add_argument("--browser-console", help="its console.log, if elsewhere")
    parser.add_argument("--browser-status", help="its run.log (stdout), if elsewhere")
    parser.add_argument("--native-log", help="the log of a native node's bootstrap")
    parser.add_argument("--soak", metavar="URL", help="a running node's HTTP address, e.g. http://localhost:3200")
    parser.add_argument(
        "--soak-data-dir",
        help="the soak node's data directory (-d), whose debug/ gets mismatch dumps (default WORK/mainnet-node)",
    )
    parser.add_argument(
        "--soak-watch-dir", help="where the soak watchdog keeps handled mismatch dumps (default WORK/soak)"
    )
    parser.add_argument(
        "--soak-pidfile", help="the soak node's PID file, for its start time (default WORK/pids/native-mainnet-mesa.pid)"
    )
    parser.add_argument("--log-dir", help="where to keep the replay and rebuild output (default a new temp dir)")
    args = parser.parse_args()

    work = lambda *parts: os.path.join(args.work, *parts) if args.work else None
    log_dirs = []

    def log_dir():
        """The directory for the replay and rebuild output, made when first needed."""
        if not log_dirs:
            log_dirs.append(args.log_dir or tempfile.mkdtemp(prefix="mina-baseline-"))
            os.makedirs(log_dirs[0], exist_ok=True)
        return log_dirs[0]

    # Other work on the machine moves these numbers, so record how busy it was.
    load_at_start = os.getloadavg()[0]
    measured = {}
    replay_label = None

    if args.replay_log:
        measured["replay"] = read_replay_log(args.replay_log)
    elif args.replay != "none":
        replay_dir = args.replay_dir or work("replay")
        genesis = args.genesis or work("genesis-mainnet", "mesa-mainnet.bin")
        binary = os.path.join(args.bin_dir, "mina-block-replay")
        missing = [p for p in (replay_dir, genesis, binary) if not p or not os.path.exists(p)]
        if missing:
            measured["replay"] = (f"not run: missing {missing}", "set --work, or --replay-dir and --genesis")
        else:
            to = SUBSET_TO if args.replay == "subset" else None
            measured["replay"] = replay(binary, replay_dir, genesis, to, log_dir())
    # With --replay-log, --replay subset says the log is of the subset.
    if args.replay == "subset":
        replay_label = f"Replay subset, {SUBSET_TO - GENESIS_HEIGHT:,} blocks to {SUBSET_TO}"

    dump = args.reconstruct or work("reconstruct-current.binprot")
    if not args.no_reconstruct:
        binary = os.path.join(args.bin_dir, "examples", "profile_reconstruct")
        if dump and os.path.exists(dump) and os.path.exists(binary):
            measured["reconstruct"] = reconstruct(binary, dump, log_dir())
        else:
            measured["reconstruct"] = (
                f"not run: missing {[p for p in (dump, binary) if not p or not os.path.exists(p)]}",
                "set --work or --reconstruct, and build the example",
            )

    if args.browser_run or args.browser_console or args.browser_status:
        measured.update(browser_rows(args.browser_run, args.browser_console, args.browser_status))
    if args.native_log:
        measured["native"] = native_row(args.native_log)
    if args.soak:
        measured["soak"] = soak_row(
            args.soak.rstrip("/"),
            args.soak_data_dir or work("mainnet-node"),
            args.soak_watch_dir or work("soak"),
            args.soak_pidfile or work("pids", "native-mainnet-mesa.pid"),
        )

    recorded = recorded_baseline()
    commit = subprocess.run(
        ["git", "-C", REPO, "log", "-1", "--format=%h %cs %s"], capture_output=True, text=True
    ).stdout.strip()
    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M UTC")
    load = f"{load_at_start:.0f} at the start, {os.getloadavg()[0]:.0f} at the end, on {os.cpu_count()} CPUs"
    print(f"Baseline measured {now}, checkout at {commit or 'unknown'}.")
    print(f"Load average {load}.\n")
    print("| Measure | Recorded reference (optional) | Measured now |")
    print("| --- | --- | --- |")
    notes = {}
    for key, label in ROWS:
        if key == "replay" and replay_label:
            label = replay_label
        elif key == "replay":
            label = "Full replay of every Mesa mainnet block in the replay directory"
        value, how = measured.get(key, ("not measured", None))
        if how:
            notes.setdefault(how, []).append(label)
        # The backlog records the full replay only.
        before = "—" if key == "replay" and replay_label else recorded.get(key, "—")
        print(f"| {label} | {before} | {value} |")
    print("\nHow:\n")
    for how, labels in notes.items():
        print(f"- {'; '.join(labels)}: {how}")
    if not notes:
        print("- nothing was measured")
    if log_dirs:
        print(f"\nReplay and rebuild output is in {log_dirs[0]}.")


if __name__ == "__main__":
    main()
