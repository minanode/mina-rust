#!/usr/bin/env python3
"""Stop the experiment's recorded PIDs and descendants, never name-match kill."""

import argparse
import os
from pathlib import Path
import signal
import time

from processes import snapshot, track


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--role", choices=("rust", "ocaml", "sample", "payments", "zkapp-send", "namespace", "reserve"))
    args = parser.parse_args()
    known = track(args.dir.resolve())
    if args.role:
        root = int((args.dir / f"{args.role}.pid").read_text())
        selected = {root}
        while True:
            new = selected | {int(pid) for pid, row in known.items() if row["ppid"] in selected}
            if new == selected:
                break
            selected = new
        known = {pid: row for pid, row in known.items() if int(pid) in selected}
    for sig in (signal.SIGTERM, signal.SIGKILL):
        current = snapshot()
        for pid, row in known.items():
            pid = int(pid)
            if current.get(pid, {}).get("start") == row["start"]:
                try:
                    os.kill(pid, sig)
                    print(f"Sent {sig.name} to owned PID {pid}")
                except ProcessLookupError:
                    pass
        time.sleep(5)
    current = snapshot()
    alive = [pid for pid, row in known.items()
             if current.get(int(pid), {}).get("start") == row["start"]]
    print("Recorded processes remaining:", alive)


if __name__ == "__main__":
    main()
