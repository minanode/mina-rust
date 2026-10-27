#!/usr/bin/env python3
"""Sample a native node's HTTP endpoints and threads during a bootstrap.

Usage: sample.py --port 3430 --pid PID --out DIR [--interval 2]

Writes DIR/samples.jsonl (one line per sample) and, once the node has
reported Synced for --after seconds or the process exits, DIR/final-*.json
with /stats/sync, /stats/actions, /state/peers and /status.
"""

import argparse
import json
import os
import time
import urllib.request

TICK = os.sysconf("SC_CLK_TCK")


def get(port, path, timeout=5):
    try:
        url = f"http://127.0.0.1:{port}{path}"
        with urllib.request.urlopen(url, timeout=timeout) as r:
            return json.loads(r.read())
    except Exception as e:  # noqa: BLE001
        return {"_error": repr(e)}


def threads(pid):
    """CPU seconds (user + system) per thread, keyed by tid."""
    out = {}
    base = f"/proc/{pid}/task"
    try:
        tids = os.listdir(base)
    except FileNotFoundError:
        return None
    for tid in tids:
        try:
            with open(f"{base}/{tid}/comm") as f:
                comm = f.read().strip()
            with open(f"{base}/{tid}/stat") as f:
                st = f.read()
            fields = st[st.rfind(")") + 2 :].split()
            cpu = (int(fields[11]) + int(fields[12])) / TICK
        except (FileNotFoundError, ProcessLookupError, IndexError):
            continue
        out[tid] = (comm, cpu)
    return out


def rss_mb(pid):
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) / 1024
    except FileNotFoundError:
        return None
    return None


def socket_bytes(pid):
    """bytes_received per established TCP socket of the process, from ss."""
    import re
    import subprocess

    try:
        txt = subprocess.run(
            ["ss", "-tinpH", "state", "established"],
            capture_output=True,
            text=True,
            timeout=5,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return None
    out = {}
    lines = txt.splitlines()
    for i, line in enumerate(lines):
        if f"pid={pid}," not in line or i + 1 >= len(lines):
            continue
        cols = line.split()
        m = re.search(r"bytes_received:(\d+)", lines[i + 1])
        if len(cols) >= 4 and m:
            out[cols[2] + "-" + cols[3]] = int(m.group(1))
    return out


def summarise_peers(peers):
    if not isinstance(peers, list):
        return peers
    by_status = {}
    ready = []
    for p in peers:
        s = p.get("connection_status")
        by_status[s] = by_status.get(s, 0) + 1
        if s == "Connected":
            ready.append(
                {
                    "id": p.get("peer_id"),
                    "addr": p.get("address"),
                    "h": p.get("best_tip_height"),
                    "tip": p.get("best_tip"),
                    "in": p.get("incoming"),
                }
            )
    return {"by_status": by_status, "ready": ready}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", type=int, required=True, help="the node's HTTP port")
    ap.add_argument("--pid", type=int, required=True, help="the node's PID, for /proc and ss")
    ap.add_argument("--out", required=True, help="output directory")
    ap.add_argument("--interval", type=float, default=2.0, help="seconds between samples (default 2)")
    ap.add_argument("--after", type=float, default=60.0, help="seconds to keep sampling after Synced (default 60)")
    ap.add_argument("--max", type=float, default=3600.0, help="stop after this many seconds (default 3600)")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    start = time.time()
    synced_at = None
    syncing = False
    last_stats = 0.0
    with open(os.path.join(a.out, "samples.jsonl"), "a") as out:
        while True:
            t = time.time()
            if not os.path.exists(f"/proc/{a.pid}"):
                break
            status = get(a.port, "/status")
            tf = status.get("transition_frontier", {}) if isinstance(status, dict) else {}
            sync = tf.get("sync")
            rec = {
                "t": t,
                "sync": sync,
                "best_tip": tf.get("best_tip"),
                "peers": summarise_peers(get(a.port, "/state/peers")),
                "progress": get(a.port, "/state/message-progress"),
                "rss_mb": rss_mb(a.pid),
                "sockets": socket_bytes(a.pid),
                "threads": threads(a.pid),
            }
            if t - last_stats >= 20:
                rec["stats_sync"] = get(a.port, "/stats/sync?limit=3", timeout=10)
                last_stats = t
            out.write(json.dumps(rec) + "\n")
            out.flush()
            # The genesis frontier reports Synced before the first best tip
            # arrives: only stop once a bootstrap or catch-up has been seen.
            phase = (sync or {}).get("phase")
            if phase in ("Bootstrap", "Catchup") and (sync or {}).get("target"):
                syncing = True
            if syncing and phase == "Synced" and synced_at is None:
                synced_at = t
            if synced_at and t - synced_at > a.after:
                break
            if t - start > a.max:
                break
            time.sleep(max(0.0, a.interval - (time.time() - t)))
    for name, path in [
        ("stats-sync", "/stats/sync?limit=100"),
        ("stats-actions", "/stats/actions"),
        ("peers", "/state/peers"),
        ("status", "/status"),
    ]:
        with open(os.path.join(a.out, f"final-{name}.json"), "w") as f:
            json.dump(get(a.port, path, timeout=30), f)


if __name__ == "__main__":
    main()
