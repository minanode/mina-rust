#!/usr/bin/env python3
"""Summarize long-running Mina node logs without contacting the nodes.

Most of the kinds it reads are logged at DEBUG since O1
(private implementation notes): give it the log of a node run with
`--verbosity debug`.

Usage: python3 tools/regression/soak_health.py LOG [LOG ...]
Outputs JSON with hourly events, representative line numbers, and sync timeline.
"""

import collections
import json
import re
import sys

STAMP = re.compile(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.\d+)?Z\s+(?:INFO|WARN|ERROR|DEBUG|TRACE)\b")
LEVEL = re.compile(r"\s(INFO|WARN|ERROR|DEBUG|TRACE)\s")
KIND = re.compile(r'\bkind(?:: |=)"([^"\n]+)"')
HEIGHT = re.compile(r"new_best_tip_height(?:: |=)(\d+)")
HASH = re.compile(r"new_best_tip_hash(?:: |=)([1-9A-HJ-NP-Za-km-z]+)")
APPLY = re.compile(r'kind(?:: |=)"LedgerService::block_apply".*?summary(?:: |=)"(\d+), ([1-9A-HJ-NP-Za-km-z]+) <-')
PEER = re.compile(r'peer_id(?:: |=)"?([1-9A-HJ-NP-Za-km-z]{40,60})')


def classify(level, line, kind):
    if "Increasing buffer size" in line:
        return "mio buffer resize"
    if kind:
        if kind == "P2pNetworkSchedulerError":
            error = re.search(r"\berror(?:: |=)(.*)$", line)
            if error:
                value = error.group(1).lower()
                for label, needle in (
                    ("noise peer ID mismatch", "remote peer id doesn't match"),
                    ("yamux reset", "peer reset yamux stream"),
                    ("protocol select", "select protocol error"),
                    ("remote close", "remote peer closed connection"),
                    ("socket error", "mio error"),
                ):
                    if needle in value:
                        return kind + ": " + label
        return kind
    summary = re.search(r'\bsummary(?:: |=)"([^"\n]+)', line)
    if summary:
        if summary.group(1) == "Failure when handling a P2P action":
            action = re.search(r'\baction_kind(?:: |=)"([^"\n]+)', line)
            return "P2P action failure: " + (action.group(1) if action else "unknown")
        return summary.group(1)[:90]
    if "disconnected peer connection for address" in line:
        return "scheduler: disconnected peer connection"
    if "trying to negotiate" in line:
        return "select: negotiated but unhandled stream"
    if "Peer address name resolution failed" in line:
        return "peer DNS resolution failed"
    if "Increasing buffer size" in line:
        return "mio buffer resize"
    return "other: " + line[stamp.end():].strip()[:90] if (stamp := STAMP.search(line)) else "other"


def analyze(path):
    counters = collections.Counter()
    hours = collections.defaultdict(collections.Counter)
    examples = {}
    tips = []
    applies = []
    peers = collections.defaultdict(set)
    first = last = None
    line_count = 0
    with open(path, encoding="utf-8", errors="replace") as stream:
        for number, line in enumerate(stream, 1):
            line_count = number
            stamp = STAMP.search(line)
            if not stamp:
                continue
            instant = stamp.group(1) + "Z"
            hour = stamp.group(1)[:13]
            first = first or instant
            last = instant
            level_match = LEVEL.search(line)
            level = level_match.group(1) if level_match else "?"
            kind_match = KIND.search(line)
            kind = kind_match.group(1) if kind_match else None
            if kind:
                key = "kind:" + kind
                counters[key] += 1
                hours[hour][key] += 1
                examples.setdefault(key, number)
            if level in ("WARN", "ERROR"):
                key = level + ":" + classify(level, line, kind)
                counters[key] += 1
                hours[hour][key] += 1
                examples.setdefault(key, number)
            if kind == "TransitionFrontierSyncBestTipUpdate":
                height = HEIGHT.search(line)
                block_hash = HASH.search(line)
                if height and block_hash:
                    tips.append([instant, int(height.group(1)), block_hash.group(1), number])
            apply = APPLY.search(line)
            if apply:
                applies.append([instant, int(apply.group(1)), apply.group(2), number])
            if kind in ("P2pConnectionIncomingSuccess", "P2pDisconnectionInit", "P2pDisconnectionPeerClosed"):
                peer = PEER.search(line)
                if peer:
                    peers[kind].add(peer.group(1))
    regressions = []
    for old, new in zip(tips, tips[1:]):
        if new[1] < old[1] or (new[1] == old[1] and new[2] != old[2]):
            regressions.append([old, new])
    return {
        "path": path,
        "first": first,
        "last": last,
        "lines": line_count,
        "counts": dict(counters),
        "hours": {hour: dict(values) for hour, values in hours.items()},
        "examples": examples,
        "tips": tips,
        "tip_regressions": regressions,
        "applied_first_last": [applies[0], applies[-1]] if applies else [],
        "applied_heights": len({entry[1] for entry in applies}),
        "applies": applies,
        "unique_peers": {kind: len(values) for kind, values in peers.items()},
    }


if __name__ == "__main__":
    print(json.dumps([analyze(path) for path in sys.argv[1:]], indent=2))
