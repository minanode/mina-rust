#!/usr/bin/env python3
"""Measure return to the public devnet tip from a saved follower frontier."""

import csv
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from urllib.request import urlopen

mina, genesis, node_script, saved_node_dir = sys.argv[1:5]
out = Path(os.environ["OUT"])
http_port = os.environ["HTTP_PORT"]
p2p_port = os.environ["P2P_PORT"]
child = None
stop_requested = False


def signal_stop(_number, _frame):
    global stop_requested
    stop_requested = True
    if child and child.poll() is None:
        child.terminate()


signal.signal(signal.SIGTERM, signal_stop)
signal.signal(signal.SIGINT, signal_stop)


def cg_file(name):
    cg = Path("/proc/self/cgroup").read_text().split(":")[-1].strip().lstrip("/")
    try:
        return (Path("/sys/fs/cgroup") / cg / name).read_text().strip()
    except OSError:
        return None


def status():
    try:
        with urlopen(f"http://127.0.0.1:{http_port}/status", timeout=3) as response:
            document = json.load(response)
        frontier = document.get("transition_frontier") or {}
        sync = frontier.get("sync") or {}
        height = (frontier.get("best_tip") or {}).get("height")
        heights = [p.get("best_tip_height") for p in document.get("peers") or []
                   if p.get("connection_status") == "Connected"
                   and isinstance(p.get("best_tip_height"), int)]
        peer_max = max(heights) if heights else None
        behind = peer_max - height if isinstance(height, int) and peer_max is not None else None
        return sync.get("status"), height, peer_max, behind, len(heights)
    except (OSError, TimeoutError, ValueError, TypeError):
        return None, None, None, None, 0


def rss_kib(pid):
    try:
        return next(int(line.split()[1]) for line in Path(f"/proc/{pid}/status").read_text().splitlines()
                    if line.startswith("VmRSS:"))
    except (OSError, StopIteration, ValueError):
        return None


env = os.environ.copy()
env.update(MINA=mina, GENESIS=genesis,
           EXTRA_ARGS="--frontier-store --checkpoint-interval 300")
summary = {"start_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
           "saved_node_dir": saved_node_dir, "cpu_max": cg_file("cpu.max"),
           "memory_max": cg_file("memory.max"), "tip_reached": False}
start = time.monotonic()
with (out / "node.log").open("w") as log, (out / "samples.csv").open("w", newline="") as sample:
    writer = csv.writer(sample)
    writer.writerow(["elapsed_s", "status", "height", "peer_max_height", "behind",
                     "peers", "rss_kib", "cgroup_memory_current", "cgroup_memory_peak"])
    try:
        child = subprocess.Popen(["bash", node_script, saved_node_dir, http_port, p2p_port],
                                 env=env, stdout=log, stderr=subprocess.STDOUT)
        while not stop_requested and time.monotonic() - start < 900:
            elapsed = round(time.monotonic() - start, 1)
            sync, height, peer_max, behind, peers = status()
            writer.writerow([elapsed, sync, height, peer_max, behind, peers,
                             rss_kib(child.pid), cg_file("memory.current"),
                             cg_file("memory.peak")])
            sample.flush()
            if sync == "Synced" and behind is not None and 0 <= behind <= 2:
                summary.update(tip_reached=True, tip_elapsed_s=elapsed, tip_height=height,
                               peer_max_height=peer_max, behind=behind)
                break
            if child.poll() is not None:
                summary["exit_code"] = child.returncode
                break
            time.sleep(2)
    finally:
        if child and child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=30)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=10)
        summary["elapsed_s"] = round(time.monotonic() - start, 1)
        summary["memory_peak_bytes"] = cg_file("memory.peak")
        summary["memory_events"] = cg_file("memory.events")
        summary["end_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
