#!/usr/bin/env python3
"""Sample a remote devnet producer once per minute without changing node state."""

import argparse
import datetime as dt
import json
import os
from pathlib import Path
import time
import urllib.request


CHAIN_QUERY = {
    "query": "{ bestChain(maxLength: 30) { stateHash protocolState { consensusState { blockHeight slot slotSinceGenesis epoch } } } }"
}


def get_json(url, payload=None):
    body = None if payload is None else json.dumps(payload).encode()
    req = urllib.request.Request(
        url,
        data=body,
        headers={"Content-Type": "application/json", "User-Agent": "curl/8.0"} if body else {},
    )
    with urllib.request.urlopen(req, timeout=12) as response:
        result = json.load(response)
    if isinstance(result, dict) and result.get("errors"):
        raise ValueError(str(result["errors"])[:300])
    return result


def node_process(data_dir):
    """Find our node by its data directory in argv; never inspect its environment."""
    for proc in Path("/proc").glob("[0-9]*"):
        try:
            args = (proc / "cmdline").read_bytes().split(b"\0")
            if not args or b"--producer-key" not in args:
                continue
            if os.fsencode(str(data_dir / "home")) not in args:
                continue
            status = (proc / "status").read_text()
            rss = next(
                int(line.split()[1]) * 1024
                for line in status.splitlines()
                if line.startswith("VmRSS:")
            )
            port = None
            for i, arg in enumerate(args[:-1]):
                if arg in (b"-p", b"--port"):
                    port = int(args[i + 1])
                    break
            return int(proc.name), rss, port
        except (OSError, StopIteration, ValueError):
            continue
    return None, None, None


def tree_bytes(path):
    total = 0
    for root, _, files in os.walk(path):
        for name in files:
            try:
                total += (Path(root) / name).stat().st_size
            except OSError:
                pass
    return total


def sample(args):
    now = dt.datetime.now(dt.timezone.utc)
    row = {"time": now.isoformat(timespec="seconds")}
    pid, row["rss_bytes"], port = node_process(args.data_dir)
    row["node_pid"] = pid
    row["node_port"] = port
    row["store_bytes"] = tree_bytes(args.data_dir / "home")
    row["log_bytes"] = sum(
        p.stat().st_size for p in args.remote_root.glob("runs/D1-*/log") if p.is_file()
    )
    probes = [("public_chain", args.public_url, CHAIN_QUERY)]
    if port is not None:
        node_url = f"http://127.0.0.1:{port}"
        probes[:0] = [
            ("status", node_url + "/status", None),
            ("producer", node_url + "/stats/block_producer", None),
            ("node_chain", node_url + "/graphql", CHAIN_QUERY),
        ]
    else:
        row["node_unavailable"] = True
    for name, url, body in probes:
        try:
            result = get_json(url, body)
            if name == "status":
                frontier = result.get("transition_frontier") or {}
                sync = frontier.get("sync") or {}
                tip = frontier.get("best_tip") or {}
                row.update(
                    sync_phase=sync.get("phase"),
                    sync_status=sync.get("status"),
                    best_tip_height=tip.get("height"),
                    best_tip_hash=tip.get("hash"),
                    peers=len(result.get("peers") or []),
                )
            elif name == "producer":
                row["producer"] = {
                    field: result.get(field)
                    for field in (
                        "current_global_slot", "current_epoch", "epoch_start",
                        "epoch_end", "vrf_stats",
                    )
                } if result else None
                if row["producer"] is not None:
                    row["producer"]["attempts"] = (result.get("attempts") or [])[-32:]
                    row["producer"]["future_won_slots"] = (result.get("future_won_slots") or [])[:32]
            else:
                row[name] = [
                    {
                        "hash": b["stateHash"],
                        "height": b["protocolState"]["consensusState"]["blockHeight"],
                        "slot": b["protocolState"]["consensusState"]["slot"],
                        "global_slot": b["protocolState"]["consensusState"]["slotSinceGenesis"],
                        "epoch": b["protocolState"]["consensusState"]["epoch"],
                    }
                    for b in result["data"]["bestChain"]
                ]
        except (OSError, ValueError, KeyError, TypeError) as exc:
            row[name + "_error"] = str(exc)[:300]
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", type=Path, required=True)
    parser.add_argument("--remote-root", type=Path, required=True)
    parser.add_argument("--public-url", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--until", required=True)
    args = parser.parse_args()
    deadline = dt.datetime.fromisoformat(args.until.replace("Z", "+00:00")).timestamp()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("a", buffering=1) as output:
        while time.time() < deadline:
            started = time.monotonic()
            print(json.dumps(sample(args), separators=(",", ":")), file=output)
            time.sleep(max(0, 60 - (time.monotonic() - started)))


if __name__ == "__main__":
    main()
