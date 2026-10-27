#!/usr/bin/env python3
"""Alert on proof verification failures in a running node's log.

Reads the log from where the previous call stopped and, if the new part has
`verify_transactions OK=false` lines, prints one alert line and exits 1. A
transaction SNARK that fails against the embedded key is either a peer's
invalid work, which OCaml peers don't relay, or a key or verifier regression on
our side (private implementation notes). The node dumps the failed
proofs as verify_txs_*.binprot into its debug directory.

    tools/regression/verify_alert.py --state FILE [--pattern REGEX] LOG

The state file keeps the log's inode and the offset read so far; it is made on
the first call, which reads the whole log. A new inode (the log was rotated)
or a shorter file starts again from the top; lines written to the old file
after the previous call are not read. Only complete lines are read, so a line
being written is read on the next call. Exit status: 0 nothing new, 1 an alert
(on stdout), 2 a usage or I/O error.

It only reads the log and writes the state file, so it is safe to run next to a
live node, for example once a minute from a watchdog loop:

    tools/regression/verify_alert.py --state $RUN_DIR/verify-alert.state \\
        logs/native-mainnet-mesa.log >> $RUN_DIR/events.log

Only the standard library is used.
"""

import argparse
import datetime
import os
import re
import sys

DEFAULT_PATTERN = r"verify_transactions OK=false"
OK_PATTERN = re.compile(rb"verify_transactions OK=true")
ANSI = re.compile(rb"\x1b\[[0-9;]*[A-Za-z]")


def read_state(path):
    try:
        with open(path) as f:
            inode, offset = f.read().split()
            return int(inode), int(offset)
    except (OSError, ValueError):
        return None, 0


def write_state(path, inode, offset):
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        f.write(f"{inode} {offset}\n")
    os.replace(tmp, path)


def check(log, state, pattern):
    """Returns the alert line, or None, and advances the state."""
    failed, ok, last = 0, 0, b""
    with open(log, "rb") as f:
        stat = os.fstat(f.fileno())
        old_inode, offset = read_state(state)
        if old_inode != stat.st_ino or offset > stat.st_size:
            offset = 0
        f.seek(offset)
        # Line by line, so a long log isn't read into memory at once.
        for line in f:
            if not line.endswith(b"\n"):
                break
            offset += len(line)
            if pattern.search(line):
                failed += 1
                last = line
            elif OK_PATTERN.search(line):
                ok += 1
    write_state(state, stat.st_ino, offset)

    if not failed:
        return None
    last = ANSI.sub(b"", last).strip().decode("utf-8", "replace")[:200]
    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    return (
        f"{now} ALERT {failed} new line(s) matching '{pattern.pattern.decode()}' "
        f"in {log} (and {ok} verify_transactions OK=true); last: {last}"
    )


def main():
    parser = argparse.ArgumentParser(
        description=__doc__.split("\n\n")[0],
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("log", help="the node's log")
    parser.add_argument("--state", required=True, help="inode and offset read so far")
    parser.add_argument(
        "--pattern",
        default=DEFAULT_PATTERN,
        help="regex of the lines to alert on (default: %(default)s; block and "
        "zkApp proofs too: 'verify_(transactions|block|zkapp) OK=false')",
    )
    args = parser.parse_args()
    try:
        alert = check(args.log, args.state, re.compile(args.pattern.encode()))
    except OSError as e:
        print(f"verify_alert.py: {e}", file=sys.stderr)
        return 2
    if alert is None:
        return 0
    print(alert, flush=True)
    return 1


if __name__ == "__main__":
    sys.exit(main())
