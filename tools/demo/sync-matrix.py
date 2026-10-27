#!/usr/bin/env python3
"""Run fresh-sync.sh serially; retain every attempt and summarize successful runs."""

import argparse
import json
import math
import os
from pathlib import Path
import platform
import signal
import shutil
import statistics
import subprocess
import sys
import uuid


def summarize(rows, path):
    good = [r for r in rows if r.get("outcome") == "synced"]
    def cell(values):
        values = sorted(v for v in values if v is not None)
        if not values:
            return "unavailable | unavailable | 0"
        return f"{statistics.median(values):.2f} | {values[math.ceil(.9 * len(values)) - 1]:.2f} | {len(values)}"
    lines = ["# Fresh sync measurements", "", f"Successful attempts: {len(good)}/{len(rows)}.", "",
             "Preliminary measurements. Do not use as release claims before the RC6 matrix.", "",
             "| Metric | Median | p90 | Samples |", "| --- | ---: | ---: | ---: |"]
    for label, key in [("Time to Synced (s)", "elapsed_s"), ("Peak RSS (MiB)", "peak_rss_mib"),
                       ("Connected peers at Synced", "peers")]:
        lines.append(f"| {label} | {cell([r.get(key) for r in good])} |")
    phases = {"Bootstrap", "Catchup"} | {p for r in good for p in r.get("phase_seconds", {})}
    for phase in sorted(phases):
        lines.append(f"| {phase} (s) | {cell([r.get('phase_seconds', {}).get(phase) for r in good])} |")
    lines += ["", "p90 uses nearest rank. Failures are excluded from percentiles and listed below.",
              "Phase durations use observation times at 0.5 s polling plus HTTP latency.",
              "Unobserved phases are unavailable, not zero. Status timestamps are retained verbatim.",
              "Time starts immediately before launching the node and ends on the first observed Synced.",
              "Metadata probes, image download and CPU/memory wrapper startup are excluded.",
              "HOME and node data are fresh each time. The OS page cache is not flushed.",
              "Peak RSS is the node's Linux VmHWM through Synced, not cgroup memory or Docker CLI RSS.", "",
              "| Run | Outcome | Seconds | Height | Version | Commit |", "| --- | --- | ---: | ---: | --- | --- |"]
    for i, row in enumerate(rows, 1):
        elapsed = row.get("elapsed_s")
        seconds = f"{elapsed:.2f}" if elapsed is not None else "unavailable"
        lines.append(f"| {i} | {row['outcome']} | {seconds} | {row.get('height', '')} | {row.get('version', '')} | {row.get('commit', '')} |")
        if row.get("error"):
            lines.append(f"\nRun {i}: {row['error']}\n")
    if rows and rows[0].get("constraints"):
        config = rows[0]["constraints"]
        lines += ["", f"Network: {config['network']}. CPU list: {config.get('cpus') or 'unconstrained'}.",
                  f"MemoryMax: {config.get('memory_max') or 'unconstrained'}.",
                  f"CPU: {config.get('cpu_model', 'not recorded')}. RAM: {config.get('host_mem_total', 'not recorded')}."]
    path.write_text("\n".join(lines) + "\n")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--runs", type=int, default=5)
    p.add_argument("--network", choices=["devnet", "mainnet"], default="devnet")
    source = p.add_mutually_exclusive_group()
    source.add_argument("--binary", default="mina")
    source.add_argument("--docker")
    p.add_argument("--port", type=int, default=3000)
    p.add_argument("--libp2p-port", type=int, default=8302)
    p.add_argument("--cpus", help="taskset CPU list, e.g. 0-3")
    p.add_argument("--memory-max", help="systemd MemoryMax, e.g. 8G (8 GiB)")
    p.add_argument("--systemd", action="store_true", help="use named user units even without a cap")
    p.add_argument("--node-lock", type=Path, help="repository lock.sh, required for shared-machine mainnet")
    p.add_argument("--temp-root", type=Path)
    p.add_argument("--timeout", type=float, default=900)
    p.add_argument("--min-free-gib", type=float, default=1)
    p.add_argument("--output", type=Path, required=True, help="new output directory")
    args = p.parse_args()
    if args.runs < 1:
        p.error("runs must be positive")
    if args.docker and (args.cpus or args.memory_max):
        p.error("CPU/memory wrappers constrain native nodes only; Docker's daemon is outside the unit")
    if not args.docker:
        binary = shutil.which(args.binary)
        if not binary:
            p.error(f"binary not found: {args.binary}")
        args.binary = str(Path(binary).resolve())
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    metadata = {"network": args.network, "requested_runs": args.runs, "cpus": args.cpus,
                "memory_max": args.memory_max, "platform": platform.platform(),
                "available_cpus": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
                "timeout_s": args.timeout, "min_free_gib": args.min_free_gib}
    try:
        metadata["cpu_model"] = next(line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name"))
        metadata["host_mem_total"] = next(line for line in Path("/proc/meminfo").read_text().splitlines() if line.startswith("MemTotal:"))
    except (OSError, StopIteration):
        pass
    metadata["load_average_at_start"] = os.getloadavg() if hasattr(os, "getloadavg") else None
    (args.output / "matrix.json").write_text(json.dumps(metadata, indent=2) + "\n")
    rows = []
    active = None
    unit = None
    def interrupted(*_):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    try:
        for i in range(1, args.runs + 1):
            run_dir = args.output / f"run-{i:02d}"
            command = [str(Path(__file__).with_name("fresh-sync.sh")), "--network", args.network,
                       "--port", str(args.port), "--libp2p-port", str(args.libp2p_port),
                       "--timeout", str(args.timeout), "--min-free-gib", str(args.min_free_gib),
                       "--output", str(run_dir)]
            command += ["--docker", args.docker] if args.docker else ["--binary", args.binary]
            if args.temp_root:
                command += ["--temp-root", str(args.temp_root.resolve())]
            if args.cpus:
                command = ["taskset", "--cpu-list", args.cpus] + command
            if args.node_lock:
                command = [str(args.node_lock.resolve()), "node", "--"] + command
            unit = None
            if args.systemd or args.memory_max:
                unit = "mina-sync-demo-" + uuid.uuid4().hex
                wrapper = ["systemd-run", "--user", "--quiet", "--wait", "--pipe", "--collect",
                           "--unit", unit, "--property", "KillMode=mixed", "--property", "TimeoutStopSec=45",
                           "--property", "RuntimeMaxSec=" + str(int(args.timeout) + 300)]
                if args.memory_max:
                    wrapper += ["--property", "MemoryMax=" + args.memory_max,
                                "--property", "MemorySwapMax=0"]
                command = wrapper + ["--"] + command
            print(f"Run {i}/{args.runs}: {args.network}", flush=True)
            with (args.output / f"run-{i:02d}.console.log").open("w") as log:
                active = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
                for line in active.stdout:
                    print(line, end="", flush=True)
                    log.write(line)
                    log.flush()
                code = active.wait()
                active = None
            result_path = run_dir / "result.json"
            row = json.loads(result_path.read_text()) if result_path.exists() else {
                "outcome": "failed", "error": "runner failed before producing result.json"}
            row.update(run=i, exit_code=code, constraints=metadata, systemd_unit=unit)
            if args.memory_max and row["outcome"] == "synced" and (
                row.get("cgroup_memory_max") in (None, "max") or row.get("cgroup_memory_swap_max") != "0"
            ):
                row.update(outcome="failed", error="memory cap or zero-swap setting was not observable in cgroup v2")
            if code and row["outcome"] == "synced":
                row.update(outcome="failed", error=f"wrapper exited {code} after Synced")
            rows.append(row)
            with (args.output / "results.jsonl").open("a") as stream:
                stream.write(json.dumps(row) + "\n")
            summarize(rows, args.output / "summary.md")
            # A disk-floor failure is not helped by immediately trying again.
            if "disk floor" in row.get("error", ""):
                break
    except KeyboardInterrupt:
        print("Interrupted; stopping this run.", file=sys.stderr)
        return 130
    finally:
        if active:
            if unit:
                subprocess.run(["systemctl", "--user", "stop", unit], check=False, timeout=60)
            else:
                active.terminate()
            active.wait(timeout=60)
    return 0 if len(rows) == args.runs and all(r["outcome"] == "synced" for r in rows) else 1


if __name__ == "__main__":
    sys.exit(main())
