#!/usr/bin/env python3
"""Reserve a shared node slot before creating a time-sensitive private genesis.

Run under lock.sh node. The held marker contains our PID; touch the start marker
only once this role's configuration/identity checks have completed. The eventual
daemon keeps this PID and the inherited lock. This never skips the shared lock.
"""

import argparse
import os
from pathlib import Path
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ticket", type=Path, required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    os.fstat(9)
    command = args.command
    if command and command[0] == "--":
        command = command[1:]
    assert command, "missing command"
    held = args.ticket.with_suffix(".held")
    start = args.ticket.with_suffix(".start")
    assert not held.exists() and not start.exists(), "use a fresh ticket"
    held.write_text(str(os.getpid()) + "\n")
    print(f"Reserved shared node slot for {args.ticket.name}, PID {os.getpid()}", flush=True)
    while not start.exists():
        time.sleep(1)
    os.umask(0o077)
    with args.log.open("w") as log:
        os.dup2(log.fileno(), 1)
        os.dup2(log.fileno(), 2)
    os.execvp(command[0], command)


if __name__ == "__main__":
    main()
