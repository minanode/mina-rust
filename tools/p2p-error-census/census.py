#!/usr/bin/env python3
"""Count and classify libp2p connection errors in mina-rust node logs.

Most of the kinds it reads are logged at DEBUG since O1
(private implementation notes): give it the log of a node run with
`--verbosity debug`.

Prototype for private implementation notes Standard library only.

Usage:
    python3 -I census.py LOG [LOG ...]            # one Markdown report per log
    python3 -I census.py --seeds SEEDS.txt --resolved HOSTS.txt LOG ...

The logs are the stdout of `mina node` at the default (INFO) level, in either of
the two formats seen so far (`key: value` or `key=value`). Lines without a
timestamp continue the previous line (some error strings contain newlines).
A record may start mid-line when another thread's stdout was interleaved (for
example `verify(2026-...Z  WARN ...`); the text before the timestamp is
dropped.

What is counted, per log:
  - fatal connection errors (`P2pNetworkSchedulerError`), by error class;
  - streams closed without closing their connection
    (`P2pNetworkSchedulerStreamReset`, since I16), by cause, protocol and
    which side opened the stream;
  - multistream-select errors (`P2pNetworkSchedulerSelectError`), by what was
    being negotiated and the error;
  - disconnections we initiated (`P2pDisconnectionInit`), by reason class;
  - ERROR-level lines, by message class;
  - WARN-level sync errors about a peer query (ledger, staged ledger parts and
    block queries that failed, blocks rejected at prevalidation), by kind;
  - failures we keep the peer through (since I17): error answers to our
    requests (`P2pNetworkRpcErrorResponse`, by RPC and error), first
    timeouts of a request (`P2pChannelsRpcTimeout`, by kind), queries we
    answered `Unimplemented_rpc` (`P2pNetworkRpcUnimplementedQuery`, by RPC),
    and menu queries we answered (`P2pNetworkRpcMenuAnswered`);
  - libp2p connections that reached the ready state, from the
    `P2pChannelEvent::Opened ... channel not supported` warnings that every
    ready libp2p connection emits (one burst per connection);
  - since I18, our libp2p dials (`P2pDial`), per peer and whether each was a
    redial, the backoffs that followed (`P2pDialBackoff`), by how the dial or
    connection ended, and stale peer ids given up (`P2pStalePeerId`);
  - since I18b, first dials to peers never dialled before, Kademlia lookups
    started (`P2pNetworkKademliaStartBootstrap`) and finished
    (`P2pKadLookupFinished`), and failing peers forgotten to make room for
    discovered ones (`P2pPeerEvicted`).

Seed attribution uses the mainnet seed list (`--seeds`, the file served at
https://storage.googleapis.com/seed-lists/mainnet_seeds.txt), with host names
resolved by the caller into a `--resolved` file of `host=ip,ip` lines, because
several o1Labs seeds share a pool of node IPs and differ only by port.
"""

import argparse
import collections
import re
import sys
from datetime import datetime

TS_RE = re.compile(r'(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z\s+'
                   r'(TRACE|DEBUG|INFO|WARN|ERROR)\s+(.*)$')
KIND_RE = re.compile(r'kind(?:: |=)"([A-Za-z0-9_:]+)"')
ADDR_RE = re.compile(r'addr(?:: |=)(\d+\.\d+\.\d+\.\d+:\d+) \(incoming: (true|false)\)')
PEER_RE = re.compile(r'peer_id(?:: |=)"?([1-9A-HJ-NP-Za-km-z]{40,60})"?')
ERR_RE = re.compile(r'\berror(?:: |=)(.*)$', re.S)
REASON_RE = re.compile(r'\breason(?:: |=)(.*)$', re.S)
SELECT_KIND_RE = re.compile(r'select_kind(?:: |=)(Stream\(PeerId\(\w+\), (\d+)\)|MultiplexingNoPeerId|Multiplexing\(\w+\)|Authentication)')
LIBP2P_ID_RE = re.compile(r'12D3KooW[1-9A-HJ-NP-Za-km-z]+')
STREAM_ID_RE = re.compile(r'\bstream_id(?:: |=)(\d+)')
PROTOCOL_RE = re.compile(r'\bprotocol(?:: |=)(None|Some\((\w+)\(\w+\)\))')

