#!/usr/bin/env python3
"""Poll `mina node status --check` and independently check node log growth.

Writes UTC alert/recovery lines to stdout. Does not restart or signal the node.
Only needs Python's standard library. Run --help for thresholds and cadence.
"""

import argparse
import datetime
import os
from pathlib import Path
import stat
import subprocess
import time


class LogGrowth:
    def __init__(self):
        self.identity = None
        self.size = 0
        self.last_growth = None

    def check(self, path, maximum, now, wall):
        try:
            info = os.stat(path)
            if not stat.S_ISREG(info.st_mode):
                return "log path is not a regular file"
        except OSError as error:
            return f"log unavailable: {error.strerror}"
        identity = (info.st_dev, info.st_ino)
        if info.st_mtime > wall:
            return "log mtime is in the future (check clocks)"
        if self.last_growth is None:
            # First sample cannot prove growth. Use mtime as a bootstrap proxy;
            # an empty file has no evidence of any log output.
            self.last_growth = now - max(0, wall - info.st_mtime)
        elif identity == self.identity and info.st_size > self.size:
            self.last_growth = now
        elif identity != self.identity and info.st_size > 0:
            # Rotation to an old file must not look like fresh logging.
            self.last_growth = max(self.last_growth, now - max(0, wall - info.st_mtime))
        self.identity, self.size = identity, info.st_size
        age = max(0, now - self.last_growth)
        if info.st_size == 0:
            return "log is empty (no growth observed)"
        if age > maximum:
            return f"log has not grown for {int(age)}s (limit {maximum:g}s)"
        return None


def positive(value):
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return number


def nonnegative(value):
    number = int(value)
    if number < 0:
        raise argparse.ArgumentTypeError("must be a nonnegative integer")
    return number


def sample(args, growth):
    command = [args.mina, "node", "status", "--check", "--node", args.url,
               "--max-tip-age", str(args.max_tip_age), "--min-peers", str(args.min_peers),
               "--timeout", str(args.timeout)]
    if args.status_only:
        command.append("--status-only")
    reasons = []
    # Check before HTTP reads, which may themselves append request logs.
    if args.log_file:
        reason = growth.check(args.log_file, args.max_log_silence * 60,
                              time.monotonic(), time.time())
        if reason:
            reasons.append(reason)
    try:
        result = subprocess.run(command, capture_output=True, text=True,
                                timeout=3 * args.timeout + 5, check=False)
        if result.returncode:
            detail = " ".join((result.stdout or result.stderr).split())[:2000]
            reasons.append(detail or f"status exited {result.returncode}")
    except (OSError, subprocess.TimeoutExpired) as error:
        reasons.append(f"status command failed: {error}")
    return reasons


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mina", default="mina", help="path to mina binary")
    parser.add_argument("--url", default="http://127.0.0.1:3000")
    parser.add_argument("--status-only", action="store_true", help="only GET /status on the node")
    parser.add_argument("--interval", type=positive, default=60, help="seconds between checks")
    parser.add_argument("--max-tip-age", type=nonnegative, default=1800, help="seconds")
    parser.add_argument("--min-peers", type=nonnegative, default=1)
    parser.add_argument("--timeout", type=positive, default=5, help="per HTTP request, seconds (max 300)")
    parser.add_argument("--log-file", type=Path)
    parser.add_argument("--max-log-silence", type=positive, default=15, help="minutes without log growth")
    parser.add_argument("--once", action="store_true", help="one check; log uses mtime as initial proxy")
    args = parser.parse_args()
    if args.timeout > 300:
        parser.error("--timeout must be at most 300 seconds")
    growth, was_unhealthy = LogGrowth(), False
    while True:
        reasons = sample(args, growth)
        timestamp = datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")
        if reasons:
            print(f"{timestamp} ALERT: {'; '.join(' '.join(r.split()) for r in reasons)}", flush=True)
        elif was_unhealthy:
            print(f"{timestamp} RECOVERED: status and log checks passed", flush=True)
        elif args.once:
            print(f"{timestamp} OK: status and log checks passed", flush=True)
        was_unhealthy = bool(reasons)
        if args.once:
            return int(was_unhealthy)
        time.sleep(args.interval)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(0)
