#!/usr/bin/env python3
"""Seed connection lifetimes, redial gaps and downtimes from node logs.

Prototype for private implementation notes Standard library only.

Usage:
    python3 -I lifetimes.py --seeds SEEDS.txt --resolved HOSTS.txt LOG [LOG ...]

(see census.py for SEEDS.txt and HOSTS.txt).

For every mainnet seed, a connection starts at a ready burst (the
`channel not supported` warnings, matched to the seed by peer id) and ends at
the first fatal connection error on one of the seed's addresses, or at our own
disconnection of the seed's peer id. Per seed and log it reports:

  - lifetime: ready to end, median, over all ends and by how the connection
    ended;
  - connected: the share of the log's span with a connection open to the
    seed (one still open at the end counts up to the last line);
  - gap: ready to the next ready, median and mean;
  - downtime: end to the next ready. The node may redial a disconnected peer
    only `reconnect_timeout` (30 s) after the disconnection through
    `P2pConnectionOutgoingAction::Reconnect`; a shorter downtime means the
    redial took another path (an initial peer dialled with `Init` after it
    was removed from the peer table).

It also prints, per log, the rate of ready connections and of connection
ends over all peers, which drives how fast disconnected peers leave the peer
table (`P2pDisconnectionAction::Finish` removes the oldest one).
"""

import argparse
import collections
import os
import statistics
import sys

sys.dont_write_bytecode = True  # no __pycache__ next to the scripts
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import census  # noqa: E402

RECONNECT_TIMEOUT_S = 30.0


def med(xs):
    return statistics.median(xs) if xs else float('nan')


def analyse(path, seeds):
    ready_last = {}
    open_conn = {}  # seed -> ready time
    last_end = {}  # seed -> end time
    last_ready = {}  # seed -> previous ready time
    lifetimes = collections.defaultdict(lambda: collections.defaultdict(list))
    gaps = collections.defaultdict(list)
    downtimes = collections.defaultdict(list)
    conns = collections.Counter()
    ready_all = 0
    ends_all = 0
    first = last = None
    for t, level, text in census.read_records(path):
        first = first or t
        last = t
        km = census.KIND_RE.search(text)
        kind = km.group(1) if km else None
        seed = None
        end_kind = None
        if kind == 'P2pChannelEvent::Opened' and 'channel not supported' in text:
            pm = census.PEER_RE.search(text)
            if not pm:
                continue
            p = pm.group(1)
            new = p not in ready_last or (t - ready_last[p]).total_seconds() > 1
            ready_last[p] = t
            if not new:
                continue
            ready_all += 1
            seed = seeds.peer(p)
            if not seed:
                continue
            if seed in open_conn:
                lifetimes[seed]['superseded by a new ready'].append(
                    (t - open_conn[seed]).total_seconds())
            if seed in last_ready:
                gaps[seed].append((t - last_ready[seed]).total_seconds())
            if seed in last_end and seed not in open_conn:
                downtimes[seed].append((t - last_end[seed]).total_seconds())
            open_conn[seed] = t
            last_ready[seed] = t
            conns[seed] += 1
            continue
        if kind == 'P2pNetworkSchedulerError':
            ends_all += 1
            am = census.ADDR_RE.search(text)
            em = census.ERR_RE.search(text)
            seed = seeds.addr(am.group(1)) if am else None
            end_kind = census.fatal_class(em.group(1) if em else '?')
        elif kind == 'P2pDisconnectionInit':
            ends_all += 1
            pm = census.PEER_RE.search(text)
            seed = seeds.peer(pm.group(1)) if pm else None
            end_kind = 'we disconnected'
        if seed and seed in open_conn:
            lifetimes[seed][end_kind].append((t - open_conn.pop(seed)).total_seconds())
            last_end[seed] = t
    return first, last, ready_all, ends_all, conns, lifetimes, gaps, downtimes, open_conn


def report(path, seeds):
    (first, last, ready_all, ends_all, conns, lifetimes, gaps, downtimes,
     open_conn) = analyse(path, seeds)
    minutes = max((last - first).total_seconds() / 60, 1e-9)
    out = [f'### {os.path.basename(path)}\n',
           f'{first:%H:%M}-{last:%H:%M} UTC; all peers: {ready_all / minutes:.1f} ready '
           f'connections and {ends_all / minutes:.1f} connection ends '
           f'(fatal errors plus our disconnections) per minute.\n',
           '| Seed | Connections | Lifetime median s, all ends '
           '| Lifetime median s (how it ended, count) '
           '| Connected, share of the log | Gap ready to ready, median / mean s '
           '| Downtime median s | Downtimes under 30 s |',
           '| --- | ---: | ---: | --- | ---: | ---: | ---: | ---: |']
    for seed, n in sorted(conns.items(), key=lambda kv: -kv[1]):
        ends = sorted(lifetimes[seed].items(), key=lambda kv: -len(kv[1]))
        life = '; '.join(f'{med(v):.1f} ({k}, {len(v)})' for k, v in ends[:2])
        lives = [x for _, v in ends for x in v]
        # A connection still open at the end of the log counts up to its end.
        connected = sum(lives)
        if seed in open_conn:
            connected += (last - open_conn[seed]).total_seconds()
        share = connected / (minutes * 60)
        g = gaps[seed]
        d = downtimes[seed]
        short = sum(1 for x in d if x < RECONNECT_TIMEOUT_S)
        out.append(f'| {seed} | {n} | {med(lives):.1f} | {life} | {share:.0%} | '
                   f'{med(g):.1f} / '
                   f'{(statistics.mean(g) if g else float("nan")):.1f} | {med(d):.1f} | '
                   f'{short} of {len(d)} |')
    return '\n'.join(out) + '\n'


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--seeds', required=True, help='mainnet seed list (multiaddrs)')
    ap.add_argument('--resolved', required=True,
                    help='file of host=ip,ip lines for the seed hosts')
    ap.add_argument('logs', nargs='+')
    args = ap.parse_args()
    resolved = []
    for r in open(args.resolved).read().split():
        host, _, ips = r.partition('=')
        resolved.append((host, [i for i in ips.split(',') if i]))
    seeds = census.SeedMap(args.seeds, resolved)
    for path in args.logs:
        sys.stdout.write(report(path, seeds) + '\n')


if __name__ == '__main__':
    main()
