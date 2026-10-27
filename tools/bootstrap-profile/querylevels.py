#!/usr/bin/env python3
"""Count ledger sync responses by address length and size, per phase window.

Usage: querylevels.py LOG [boundary_s ...]  (seconds from the first log line)

Needs a log from a node with patches/rpc-logging.patch applied, which logs
the tree level and size of each ledger sync answer.
"""

import argparse
import collections
import datetime as dt
import re

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("log", help="node log with the RPC logging")
ap.add_argument("bounds", nargs="*", type=float, help="window boundaries, seconds from the first log line")
args = ap.parse_args()
log = args.log
bounds = args.bounds
ts = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z")
pat = re.compile(r'kind[=:] ?"TransitionFrontierSyncLedgerSnarked(Child\w+)".*?at[=:] ?(\d+),? (n|changed)[=:] ?(\d+)')
t0 = None
counts = collections.defaultdict(collections.Counter)
changed = collections.defaultdict(collections.Counter)
for line in open(log, errors="replace"):
    m = ts.match(line)
    if not m:
        continue
    t = dt.datetime.fromisoformat(m.group(1)[:26]).replace(tzinfo=dt.timezone.utc).timestamp()
    if t0 is None:
        t0 = t
    m = pat.search(line)
    if not m:
        continue
    rel = t - t0
    w = sum(1 for b in bounds if rel >= b)
    kind, at, key, n = m.group(1), int(m.group(2)), m.group(3), int(m.group(4))
    if kind == "ChildHashesReceived":
        counts[w][("hashes", at, n)] += 1
    elif kind == "ChildAccountsReceived":
        counts[w][("accounts", at, min(n, 64))] += 1
    elif kind == "ChildHashesAccepted":
        changed[w][at] += n
for w in sorted(counts):
    c = counts[w]
    tot = sum(c.values())
    print(f"window {w}: {tot} responses")
    for (k, at, n), v in sorted(c.items()):
        if k == "accounts" and n == 64:
            print(f"  {k:8s} at {at:2d} n={n:3d}: {v}")
        elif k == "hashes":
            print(f"  {k:8s} at {at:2d} n={n:3d}: {v}   (children that differed and were queued: {changed[w][at]})")
    small = sum(v for (k, at, n), v in c.items() if k == "accounts" and n < 64)
    print(f"  accounts with fewer than 64: {small}")
