#!/usr/bin/env python3
"""Measure a read-only devnet follower in one tools/remote run slot."""

import csv
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from urllib.request import urlopen

out = Path(os.environ["OUT"])
data = Path(os.environ["DATA"])
mina, genesis, node_script = sys.argv[1:4]
http_port = os.environ["HTTP_PORT"]
p2p_port = os.environ["P2P_PORT"]
node_dir = data / "node"
node_dir.mkdir(exist_ok=True)
stop_requested = False
child = None


def stop_signal(_number, _frame):
    global stop_requested
    stop_requested = True
    if child and child.poll() is None:
        child.send_signal(signal.SIGTERM)


signal.signal(signal.SIGTERM, stop_signal)
signal.signal(signal.SIGINT, stop_signal)


def cg_file(name):
    cg = Path("/proc/self/cgroup").read_text().split(":")[-1].strip().lstrip("/")
    path = Path("/sys/fs/cgroup") / cg / name
    try:
        return path.read_text().strip()
    except OSError:
        return None


def proc_stats(pid):
    try:
        lines = Path(f"/proc/{pid}/status").read_text().splitlines()
        rss_kib = next(int(line.split()[1]) for line in lines if line.startswith("VmRSS:"))
        stat = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
        cpu_ticks = int(stat[11]) + int(stat[12])
        return rss_kib, cpu_ticks
    except (OSError, StopIteration, ValueError, IndexError):
        return None, None


def status():
    try:
        with urlopen(f"http://127.0.0.1:{http_port}/status", timeout=4) as response:
            document = json.load(response)
        frontier = document.get("transition_frontier") or {}
        sync = frontier.get("sync") or {}
        tip = frontier.get("best_tip") or {}
        heights = [p.get("best_tip_height") for p in document.get("peers") or []
                   if p.get("connection_status") == "Connected"
                   and p.get("best_tip_height") is not None]
        height = tip.get("height")
        peer_max = max(heights) if heights else None
        behind = peer_max - height if isinstance(height, int) and isinstance(peer_max, int) else None
        return sync.get("status"), sync.get("phase"), height, peer_max, behind, len(heights)
    except (OSError, TimeoutError, ValueError, TypeError):
        return None, None, None, None, None, 0


def launch(phase):
    global child
    env = os.environ.copy()
    env.update(MINA=mina, GENESIS=genesis,
               EXTRA_ARGS="--frontier-store --checkpoint-interval 300")
    log = (out / f"node-{phase}.log").open("w")
    child = subprocess.Popen(["bash", node_script, str(node_dir), http_port, p2p_port],
                             env=env, stdout=log, stderr=subprocess.STDOUT)
    return log


def stop_node():
    global child
    if child and child.poll() is None:
        child.terminate()
        try:
            child.wait(timeout=30)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=10)
    child = None


summary = {"start_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
           "cpu_max": cg_file("cpu.max"), "memory_max": cg_file("memory.max"),
           "phases": {}, "sampling_seconds": 10}
with (out / "samples.csv").open("w", newline="") as stream:
    writer = csv.writer(stream)
    writer.writerow(["phase", "elapsed_s", "status", "sync_phase", "height", "peer_max_height",
                     "behind", "peers", "rss_kib", "cpu_pct_one_core", "cgroup_memory_current",
                     "cgroup_memory_peak"])
    try:
        for phase, limit in (("bootstrap", 3600), ("tip", 3600), ("restart", 1200)):
            if phase == "tip" and not summary["phases"].get("bootstrap", {}).get("synced"):
                break
            if phase == "restart":
                stop_node()
                time.sleep(3)
            log = launch(phase) if phase != "tip" else None
            start = time.monotonic()
            previous = None
            phase_result = {"synced": False, "exit_code": None, "duration_s": None,
                            "peak_rss_kib": 0, "max_behind": None, "samples": 0,
                            "synced_samples": 0}
            summary["phases"][phase] = phase_result
            while not stop_requested and time.monotonic() - start < limit:
                now = time.monotonic()
                sync, sync_phase, height, peer_max, behind, peers = status()
                rss, ticks = proc_stats(child.pid)
                cpu = None
                if previous and ticks is not None and previous[1] is not None:
                    cpu = round((ticks - previous[1]) / os.sysconf("SC_CLK_TCK") /
                                (now - previous[0]) * 100, 1)
                previous = (now, ticks)
                phase_result["samples"] += 1
                if rss is not None:
                    phase_result["peak_rss_kib"] = max(phase_result["peak_rss_kib"], rss)
                if behind is not None:
                    phase_result["max_behind"] = max(phase_result["max_behind"] or 0, behind)
                if str(sync).lower() == "synced":
                    phase_result["synced_samples"] += 1
                    if behind is not None and 0 <= behind <= 2 and not phase_result["synced"]:
                        phase_result["synced"] = True
                        phase_result["sync_elapsed_s"] = round(now - start, 1)
                writer.writerow([phase, round(now - start, 1), sync, sync_phase, height,
                                 peer_max, behind, peers, rss, cpu,
                                 cg_file("memory.current"), cg_file("memory.peak")])
                stream.flush()
                if phase in ("bootstrap", "restart") and phase_result["synced"]:
                    break
                if child.poll() is not None:
                    phase_result["exit_code"] = child.returncode
                    break
                time.sleep(10)
            phase_result["duration_s"] = round(time.monotonic() - start, 1)
            if phase != "tip":
                log.close()
            if phase == "tip" and phase_result["duration_s"] < 3550:
                break
    finally:
        stop_node()
        summary["memory_peak_bytes"] = cg_file("memory.peak")
        summary["memory_events"] = cg_file("memory.events")
        summary["end_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
