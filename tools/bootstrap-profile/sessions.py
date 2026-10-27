#!/usr/bin/env python3
"""Per-peer connection sessions from the RPC logging: a session starts with the
get_best_tip request (id 0) sent when a connection opens and ends at that
peer's last RPC event.

Usage: sessions.py LOG [v]

Needs a log from a node with patches/rpc-logging.patch applied.
"""

import argparse
import collections
import re
import statistics
import sys

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("log", help="node log with the RPC logging")
ap.add_argument("v", nargs="?", help="any value: also list every session")
args = ap.parse_args()
path = args.log
ev = collections.defaultdict(list)
pat = re.compile(
    r'time[=:] ?"(\d+)",? kind[=:] ?"P2pChannelsRpc(\w+)",? peer_id[=:] ?(\w+),? id[=:] ?(\d+)(?:,? rpc[=:] ?(\S+))?'
)
for line in open(path, errors="replace"):
    if "P2pChannelsRpc" not in line:
        continue
    m = pat.search(line)
    if not m:
        continue
    t = int(m.group(1)) / 1e9
    ev[m.group(3)].append((t, m.group(2), int(m.group(4)), m.group(5)))
if not ev:
    sys.exit(f"{path}: no RPC events; the node needs patches/rpc-logging.patch")
t0 = min(e[0][0] for e in ev.values())
sess = []
for p, es in ev.items():
    cur = None
    for t, k, i, r in es:
        if k == "RequestSend" and i == 0:
            if cur:
                sess.append(cur)
            cur = {"peer": p, "start": t, "end": t, "n": 0, "blocks": 0, "lat": []}
        if cur:
            cur["end"] = t
            if k == "RequestSend" and r == "LedgerQuery":
                cur["n"] += 1
            if k == "RequestSend" and r == "Block":
                cur["blocks"] += 1
    if cur:
        sess.append(cur)
print("peers", len(ev), "sessions", len(sess))
durs = sorted(s["end"] - s["start"] for s in sess)
print(
    "session (first to last RPC event) duration median %.1f p10 %.1f p90 %.1f"
    % (statistics.median(durs), durs[len(durs) // 10], durs[9 * len(durs) // 10])
)
per_peer = collections.Counter(s["peer"] for s in sess)
print("sessions per peer:", collections.Counter(per_peer.values()))
if args.v:
    for s in sorted(sess, key=lambda s: s["start"]):
        print(
            f"{s['peer'][:12]} start {s['start'] - t0:6.1f} end {s['end'] - t0:6.1f} "
            f"dur {s['end'] - s['start']:6.1f} ledgerq {s['n']} blocks {s['blocks']}"
        )
