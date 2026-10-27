#!/usr/bin/env python3
"""Summarise a devnet node's health log, written by watch.sh, hour by hour.

Usage: python3 tools/regression/devnet/summary.py HEALTH [EVENTS]

HEALTH has one JSON line a minute from the node's /status; EVENTS (default:
HEALTH with .events for .health) the watchdog's starts and stops. It prints a
Markdown table, one row per UTC hour: samples, the share Synced, best tip
heights, the most blocks it was behind its highest connected peer, connected
peers (libp2p and WebRTC), resident memory, unreadable samples; then the
events. Reads files only; it doesn't contact the node.
"""

import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def rows(path):
    with open(path, encoding="utf-8") as stream:
        for line in stream:
            line = line.strip()
            if line:
                try:
                    yield json.loads(line)
                except json.JSONDecodeError:
                    yield {"t": None, "error": "bad line"}


def span(values):
    values = [v for v in values if v is not None]
    if not values:
        return "-"
    low, high = min(values), max(values)
    return str(low) if low == high else f"{low}-{high}"


def main():
    if len(sys.argv) not in (2, 3) or sys.argv[1] in ("-h", "--help"):
        print(__doc__.strip())
        return 0 if len(sys.argv) == 2 else 2
    health = Path(sys.argv[1])
    events = Path(sys.argv[2]) if len(sys.argv) == 3 else health.with_suffix(".events")
    hours = defaultdict(list)
    for row in rows(health):
        hours[(row.get("t") or "unknown")[:13]].append(row)
    print("| hour (UTC) | samples | Synced | height | max behind | peers (median) | WebRTC | RSS MB | unreadable |")
    print("| --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for hour in sorted(hours):
        samples = hours[hour]
        good = [s for s in samples if "error" not in s]
        synced = sum(1 for s in good if s.get("sync") == "Synced")
        peers = [s.get("peers") for s in good if s.get("peers") is not None]
        median = f" ({statistics.median(peers):g})" if peers else ""
        print(
            f"| {hour.replace('T', ' ')}h | {len(samples)} | {synced}/{len(good)} "
            f"| {span(s.get('height') for s in good)} "
            f"| {max((s.get('behind') or 0 for s in good), default='-')} "
            f"| {span(peers)}{median} | {span(s.get('webrtc') for s in good)} "
            f"| {span(s.get('rss_mb') for s in samples)} | {len(samples) - len(good)} |"
        )
    if events.exists():
        print()
        print(f"Events ({events}):")
        print()
        for line in events.read_text(encoding="utf-8").splitlines():
            print(f"- {line}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
