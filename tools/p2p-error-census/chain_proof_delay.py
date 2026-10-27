#!/usr/bin/env python3
"""How long new best tips waited for their chain proof in a mina-rust node log.

Most of the kinds it reads are logged at DEBUG since O1
(private implementation notes): give it the log of a node run with
`--verbosity debug`.

For each candidate block the node logged as received (`candidate block
received`), the time until it became the sync target (`TransitionFrontierSync
BestTipUpdate` or `Init` with that hash). A block becomes the target once it is
verified and has a chain proof: from our own best chain when it extends it,
otherwise from a peer's `get_best_tip` answer, which is what the delay mostly
measures during a bootstrap. Blocks that never became the target (siblings
that lost, blocks superseded first) are counted apart. Since I17 it also lists
the chain proof requests (`ChainProofPeerQueryInit`) per block, to show that
no peer was asked twice for one block. Standard library only.

Usage:
    python3 -I chain_proof_delay.py LOG
"""

import collections
import re
import statistics
import sys
from datetime import datetime

TS_RE = re.compile(r'(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z\s+(?:TRACE|DEBUG|INFO|WARN|ERROR)\s')
KIND_RE = re.compile(r'kind(?:: |=)"([A-Za-z0-9_:]+)"')
HASH = r'"?(3N[1-9A-HJ-NP-Za-km-z]{49})"?'
RECEIVED_RE = re.compile(r'block_hash(?:: |=)' + HASH)
HEIGHT_RE = re.compile(r'block_height(?:: |=)(\d+)')
TARGET_RE = re.compile(r'(?:new_best_tip_hash|block_hash)(?:: |=)' + HASH)
ASK_RE = re.compile(r'\bhash(?:: |=)' + HASH)
PEER_RE = re.compile(r'peer_id(?:: |=)"?([1-9A-HJ-NP-Za-km-z]{40,60})"?')


def parse_ts(s):
    return datetime.strptime(s[:26], '%Y-%m-%dT%H:%M:%S.%f')


def main(path):
    received = {}
    heights = {}
    target = {}
    asks = collections.defaultdict(list)
    with open(path, errors='replace') as f:
        for line in f:
            m = TS_RE.search(line)
            km = KIND_RE.search(line)
            if not m or not km:
                continue
            t = parse_ts(m.group(1))
            kind = km.group(1)
            if kind == 'TransitionFrontierCandidateBlockReceived':
                hm = RECEIVED_RE.search(line)
                if hm and hm.group(1) not in received:
                    received[hm.group(1)] = t
                    h = HEIGHT_RE.search(line)
                    heights[hm.group(1)] = int(h.group(1)) if h else None
            elif kind in ('TransitionFrontierSyncBestTipUpdate', 'TransitionFrontierSyncInit'):
                hm = TARGET_RE.search(line)
                if hm and hm.group(1) not in target:
                    target[hm.group(1)] = t
            elif kind == 'TransitionFrontierCandidateChainProofPeerQueryInit':
                hm = ASK_RE.search(line)
                pm = PEER_RE.search(line)
                if hm:
                    asks[hm.group(1)].append((t, pm.group(1) if pm else '?'))
    delays = []
    print('| Height | Block | Received | Target after (s) | Chain proof requests |')
    print('| ---: | --- | --- | ---: | ---: |')
    for h, t in sorted(received.items(), key=lambda kv: kv[1]):
        d = (target[h] - t).total_seconds() if h in target else None
        if d is not None and d >= 0:
            delays.append(d)
        cell = f'{d:.2f}' if d is not None else 'never'
        print(f'| {heights.get(h)} | {h[:12]}… | {t.time()} | {cell} | {len(asks.get(h, []))} |')
    if delays:
        print(f'\n{len(delays)} of {len(received)} received blocks became the sync target; '
              f'delay median {statistics.median(delays):.2f} s, max {max(delays):.2f} s.')
    repeated = {h: v for h, v in asks.items()
                if len({p for _, p in v}) != len(v)}
    total = sum(len(v) for v in asks.values())
    print(f'{total} chain proof requests for {len(asks)} blocks; '
          f'{len(repeated)} blocks with a peer asked twice.')
    return 0


if __name__ == '__main__':
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1]))
