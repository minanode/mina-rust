#!/usr/bin/env python3
"""Summarize D1 producer samples pulled from tools/remote."""

import argparse
import datetime as dt
import json
from pathlib import Path
import re


SLOT_NS = 90_000_000_000


def read_rows(paths):
    rows = []
    for path in paths:
        for line in path.read_text().splitlines():
            if line:
                rows.append(json.loads(line))
    return sorted(rows, key=lambda row: row["time"])


def attempts_by_slot(rows):
    attempts = {}
    for row in rows:
        if row.get("node_pid") is None:
            continue
        for attempt in (row.get("producer") or {}).get("attempts") or []:
            slot = attempt["won_slot"]["global_slot"]
            previous = attempts.get(slot)
            if previous is None or rank(attempt) >= rank(previous):
                attempts[slot] = attempt
    return attempts


def rank(attempt):
    status = attempt["status"]
    return (
        bool((attempt.get("times") or {}).get("committed")),
        status in ("Canonical", "Orphaned"),
        status == "Discarded",
        bool(attempt.get("block")),
    )


def log_reasons(paths, slot):
    patterns = ("BlockProducer", "discard", "proof", "panic", "ERROR")
    found = []
    for path in paths:
        for line in path.read_text(errors="replace").splitlines():
            if str(slot) in line and any(p in line for p in patterns):
                found.append(line[:350])
    return found[-3:]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("samples", type=Path, nargs="+")
    parser.add_argument("--log", type=Path, action="append", default=[])
    args = parser.parse_args()
    rows = read_rows(args.samples)
    if not rows:
        raise SystemExit("no samples")
    attempts = attempts_by_slot(rows)
    public_hashes = {
        block["hash"] for row in rows for block in row.get("public_chain") or []
    }
    latest_public_at_height = {}
    for row in rows:
        for block in row.get("public_chain") or []:
            latest_public_at_height[int(block["height"])] = block["hash"]
    node_hashes = {
        block["hash"] for row in rows if row.get("node_pid") is not None
        for block in row.get("node_chain") or []
    }
    now_ns = int(dt.datetime.fromisoformat(rows[-1]["time"]).timestamp() * 1e9)
    reached = {
        slot: attempt for slot, attempt in attempts.items()
        if now_ns >= attempt["won_slot"]["slot_time"]
    }
    future = sorted(set(attempts) - set(reached))
    produced = []
    canonical = []
    orphaned = []
    missed = []
    late = []
    for slot, attempt in sorted(reached.items()):
        block = attempt.get("block")
        times = attempt.get("times") or {}
        status = attempt["status"]
        if times.get("committed") and block:
            produced.append((slot, block))
            public_at_height = latest_public_at_height.get(int(block["height"]))
            if status == "Orphaned" or public_at_height not in (None, block["hash"]):
                orphaned.append((slot, block))
            elif block["hash"] in public_hashes or status == "Canonical":
                canonical.append((slot, block))
            if times["committed"] > attempt["won_slot"]["slot_time"] + SLOT_NS:
                late.append((slot, block))
        elif now_ns > attempt["won_slot"]["slot_time"] + SLOT_NS:
            reason = attempt.get("discard_reason") or status
            missed.append((slot, reason, log_reasons(args.log, slot)))
    pids = [(row["time"], row.get("node_pid")) for row in rows]
    changes = []
    last_pid = pids[0][1]
    for time, pid in pids[1:]:
        if pid != last_pid:
            changes.append((time, last_pid, pid))
            last_pid = pid
    restored = []
    for path in args.log:
        for line in path.read_text(errors="replace").splitlines():
            if 'kind: "PersistRestored"' in line:
                times = {
                    name: float(value)
                    for name, value in re.findall(r"(load_ms|replay_ms|total_ms): ([0-9.]+)", line)
                }
                restored.append({"time": line[:27], **times})
    phases = [row.get("sync_phase") if row.get("node_pid") is not None else None for row in rows]
    epochs = [(row["time"], (row.get("producer") or {}).get("current_epoch"))
              for row in rows if row.get("node_pid") is not None]
    transitions = [epochs[i] for i in range(1, len(epochs)) if epochs[i][1] != epochs[i-1][1]]
    print(f"window: {rows[0]['time']} to {rows[-1]['time']} ({len(rows)} samples)")
    print(f"sync: {phases.count('Synced')}/{len(phases)} samples Synced")
    print(f"won slots reached: {len(reached)} {sorted(reached)}")
    print(f"scheduled future slots: {len(future)} {future}")
    print(f"produced (committed): {len(produced)}")
    print(f"canonical (latest public view or node status): {len(canonical)}")
    print(f"orphaned (latest public view or node status): {len(orphaned)}")
    print(f"missed (past slot end, no commit): {len(missed)}")
    print(f"late (committed after slot end): {len(late)}")
    for slot, block in produced:
        label = "canonical" if (slot, block) in canonical else "orphaned" if (slot, block) in orphaned else "unconfirmed"
        print(f"  block slot={slot} height={block['height']} hash={block['hash']} {label} node_chain_seen={block['hash'] in node_hashes}")
    for slot, reason, evidence in missed:
        print(f"  missed slot={slot} reason={reason} log={evidence}")
    print(f"restarts / PID changes: {changes}")
    print(f"restores: {restored}")
    for metric in ("rss_bytes", "store_bytes", "log_bytes"):
        values = [row[metric] for row in rows if row.get(metric) is not None]
        if values:
            print(f"{metric}: first={values[0]} last={values[-1]} min={min(values)} max={max(values)} delta={values[-1]-values[0]}")
    print(f"epoch transitions: {transitions}")
    if transitions:
        print("epoch boundary observed in sampler")
    else:
        print("epoch boundary not yet observed")


if __name__ == "__main__":
    main()
