#!/usr/bin/env python3
"""Stop this run's recorded processes and their descendants: SIGTERM, a grace
period, then SIGKILL. Never matches by name. With `--name rust-old` (a `.pid`
file's stem) it stops that one process tree; without, everything recorded,
including the namespace keeper.
"""

import argparse
import json
import os
from pathlib import Path
import signal
import time


def snapshot():
    result = {}
    for path in Path("/proc").iterdir():
        if not path.name.isdigit():
            continue
        try:
            fields = (path / "stat").read_text().rsplit(")", 1)[1].split()
            result[int(path.name)] = {"ppid": int(fields[1]), "start": fields[19]}
        except (OSError, IndexError):
            pass
    return result


def track(work, only=None):
    """Recorded roots (`*.pid` files whose process names this run directory)
    and their descendants, remembered with start times in processes.json."""
    path = work / "processes.json"
    known = json.loads(path.read_text()) if path.exists() else {}
    current = snapshot()
    roots = set()
    for pidfile in work.glob("*.pid"):
        if only and pidfile.stem not in (only, f"{only}-launcher"):
            continue
        try:
            pid = int(pidfile.read_text())
            cmdline = Path(f"/proc/{pid}/cmdline").read_bytes()
        except (OSError, ValueError):
            continue
        if str(work).encode() in cmdline and pid in current:
            roots.add(pid)
    tree = set(roots)
    while True:
        grown = tree | {pid for pid, row in current.items() if row["ppid"] in tree}
        if grown == tree:
            break
        tree = grown
    for pid in tree:
        known[str(pid)] = current[pid]
    path.write_text(json.dumps(known, indent=2) + "\n")
    if only:
        return {str(pid): current[pid] for pid in tree}
    return known


def alive(known):
    current = snapshot()
    return [int(pid) for pid, row in known.items()
            if current.get(int(pid), {}).get("start") == row["start"]]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--name")
    parser.add_argument("--grace", type=int, default=60)
    args = parser.parse_args()
    work = args.dir.resolve()
    known = track(work, args.name)
    for pid in alive(known):
        try:
            os.kill(pid, signal.SIGTERM)
            print(f"SIGTERM owned PID {pid}")
        except ProcessLookupError:
            pass
    deadline = time.time() + args.grace
    while alive(known) and time.time() < deadline:
        time.sleep(1)
    for pid in alive(known):
        try:
            os.kill(pid, signal.SIGKILL)
            print(f"SIGKILL owned PID {pid} after {args.grace} s")
        except ProcessLookupError:
            pass
    time.sleep(2)
    remaining = alive(known)
    print(json.dumps({"stopped": args.name or "all", "remaining": remaining}))


if __name__ == "__main__":
    main()
