"""Estimate concurrently connected libp2p peers over time from a node log.

Usage: python3 peers_over_time.py LOG SEEDS.txt HOSTS.txt [MINUTES]
(see census.py for SEEDS.txt and HOSTS.txt).

A connection starts at a ready burst (channel-not-supported warnings) and ends
at the next fatal connection error on one of the peer's addresses, or at our
own disconnection of the peer, whichever comes first; a new ready ends the
previous interval too.
"""
import collections
import os
import re
import sys
from datetime import timedelta

sys.dont_write_bytecode = True  # no __pycache__ next to the scripts
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import census  # noqa: E402

if len(sys.argv) < 4:
    sys.exit('usage: peers_over_time.py LOG SEEDS.txt HOSTS.txt [MINUTES]')
log, seeds_path, resolved_path = sys.argv[1:4]
bucket_min = int(sys.argv[4]) if len(sys.argv) > 4 else 30
resolved = []
for r in open(resolved_path).read().split():
    host, _, ips = r.partition('=')
    resolved.append((host, [i for i in ips.split(',') if i]))
seeds = census.SeedMap(seeds_path, resolved)

addr_peers = collections.defaultdict(set)
for a, name in seeds.by_addr.items():
    addr_peers[a].add('seed:' + name)

peer_alias = {}
events = []
first = last = None
for t, level, text in census.read_records(log):
    first = first or t
    last = t
    km = census.KIND_RE.search(text)
    kind = km.group(1) if km else None
    pm = census.PEER_RE.search(text)
    if kind == 'P2pNetworkKademliaUpdateRoutingTable' and pm:
        p = pm.group(1)
        for ip, port in re.findall(r'/ip4/(\d+\.\d+\.\d+\.\d+)/tcp/(\d+)', text):
            addr_peers[f'{ip}:{port}'].add(p)
    elif kind == 'P2pChannelEvent::Opened' and 'channel not supported' in text and pm:
        events.append((t, 'ready', pm.group(1)))
    elif kind == 'P2pNetworkSchedulerError':
        am = census.ADDR_RE.search(text)
        if am:
            events.append((t, 'end_addr', am.group(1)))
    elif kind == 'P2pDisconnectionInit' and pm:
        events.append((t, 'end', pm.group(1)))


def canon(p):
    s = seeds.peer(p)
    return 'seed:' + s if s else p


open_since = {}
intervals = []
for t, k, x in events:
    if k == 'ready':
        p = canon(x)
        if p in open_since and (t - open_since[p]).total_seconds() < 1:
            continue
        if p in open_since:
            intervals.append((p, open_since[p], t))
        open_since[p] = t
    elif k == 'end':
        p = canon(x)
        if p in open_since:
            intervals.append((p, open_since.pop(p), t))
    else:
        for q in addr_peers.get(x, ()):
            q = canon(q) if not q.startswith('seed:') else q
            if q in open_since:
                intervals.append((q, open_since.pop(q), t))
for p, s in open_since.items():
    intervals.append((p, s, last))

step = timedelta(minutes=bucket_min)
t0 = first.replace(minute=(first.minute // bucket_min) * bucket_min, second=0, microsecond=0)
print('| Window start (UTC) | Mean peers connected | of which seeds | Non-seed peers seen |')
print('| --- | ---: | ---: | ---: |')
while t0 < last:
    t1 = t0 + step
    lo, hi = max(t0, first), min(t1, last)
    span = (hi - lo).total_seconds()
    if span <= 0:
        t0 = t1
        continue
    tot = seed = 0.0
    nonseed = set()
    for p, s, e in intervals:
        ov = (min(e, hi) - max(s, lo)).total_seconds()
        if ov > 0:
            tot += ov
            if p.startswith('seed:'):
                seed += ov
            else:
                nonseed.add(p)
    print(f'| {t0:%H:%M} | {tot / span:.1f} | {seed / span:.1f} | {len(nonseed)} |')
    t0 = t1
long_lived = collections.Counter()
for p, s, e in intervals:
    long_lived[p] += (e - s).total_seconds()
print()
print('| Peer | Connected for (min) | Connections |')
print('| --- | ---: | ---: |')
cnt = collections.Counter(p for p, _, _ in intervals)
for p, secs in long_lived.most_common(12):
    print(f'| {p[:20]} | {secs / 60:.0f} | {cnt[p]} |')
