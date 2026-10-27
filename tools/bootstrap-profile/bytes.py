#!/usr/bin/env python3
"""Bytes received per sync status, from the sampler's per-socket counters.

Usage: bytes.py samples.jsonl

Increments between consecutive samples of the same socket are attributed to
the later sample's status. Data received by a socket after its last sample
before it closed is missed, so these are lower bounds.
"""

import argparse
import collections
import json

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("samples", help="the sampler's samples.jsonl")
args = ap.parse_args()
rows = [json.loads(line) for line in open(args.samples)]
t0 = rows[0]["t"]
prev = {}
by_status = collections.OrderedDict()
top_socket = collections.defaultdict(collections.Counter)
for r in rows:
    socks = r.get("sockets")
    if socks is None:
        continue
    st = (r.get("sync") or {}).get("status") or "?"
    e = by_status.setdefault(st, {"bytes": 0, "t0": r["t"] - t0, "t1": 0})
    e["t1"] = r["t"] - t0
    for k, v in socks.items():
        inc = v - prev.get(k, 0)
        if inc > 0:
            e["bytes"] += inc
            top_socket[st][k.split("-")[1]] += inc
        prev[k] = v
for st, e in by_status.items():
    dur = max(e["t1"] - e["t0"], 1e-9)
    tops = top_socket[st].most_common(3)
    print(
        f"{st:26s} {e['t0']:6.1f}-{e['t1']:6.1f}s {e['bytes'] / 1e6:8.1f} MB "
        f"({e['bytes'] / 1e6 / dur:5.2f} MB/s)  top sockets: "
        + ", ".join(f"{k} {v / 1e6:.1f} MB" for k, v in tops)
    )
