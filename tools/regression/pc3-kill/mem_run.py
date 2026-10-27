"""PC3-KILL: a devnet follower under a 1 GB memory cap, sampled by phase.

  python3 -I tools/regression/pc3-kill/mem_run.py --run DIR --bin MINA \
      --http 4165 --p2p 9365 --unit pc3k-mem [--reset] [--minutes 30]

Starts tools/regression/devnet/node.sh as a `systemd-run --user` unit with
MemoryMax=1G and MemorySwapMax=0, the Raspberry Pi 3's RAM with no swap
(and with --cpus 0-3 its four cores, which size the node's rayon pool), and
samples the unit's cgroup every second into DIR/mem-NAME.tsv:
"utc phase current_mib anon_mib file_mib peak_mib oom_kill". The phase is the
last of the log's events below seen so far. Runs for --minutes (0: until the
unit stops), then stops the unit with SIGTERM unless --keep, and writes
DIR/mem-NAME.json: per phase the peak anonymous and total memory, the time to
NodeSynced, any OOM kill, and how often the cgroup reached its limit
(`memory.events` `max`). --reset starts from an empty frontier store
(a fresh bootstrap); without it the node restores from the one in DIR/home.
"""

import argparse
import json
import os
import re
import subprocess
import time

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "../../.."))
PHASES = [
    ("starting", r'kind: "NodeStart"'),
    # A restore: the store's load (its checks, the ledgers rebuilt in
    # memory), then the replay of its blocks.
    ("load", r'kind: "PersistOpened", summary: "frontier store opened generation'),
    ("replay", r'kind: "PersistLoad", summary: "frontier store loaded'),
    ("staking", r"no ledger to copy for sync|kind: \"TransitionFrontierSyncBestTipUpdate\""),
    ("next", r'kind: "TransitionFrontierSyncLedgerNextEpochPending"'),
    ("root", r'kind: "TransitionFrontierSyncLedgerRootPending"'),
    ("blocks", r'kind: "TransitionFrontierSyncLedgerRootSuccess"'),
    ("restored", r'kind: "PersistRestored"'),
    ("synced", r'kind: "NodeSynced"'),
    ("steady", r'kind: "PersistCheckpoint"'),
]


def cgroup(unit):
    out = subprocess.run(["systemctl", "--user", "show", "-p", "ControlGroup", "--value", unit + ".service"],
                         capture_output=True, text=True).stdout.strip()
    return "/sys/fs/cgroup" + out if out else None


def read(path):
    try:
        return open(path).read()
    except OSError:
        return ""


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    for name in ("--run", "--bin", "--unit"):
        p.add_argument(name, required=True)
    p.add_argument("--http", type=int, required=True)
    p.add_argument("--p2p", type=int, required=True)
    p.add_argument("--name", default="run")
    p.add_argument("--reset", action="store_true")
    p.add_argument("--keep", action="store_true", help="leave the unit running at the end")
    p.add_argument("--minutes", type=float, default=30)
    p.add_argument("--extra", default="")
    p.add_argument("--cpus", default="", help="CPUAffinity, e.g. 0-3 for a Pi 3's four cores; the node's "
                   "rayon pool is sized from it")
    a = p.parse_args()
    run = os.path.abspath(a.run)
    os.makedirs(run, exist_ok=True)
    log_path = os.path.join(run, "node-%s.log" % a.name)
    extra = (a.extra + (" --frontier-store-reset" if a.reset else "")).strip()
    subprocess.run(["systemd-run", "--user", "--unit", a.unit, "--collect", "--quiet",
                    "-p", "MemoryMax=1G", "-p", "MemorySwapMax=0",
                    *(["-p", "CPUAffinity=" + a.cpus] if a.cpus else []),
                    "-p", "StandardOutput=append:" + log_path, "-p", "StandardError=append:" + log_path,
                    "-p", "KillMode=mixed", "-p", "TimeoutStopSec=60",
                    "-E", "MINA=" + os.path.abspath(a.bin), "-E", "EXTRA_ARGS=" + extra,
                    os.path.join(REPO, "tools/regression/devnet/node.sh"), run, str(a.http), str(a.p2p)],
                   check=True)
    t0 = time.time()
    cg = None
    for _ in range(50):
        cg = cgroup(a.unit)
        if cg and os.path.isdir(cg):
            break
        time.sleep(0.2)
    tsv = open(os.path.join(run, "mem-%s.tsv" % a.name), "a")
    phase, seen, pos = "starting", {}, 0
    stats = {}
    oom = at_max = 0
    end = t0 + a.minutes * 60 if a.minutes else None
    while cg and os.path.isdir(cg):
        try:
            with open(log_path, errors="replace") as f:
                f.seek(pos)
                chunk = f.read()
                pos = f.tell()
        except FileNotFoundError:
            chunk = ""
        for line in chunk.split("\n"):
            for name, pattern in PHASES:
                if name not in seen and re.search(pattern, line):
                    seen[name] = round(time.time() - t0, 1)
                    phase = name
        stat = dict(l.split() for l in read(cg + "/memory.stat").splitlines() if l.count(" ") == 1)
        ev = dict(l.split() for l in read(cg + "/memory.events").splitlines() if l.count(" ") == 1)
        cur = int(read(cg + "/memory.current") or 0)
        peak = int(read(cg + "/memory.peak") or 0)
        anon, file = int(stat.get("anon", 0)), int(stat.get("file", 0))
        oom = int(ev.get("oom_kill", 0))
        at_max = int(ev.get("max", 0))
        mib = lambda b: round(b / 1048576, 1)
        tsv.write("%s\t%s\t%s\t%s\t%s\t%s\t%d\n" % (time.strftime("%FT%TZ", time.gmtime()), phase, mib(cur),
                                                    mib(anon), mib(file), mib(peak), oom))
        tsv.flush()
        s = stats.setdefault(phase, {"anon_peak_mib": 0, "current_peak_mib": 0, "samples": 0})
        s["anon_peak_mib"] = max(s["anon_peak_mib"], mib(anon))
        s["current_peak_mib"] = max(s["current_peak_mib"], mib(cur))
        s["samples"] += 1
        if end and time.time() > end:
            if not a.keep:
                subprocess.run(["systemctl", "--user", "stop", a.unit])
            break
        time.sleep(1)
    text = read(log_path)
    summary = {
        "name": a.name, "reset": a.reset, "cpus": a.cpus or "all", "start_utc": time.strftime("%FT%TZ", time.gmtime(t0)),
        "events_s": seen, "synced_s": seen.get("synced"), "restored": "restored" in seen,
        "oom_kill": oom, "memory_max_events": at_max, "phases": stats, "panic": bool(re.search(r"panicked", text)),
        "cgroup_peak_mib": max((s["current_peak_mib"] for s in stats.values()), default=0),
        "anon_peak_mib": max((s["anon_peak_mib"] for s in stats.values()), default=0),
    }
    json.dump(summary, open(os.path.join(run, "mem-%s.json" % a.name), "w"), indent=1)
    print(json.dumps(summary))


if __name__ == "__main__":
    main()
