#!/usr/bin/env python3
"""How often a mina-rust node asked each libp2p peer for its best tip.

Most of the kinds it reads are logged at DEBUG since O1
(private implementation notes): give it the log of a node run with
`--verbosity debug`.

Reads the `P2pBestTipRequestSent` lines that a node logs (since I17) for every
`get_best_tip` request to a libp2p peer, and reports, per peer id and per IP,
the most requests in any 60 s and in any 5 minutes, against OCaml's limit of 3
a minute (15 per 5 minutes). It also counts each peer's connections that
reached the ready state, as census.py does, to show the rate across
reconnections. Standard library only.

Usage:
    python3 -I best_tip_rate.py LOG
"""

import collections
import re
import sys
from datetime import datetime

TS_RE = re.compile(r'(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z\s+(?:TRACE|DEBUG|INFO|WARN|ERROR)\s')
PEER_RE = re.compile(r'peer_id(?:: |=)"?([1-9A-HJ-NP-Za-km-z]{40,60})"?')
KIND_RE = re.compile(r'kind(?:: |=)"([A-Za-z0-9_:]+)"')
IP_RE = re.compile(r'\bip(?:: |=)Some\(([0-9a-fA-F.:]+)\)')


def parse_ts(s):
    return datetime.strptime(s[:26], '%Y-%m-%dT%H:%M:%S.%f')


def max_in_window(times, seconds):
    """Most entries of sorted `times` within any half-open window."""
    best = start = 0
    for end, t in enumerate(times):
        while (t - times[start]).total_seconds() >= seconds:
            start += 1
        best = max(best, end - start + 1)
    return best


def main(path):
    by_peer = collections.defaultdict(list)
    by_ip = collections.defaultdict(list)
    ready = collections.Counter()
    ready_last = {}
    first = last = None
    with open(path, errors='replace') as f:
        for line in f:
            m = TS_RE.search(line)
            if not m:
                continue
            t = parse_ts(m.group(1))
            first = first or t
            last = t
            km = KIND_RE.search(line)
            kind = km.group(1) if km else None
            if kind == 'P2pBestTipRequestSent':
                pm = PEER_RE.search(line)
                im = IP_RE.search(line)
                if pm:
                    by_peer[pm.group(1)].append(t)
                if im:
                    by_ip[im.group(1)].append(t)
            elif kind == 'P2pChannelEvent::Opened' and 'channel not supported' in line:
                pm = PEER_RE.search(line)
                if pm:
                    p = pm.group(1)
                    if p not in ready_last or (t - ready_last[p]).total_seconds() > 1:
                        ready[p] += 1
                    ready_last[p] = t
    if first is None:
        print('no timestamped lines')
        return 1
    minutes = max((last - first).total_seconds() / 60, 1e-9)
    total = sum(len(v) for v in by_peer.values())
    print(f'{path}: {first} to {last} UTC ({minutes:.1f} min), '
          f'{total} best tip requests to {len(by_peer)} peers at {len(by_ip)} IPs\n')
    for title, table, label in (('Per peer id', by_peer, lambda k: k[:16] + '…'),
                                ('Per IP', by_ip, lambda k: k)):
        print(f'{title}:\n')
        print('| Peer | Requests | Ready connections | Most in 60 s | Most in 5 min |')
        print('| --- | ---: | ---: | ---: | ---: |')
        rows = sorted(table.items(), key=lambda kv: -len(kv[1]))
        for k, times in rows:
            times.sort()
            conns = ready.get(k, '') if table is by_peer else ''
            print(f'| {label(k)} | {len(times)} | {conns} | '
                  f'{max_in_window(times, 60)} | {max_in_window(times, 300)} |')
        worst60 = max((max_in_window(sorted(v), 60) for v in table.values()), default=0)
        worst300 = max((max_in_window(sorted(v), 300) for v in table.values()), default=0)
        print(f'\nWorst: {worst60} in 60 s, {worst300} in 5 minutes '
              '(OCaml allows 3 and 15).\n')
    return 0


if __name__ == '__main__':
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1]))
