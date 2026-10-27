#!/usr/bin/env python3
"""Per-peer LedgerQuery counts and latencies in a window, with addresses.

Usage: perpeer.py LOG SAMPLES.jsonl START_S END_S (seconds from first log line)

Needs a log from a node with patches/rpc-logging.patch applied.
"""

import argparse
import collections
import json
import re
import statistics

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("log", help="node log with the RPC logging")
ap.add_argument("samples", help="the sampler's samples.jsonl, for peer addresses")
ap.add_argument("start", type=float, help="window start, seconds from the first log line")
ap.add_argument("end", type=float, help="window end, seconds from the first log line")
args = ap.parse_args()
log, samples, ws, we = args.log, args.samples, args.start, args.end
addr = {}
try:
    for line in open(samples):
        r = json.loads(line)
        p = r.get("peers")
        if isinstance(p, dict):
            for x in p.get("ready") or []:
                addr[x["id"]] = x["addr"]
except FileNotFoundError:
    pass
pat = re.compile(
    r'time[=:] ?"(\d+)",? kind[=:] ?"P2pChannelsRpc(\w+)",? peer_id[=:] ?(\w+),? id[=:] ?(\d+)(?:,? rpc[=:] ?(\S+))?'
)
ts = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z")
t0 = None
pending = {}
lat = collections.defaultdict(list)
for line in open(log, errors="replace"):
    if t0 is None:
        m = ts.match(line)
        if m:
            import datetime as dt

            t0 = dt.datetime.fromisoformat(m.group(1)[:26]).replace(tzinfo=dt.timezone.utc).timestamp()
    if "P2pChannelsRpc" not in line:
        continue
    m = pat.search(line)
    if not m:
        continue
    t = int(m.group(1)) / 1e9 - t0
    k, peer, rid, rpc = m.group(2), m.group(3), m.group(4), m.group(5)
    if k == "RequestSend":
        pending[(peer, rid)] = (t, rpc)
    elif k in ("ResponseReceived", "Timeout"):
        r = pending.pop((peer, rid), None)
        if r and r[1] == "LedgerQuery" and ws <= r[0] < we:
            lat[peer].append(t - r[0])
rows = sorted(lat.items(), key=lambda kv: -len(kv[1]))
total = sum(len(v) for v in lat.values())
print(f"{total} ledger queries from {len(rows)} peers in {ws}-{we}s")
for peer, ls in rows:
    a = addr.get(peer, "?")
    ip = re.search(r"/ip4/([\d.]+)/tcp/(\d+)", a)
    print(
        f"{len(ls):6d} {100 * len(ls) / total:5.1f}%  med {statistics.median(ls) * 1000:6.0f}ms "
        f"p90 {sorted(ls)[int(0.9 * len(ls))] * 1000:6.0f}ms  {peer[:14]} {ip.group(1) + ':' + ip.group(2) if ip else a}"
    )
