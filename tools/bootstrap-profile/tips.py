#!/usr/bin/env python3
"""Connected peers, peers whose best tip equals the sync target, and requests
in flight over time.

Usage: tips.py samples.jsonl [STEP_S]
"""

import argparse
import collections
import json

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("samples", help="the sampler's samples.jsonl")
ap.add_argument("step", nargs="?", type=float, default=10, help="seconds between printed rows (default 10)")
args = ap.parse_args()
rows = [json.loads(line) for line in open(args.samples)]
step = args.step
t0 = rows[0]["t"]
last = -1e9
for r in rows:
    s = r.get("sync") or {}
    tgt = (s.get("target") or {}).get("hash")
    p = r.get("peers") or {}
    rd = p.get("ready") if isinstance(p, dict) else None
    if rd is None or not tgt:
        continue
    if r["t"] - last < step:
        continue
    last = r["t"]
    same = sum(1 for x in rd if x["tip"] == tgt)
    hs = collections.Counter(x["h"] for x in rd)
    prog = r.get("progress") if isinstance(r.get("progress"), dict) else {}
    inflight = len(prog.get("messages_stats") or {})
    bs = p.get("by_status")
    print(
        f"{r['t'] - t0:6.1f} {s.get('status'):24s} connected {len(rd):3d} "
        f"tip=target {same:3d} in-flight {inflight:3d} statuses {bs} heights {dict(hs)}"
    )