SYNC_WARN_KINDS = (
    'TransitionFrontierSyncLedgerSnarkedPeerQueryAddressError',
    'TransitionFrontierSyncLedgerSnarkedPeerQueryNumAccountsError',
    'TransitionFrontierSyncLedgerStagedPartsPeerFetchError',
    'TransitionFrontierSyncBlocksPeerQueryError',
    'TransitionFrontierCandidateBlockPrevalidateError',
    'P2pRpcBestTipHashMismatch',
)

# What no longer closes the connection (I17), by log kind.
KEPT_KINDS = {
    'P2pNetworkRpcErrorResponse': 'error answer',
    'P2pChannelsRpcTimeout': 'first timeout',
    'P2pNetworkRpcUnimplementedQuery': 'answered unimplemented',
    'P2pNetworkRpcMenuAnswered': 'answered',
}
SUMMARY_RE = re.compile(r'summary(?:: |=)"?([^"\n]*?)"?(?:,? \w+(?:: |=)|$)')

B58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'


def b58decode(s):
    n = 0
    for c in s:
        n = n * 58 + B58.index(c)
    raw = n.to_bytes((n.bit_length() + 7) // 8, 'big') if n else b''
    pad = len(s) - len(s.lstrip('1'))
    return b'\x00' * pad + raw


def key_of_libp2p_id(s):
    """32-byte ed25519 key inside a `12D3KooW...` identity multihash."""
    b = b58decode(s)
    return b[-32:] if len(b) == 38 else None


def key_of_mina_peer_id(s):
    """32-byte key inside a mina-rust `2...` base58check peer id."""
    b = b58decode(s)
    return b[1:33] if len(b) == 37 else None


def parse_ts(s):
    return datetime.strptime(s[:26], '%Y-%m-%dT%H:%M:%S.%f')


def read_records(path):
    """Yield (timestamp, level, text) with continuation lines joined."""
    cur = None
    with open(path, 'rb') as f:
        for raw in f:
            line = raw.decode('utf-8', 'replace').rstrip('\n')
            line = re.sub(r'\x1b\[[0-9;]*m', '', line)
            m = TS_RE.search(line)
            if m:
                if cur:
                    yield cur
                cur = [parse_ts(m.group(1)), m.group(2), m.group(3)]
            elif cur:
                cur[2] += '\n' + line
    if cur:
        yield cur


def fatal_class(err):
    e = err.strip()
    if "remote peer id doesn't match" in e:
        return 'noise: remote peer id does not match expected'
    if 'first MAC mismatch' in e:
        return 'noise: first MAC mismatch'
    if e.startswith('noise handshake error'):
        return 'noise: other'
    if e.startswith('peer reset yamux stream'):
        return 'yamux: peer reset a stream (RST)'
    if e.startswith('select protocol error'):
        return 'select: a negotiation failed'
    if e.startswith('remote peer closed connection'):
        return 'remote peer closed the connection'
    if 'GET_PROVIDERS' in e:
        return 'kad incoming: GET_PROVIDERS unsupported'
    if 'message is too long' in e and 'kademlia' in e:
        return 'kad incoming: request over our size limit'
    if e.startswith('mio error'):
        return 'socket (mio) error'
    return 'other: ' + e.split('\n')[0][:60]


def select_class(kind_text, stream_id, err):
    e = err.strip().split('\n')[0]
    if e.startswith('unknown protocol'):
        e = 'unknown protocol (bytes that are not a multistream token)'
    if stream_id is not None:
        sid = int(stream_id)
        side = 'peer-opened' if sid % 2 == 0 else 'we opened'
        return f'stream {sid} ({side}): {e}'
    return f'{kind_text}: {e}'


def stream_reset_class(text):
    """Cause, protocol and opener of a stream closed on its own."""
    rm = REASON_RE.search(text)
    reason = rm.group(1).strip().split('\n')[0] if rm else '?'
    if reason.startswith('peer reset'):
        cause = 'peer reset'
    elif reason.startswith('protocol negotiation failed'):
        detail = reason.split(':', 1)[1].strip()
        if detail.startswith('unknown protocol'):
            detail = 'unknown protocol'
        cause = f'negotiation failed ({detail[:40]})'
    else:
        cause = reason[:50]
    pm = PROTOCOL_RE.search(text)
    protocol = (pm.group(2) or 'not negotiated') if pm else '?'
    if protocol == 'not negotiated' or pm is None:
        protocol = 'not negotiated'
    sm = STREAM_ID_RE.search(text)
    am = ADDR_RE.search(text)
    side = '?'
    if sm and am:
        incoming = am.group(2) == 'true'
        side = 'peer-opened' if (int(sm.group(1)) % 2 == 1) == incoming else 'we opened'
    return f'{cause}: {protocol}, {side}'


def disconnect_class(reason):
    r = reason.strip().split('\n')[0]
    r = re.sub(r'^failed to receive message from channel: ', '', r)
    r = re.sub(r'\d{3,}', 'N', r)
    return r[:110]


def error_line_class(text):
    t = re.sub(r'time(?:: |=)"?\d+"?,? ?', '', text)
    if 'unimplemented stream kind' in t:
        m = re.search(r'unimplemented stream kind ([A-Za-z0-9_()]+)', t)
        return 'select: negotiated but unhandled stream ' + (m.group(1) if m else '?')
    if 'Identify stream already exists' in t:
        return 'identify: second identify stream for a connection'
    if 'TryFromIntError' in t:
        return 'pubsub: gossip message failed to decode (TryFromIntError)'
    if 'kademlia incoming stream state' in t:
        return 'kad: incoming stream in wrong state'
    if 'disconnected peer connection for address' in t:
        return 'scheduler: connection closed for an already disconnected peer'
    if 'http_request' in t or 'tower_http' in t:
        return 'http: request failed (not p2p)'
    return 'other: ' + t.split('\n')[0][:80]


class SeedMap:
    def __init__(self, seeds_path, resolved):
        self.by_addr = {}
        self.by_key = {}
        if not seeds_path:
            return
        hosts = dict(resolved)
        for line in open(seeds_path):
            line = line.strip()
            m = re.match(r'/dns4/([^/]+)/tcp/(\d+)/p2p/(\w+)', line)
            if not m:
                continue
            host, port, pid = m.groups()
            name = host.split('.')[0] if 'o1test' in host else host
            for ip in hosts.get(host, []):
                self.by_addr[f'{ip}:{port}'] = name
            k = key_of_libp2p_id(pid)
            if k:
                self.by_key[k] = name

    def addr(self, a):
        return self.by_addr.get(a)

    def peer(self, mina_peer_id):
        k = key_of_mina_peer_id(mina_peer_id)
        return self.by_key.get(k) if k else None


def table(title, counter, hours, extra=None):
    out = [f'\n#### {title}\n', '| Count | Per hour | Kind |', '| ---: | ---: | --- |']
    for k, v in counter.most_common():
        out.append(f'| {v} | {v / hours:.1f} | {k} |')
    if not counter:
        out.append('| 0 | 0 | (none) |')
    return '\n'.join(out)


def analyse(path, seeds):
    first = last = None
    fatal = collections.Counter()
    fatal_by_addr = collections.defaultdict(collections.Counter)
    select = collections.Counter()
    select_by_addr = collections.defaultdict(collections.Counter)
    stream_reset = collections.Counter()
    disc = collections.Counter()
    disc_by_peer = collections.defaultdict(collections.Counter)
    errors = collections.Counter()
    sync_warn = collections.Counter()
    kept = collections.Counter()
    ready_last = {}
    ready = collections.Counter()
    mismatch_pairs = collections.Counter()
    incoming_fatal = 0
    dials = collections.Counter()
    redials = collections.Counter()
    backoff_ends = collections.Counter()
    backoff_max_failures = collections.Counter()
    stale_ids = collections.Counter()
    lookups_started = lookups_finished = evicted = 0
    for t, level, text in read_records(path):
        first = first or t
        last = t
        km = KIND_RE.search(text)
        kind = km.group(1) if km else None
        if kind == 'P2pNetworkSchedulerError':
            am = ADDR_RE.search(text)
            em = ERR_RE.search(text)
            c = fatal_class(em.group(1) if em else '?')
            fatal[c] += 1
            if am:
                fatal_by_addr[c][am.group(1)] += 1
                if am.group(2) == 'true':
                    incoming_fatal += 1
            if c.startswith('noise: remote peer id'):
                ids = LIBP2P_ID_RE.findall(text)
                if am and len(ids) >= 2:
                    mismatch_pairs[(am.group(1), ids[0], ids[1])] += 1
        elif kind == 'P2pNetworkSchedulerSelectError':
            am = ADDR_RE.search(text)
            sm = SELECT_KIND_RE.search(text)
            em = ERR_RE.search(text)
            c = select_class(sm.group(1) if sm else '?', sm.group(2) if sm else None,
                             em.group(1) if em else '?')
            select[c] += 1
            if am:
                select_by_addr[c][am.group(1)] += 1
        elif kind == 'P2pNetworkSchedulerStreamReset':
            stream_reset[stream_reset_class(text)] += 1
        elif kind == 'P2pDisconnectionInit':
            rm = REASON_RE.search(text)
            pm = PEER_RE.search(text)
            c = disconnect_class(rm.group(1) if rm else '?')
            disc[c] += 1
            if pm:
                disc_by_peer[c][pm.group(1)] += 1
        elif kind == 'P2pChannelEvent::Opened' and 'channel not supported' in text:
            pm = PEER_RE.search(text)
            if pm:
                p = pm.group(1)
                if p not in ready_last or (t - ready_last[p]).total_seconds() > 1:
                    ready[p] += 1
                ready_last[p] = t
        elif kind == 'P2pDial':
            pm = PEER_RE.search(text)
            if pm:
                dials[pm.group(1)] += 1
                if re.search(r'\bredial(?:: |=)true', text):
                    redials[pm.group(1)] += 1
        elif kind == 'P2pDialBackoff':
            pm = PEER_RE.search(text)
            em = re.search(r'\bend(?:: |=)(\w+)', text)
            fm = re.search(r'\bfailures(?:: |=)(\d+)', text)
            backoff_ends[em.group(1) if em else '?'] += 1
            if pm and fm:
                p = pm.group(1)
                backoff_max_failures[p] = max(backoff_max_failures[p], int(fm.group(1)))
        elif kind == 'P2pNetworkKademliaStartBootstrap':
            lookups_started += 1
        elif kind == 'P2pKadLookupFinished':
            lookups_finished += 1
        elif kind == 'P2pPeerEvicted':
            evicted += 1
        elif kind == 'P2pStalePeerId':
            am = re.search(r'\baddr(?:: |=)(\d+\.\d+\.\d+\.\d+:\d+)', text)
            pm = PEER_RE.search(text)
            stale_ids[(am.group(1) if am else '?', pm.group(1) if pm else '?')] += 1
        elif kind in KEPT_KINDS:
            sm = SUMMARY_RE.search(text)
            em = ERR_RE.search(text)
            what = sm.group(1) if sm else '?'
            if em:
                # The error names the RPC.
                err = em.group(1).strip().split('\n')[0].strip('"')
                what = re.sub(r'\d{3,}', 'N', err)[:80]
            kept[f'{KEPT_KINDS[kind]}, {what}'] += 1
        elif level == 'WARN' and kind in SYNC_WARN_KINDS:
            em = re.search(r'\berror(?:: |=)([A-Za-z]+)', text)
            sync_warn[kind + (f' ({em.group(1)})' if em else '')] += 1
        if level == 'ERROR':
            errors[error_line_class(text)] += 1
    hours = max((last - first).total_seconds() / 3600, 1e-9) if first else 1
    out = [f'### {path.split("/")[-1]}\n',
           f'From {first} to {last} UTC ({hours:.2f} h).']
    out.append(table('Fatal connection errors (`P2pNetworkSchedulerError`)', fatal, hours))
    out.append(f'\nOf these, {incoming_fatal} were on connections the peer opened.')
    out.append(table('Multistream-select errors (`P2pNetworkSchedulerSelectError`)', select, hours))
    out.append(table('Streams closed, connection kept (`P2pNetworkSchedulerStreamReset`)',
                     stream_reset, hours))
    out.append(table('Disconnections we initiated (`P2pDisconnectionInit`)', disc, hours))
    out.append(table('ERROR lines', errors, hours))
    out.append(table('WARN: failed peer queries during sync, rejected blocks', sync_warn, hours))
    out.append(table('What we kept the peer through (I17)', kept, hours))

    def label_addr(a):
        s = seeds.addr(a)
        return f'{a} ({s})' if s else a

    out.append('\n#### Top addresses per fatal error class\n')
    out.append('| Class | Addresses | Top addresses (count) | Share on seeds |')
    out.append('| --- | ---: | --- | ---: |')
    for c, n in fatal.most_common():
        by = fatal_by_addr[c]
        top = ', '.join(f'{label_addr(a)} {v}' for a, v in by.most_common(4))
        on_seeds = sum(v for a, v in by.items() if seeds.addr(a))
        out.append(f'| {c} | {len(by)} | {top} | {on_seeds / n:.0%} |')
    out.append('\n#### Top addresses per select error class\n')
    out.append('| Class | Addresses | Top addresses (count) | Share on seeds |')
    out.append('| --- | ---: | --- | ---: |')
    for c, n in select.most_common():
        by = select_by_addr[c]
        top = ', '.join(f'{label_addr(a)} {v}' for a, v in by.most_common(4))
        on_seeds = sum(v for a, v in by.items() if seeds.addr(a))
        out.append(f'| {c} | {len(by)} | {top} | {on_seeds / n:.0%} |')
    if mismatch_pairs:
        out.append('\n#### Peer id mismatches (address, id we dialled, id that answered)\n')
        out.append('| Count | Address | Expected | Got |')
        out.append('| ---: | --- | --- | --- |')
        for (a, e, g), v in mismatch_pairs.most_common(8):
            out.append(f'| {v} | {a} | {e[:16]}… | {g[:16]}… |')
    total_ready = sum(ready.values())
    on_seed_ready = sum(v for p, v in ready.items() if seeds.peer(p))
    out.append(f'\n#### Connections that reached ready: {total_ready} '
               f'({total_ready / hours:.0f} per hour) with {len(ready)} peers; '
               f'{on_seed_ready} of them to seeds\n')
    out.append('| Connections | Per hour | Peer | Seed |')
    out.append('| ---: | ---: | --- | --- |')
    for p, v in ready.most_common(10):
        out.append(f'| {v} | {v / hours:.1f} | {p[:14]}… | {seeds.peer(p) or ""} |')
    total_dials = sum(dials.values())
    if total_dials or backoff_ends or stale_ids:
        total_redials = sum(redials.values())
        on_seeds = sum(v for p, v in dials.items() if seeds.peer(p))
        seed_redials = sum(v for p, v in redials.items() if seeds.peer(p))
        out.append(f'\n#### Our libp2p dials (I18): {total_dials} '
                   f'({total_dials / hours:.0f} per hour) to {len(dials)} peers; '
                   f'{total_dials - total_redials} first dials, '
                   f'{total_redials} redials, {on_seeds} to seeds '
                   f'({seed_redials} seed redials); at most '
                   f'{max(redials.values(), default=0)} redials of one peer\n')
        out.append(f'Kademlia lookups (I18b): {lookups_started} started, '
                   f'{lookups_finished} finished; {evicted} failing peers '
                   f'forgotten for discovered ones.\n')
        out.append('| Dials | Redials | Per hour | Most failures in a row | Peer | Seed |')
        out.append('| ---: | ---: | ---: | ---: | --- | --- |')
        for p, v in dials.most_common(10):
            out.append(f'| {v} | {redials[p]} | {v / hours:.1f} | '
                       f'{backoff_max_failures.get(p, 0)} | {p[:14]}… | {seeds.peer(p) or ""} |')
        out.append(table('Backoffs, by how the dial or connection ended (`P2pDialBackoff`)',
                         backoff_ends, hours))
        out.append('\n#### Stale peer ids given up (`P2pStalePeerId`)\n')
        out.append('| Count | Address | Peer id |')
        out.append('| ---: | --- | --- |')
        for (a, p), v in stale_ids.most_common(8):
            out.append(f'| {v} | {a} | {p[:16]}… |')
        if not stale_ids:
            out.append('| 0 | (none) | |')
    out.append('\n#### Disconnections we initiated, top peers per reason\n')
    out.append('| Reason | Top peers (count) |')
    out.append('| --- | --- |')
    for c, _ in disc.most_common():
        top = ', '.join(f'{p[:10]}… {seeds.peer(p) or ""} {v}'
                        for p, v in disc_by_peer[c].most_common(4))
        out.append(f'| {c} | {top} |')
    counts = {'hours': hours, 'ready': total_ready, 'ready on seeds': on_seed_ready,
              'dials': total_dials, 'redials': sum(redials.values()),
              'first dials': total_dials - sum(redials.values()),
              'seed redials': sum(v for p, v in redials.items() if seeds.peer(p)),
              'most redials of one peer': max(redials.values(), default=0),
              'kad lookups started': lookups_started,
              'kad lookups finished': lookups_finished,
              'peers evicted': evicted,
              'stale peer ids': sum(stale_ids.values())}
    for name, counter in (('fatal', fatal), ('select', select),
                          ('stream only', stream_reset), ('ours', disc),
                          ('ERROR', errors), ('WARN sync', sync_warn), ('kept', kept),
                          ('backoff', backoff_ends)):
        for k, v in counter.items():
            counts[f'{name}: {k}'] = v
    return '\n'.join(out) + '\n', counts


def summary(results):
    """One table, a row per counted kind and a column per log."""
    names = [r[0].split('/')[-1].replace('native-', '').replace('.log', '')
             for r in results]
    keys = []
    for _, counts in results:
        for k in counts:
            if k not in keys:
                keys.append(k)
    out = ['| Kind | ' + ' | '.join(names) + ' | Total |',
           '| --- | ' + ' | '.join('---:' for _ in names) + ' | ---: |']
    for k in keys:
        vals = [c.get(k, 0) for _, c in results]
        if k == 'hours':
            cells = [f'{v:.2f}' for v in vals]
            total = f'{sum(vals):.2f}'
        else:
            cells = [str(v) for v in vals]
            total = str(sum(vals))
        out.append(f'| {k} | ' + ' | '.join(cells) + f' | {total} |')
    return '\n'.join(out) + '\n'


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--seeds', help='mainnet seed list (multiaddrs)')
    ap.add_argument('--resolved', help='file of host=ip,ip lines for the seed hosts')
    ap.add_argument('--summary', action='store_true',
                    help='print one cross-log table instead of per-log reports')
    ap.add_argument('logs', nargs='+')
    args = ap.parse_args()
    resolved = []
    for r in open(args.resolved).read().split() if args.resolved else []:
        host, _, ips = r.partition('=')
        resolved.append((host, [i for i in ips.split(',') if i]))
    seeds = SeedMap(args.seeds, resolved)
    results = []
    for path in args.logs:
        report, counts = analyse(path, seeds)
        results.append((path, counts))
        if not args.summary:
            sys.stdout.write(report + '\n')
    if args.summary:
        sys.stdout.write(summary(results))


if __name__ == '__main__':
    main()
