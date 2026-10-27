#!/usr/bin/env python3
"""Which best tips did a running node re-encode and gossip itself?

Most of the kinds it reads are logged at DEBUG since O1
(private implementation notes): give it the log of a node run with
`--verbosity debug`.

Prototype for private implementation notes Standard library only. Reads
the node's HTTP action statistics, so it only works against a live node, and
only reads.

Usage:
    python3 -I rebroadcasts.py PORT [DELAY_S] [--tips FILE] [--log FILE]

`--tips FILE` also writes one line per best tip, tab-separated: interval id,
time (ns), height, state hash, verdict, and the three counts behind it, and
with `--log`, the log's verdict and the block's sources.

`--log FILE` reads the node's log for the line `synced_effects` writes, since
I14, for each best tip that didn't come as libp2p gossip
(`TransitionFrontierSyncedBestTipGossip`, with `reencoded`, `from_libp2p` and
`from_webrtc`), and gives each best tip a certain verdict: "original gossip"
(no such line: the original message was forwarded), "not gossiped" or
"re-encoded". It counts re-encoded blocks that a libp2p peer had sent us too,
which I14 should make zero. Older nodes write no such line, so every best tip
would read "original gossip": use it only with nodes built since I14.

When a block becomes the best tip, `synced_effects`
(crates/node/src/transition_frontier/transition_frontier_effects.rs) asks
pubsub to forward the libp2p gossip message it came in (original bytes,
`BroadcastValidatedMessage`). If no such message is in the message cache, it
dispatches `WebRtcRebroadcast` instead, which binprot-encodes the block again,
signs it with a key derived from the bytes, and gossips it to libp2p peers.

`GET /stats/actions?id=N` returns the actions dispatched between best tip N
and best tip N+1, and `synced_effects` runs right after the interval starts,
so an interval with one `TransitionFrontierSynced` and more
`P2pNetworkPubsubWebRtcRebroadcast` than snark and transaction rebroadcast
candidates (`P2pChannels{Snark,Transaction}Libp2pBroadcast`) certainly
re-encoded its best tip. Intervals with one rebroadcast and some snark
broadcasts are counted as ambiguous. An interval without a rebroadcast did not
re-encode its best tip: it either forwarded the original gossip message, or,
since I14 (private implementation notes), gossiped nothing
because the block came from a libp2p peer by RPC.
"""

import collections
import datetime
import json
import re
import sys
import time
import urllib.request

KEYS = (
    'TransitionFrontierSynced',
    'P2pNetworkPubsubWebRtcRebroadcast',
    'P2pChannelsSnarkLibp2pBroadcast',
    'P2pChannelsTransactionLibp2pBroadcast',
    'P2pChannelsBestTipReceived',
)


def get(port, query=''):
    url = f'http://localhost:{port}/stats/actions{query}'
    with urllib.request.urlopen(url, timeout=30) as r:
        return json.load(r)


def calls(stats, key):
    return sum(v['total_calls'] for v in stats.get(key, {}).values())


LOG_KIND = 'kind: "TransitionFrontierSyncedBestTipGossip"'
LOG_FIELD = re.compile(r'(\w+): (?:"([^"]*)"|(\w+))')


def read_log(path):
    """State hash -> the fields of its `TransitionFrontierSyncedBestTipGossip` line."""
    lines = {}
    with open(path, errors='replace') as f:
        for line in f:
            if LOG_KIND not in line:
                continue
            fields = {m[0]: m[1] or m[2] for m in LOG_FIELD.findall(line)}
            if 'state_hash' in fields:
                lines[fields['state_hash']] = fields
    return lines


def log_verdict(lines, state_hash):
    fields = lines.get(state_hash)
    if fields is None:
        return 'original gossip', '', ''
    verdict = 're-encoded' if fields.get('reencoded') == 'true' else 'not gossiped'
    return verdict, fields.get('from_libp2p', ''), fields.get('from_webrtc', '')


def take_option(args, name):
    if name not in args:
        return None
    i = args.index(name)
    if i + 1 >= len(args):
        sys.exit(f'{name} needs a file')
    value = args[i + 1]
    del args[i:i + 2]
    return value


def main():
    args = sys.argv[1:]
    tips = take_option(args, '--tips')
    tips = open(tips, 'w') if tips else None
    log = take_option(args, '--log')
    log = read_log(log) if log else None
    if not args:
        sys.exit('usage: rebroadcasts.py PORT [DELAY_S] [--tips FILE] [--log FILE]')
    port = int(args[0])
    delay = float(args[1]) if len(args) > 1 else 0.1
    since_start = get(port)['stats']
    print('since start:', {k: calls(since_start, k) for k in KEYS})
    last = get(port, '?id=latest')['id']
    verdicts = collections.Counter()
    log_verdicts = collections.Counter()
    per_hour = collections.defaultdict(collections.Counter)
    for i in range(last + 1):
        d = get(port, f'?id={i}')
        time.sleep(delay)
        if not d or not calls(d['stats'], 'TransitionFrontierSynced'):
            continue
        st = d['stats']
        w = calls(st, 'P2pNetworkPubsubWebRtcRebroadcast')
        s = (calls(st, 'P2pChannelsSnarkLibp2pBroadcast')
             + calls(st, 'P2pChannelsTransactionLibp2pBroadcast'))
        v = ('not re-encoded' if w == 0 else
             're-encoded' if w > s else 'ambiguous')
        verdicts[v] += 1
        from_log = ()
        if log is not None:
            from_log = log_verdict(log, d.get('block_hash'))
            lv, from_libp2p, from_webrtc = from_log
            log_verdicts[lv] += 1
            if lv == 're-encoded' and from_libp2p == 'true':
                log_verdicts['re-encoded, from a libp2p peer too'] += 1
        if tips:
            synced = calls(st, 'TransitionFrontierSynced')
            print(i, d['time'], d.get('block_level'), d.get('block_hash'), v,
                  synced, w, s, *from_log, sep='\t', file=tips)
        hour = datetime.datetime.fromtimestamp(d['time'] / 1e9, datetime.timezone.utc)
        per_hour[hour.strftime('%H')][v] += 1
    print('best tips:', dict(verdicts))
    if log is not None:
        log_verdicts.setdefault('re-encoded, from a libp2p peer too', 0)
        print('best tips, from the log:', dict(log_verdicts))
    for h, c in sorted(per_hour.items()):
        print(f'  {h}h', dict(c))


if __name__ == '__main__':
    main()
