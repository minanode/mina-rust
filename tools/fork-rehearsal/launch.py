#!/usr/bin/env python3
"""Launch one rehearsal node in the run's namespace under the run's deadline.

The first launch fixes a deadline (default 6000 s) shared by every later
launch of the run; `timeout` signals the node if it is still running then.
"""

import argparse
import json
import os
from pathlib import Path
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("role", choices=("rust", "ocaml"))
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--phase", choices=("old", "new"), required=True)
    parser.add_argument("--mina", type=Path)
    parser.add_argument("--blobs", type=Path)
    parser.add_argument("--producer")
    parser.add_argument("--deadline-seconds", type=int, default=6000)
    args = parser.parse_args()
    os.umask(0o077)
    work = args.dir.resolve()
    root = Path(__file__).resolve().parents[2]
    deadline = work / "node-deadline.json"
    if not deadline.exists():
        now = time.time()
        deadline.write_text(json.dumps({"started_unix": now,
                                        "stop_unix": now + args.deadline_seconds}) + "\n")
    remaining = int(json.loads(deadline.read_text())["stop_unix"] - time.time())
    assert remaining > 0, "the run's node deadline has passed"
    name = f"{args.role}-{args.phase}"
    (work / f"{name}-launcher.pid").write_text(str(os.getpid()) + "\n")
    command = ["timeout", "--foreground", "--signal=TERM", "--kill-after=30s", str(remaining),
               "bash", str(root / "tools/local-mesa-net/enter.sh"), str(work),
               "python3", str(root / "tools/fork-rehearsal/node.py"), args.role,
               "--dir", str(work), "--phase", args.phase]
    for flag, value in (("--mina", args.mina), ("--blobs", args.blobs),
                        ("--producer", args.producer)):
        if value is not None:
            command.extend((flag, str(Path(value).resolve()) if flag != "--producer" else value))
    os.execvp(command[0], command)


if __name__ == "__main__":
    main()
