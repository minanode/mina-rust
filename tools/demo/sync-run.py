#!/usr/bin/env python3
"""Single-run implementation shared by the camera demo and matrix (stdlib only)."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid


MAX_RESPONSE_BYTES = 4 * 1024 * 1024


def get_json(opener, url, timeout=1):
    """Bound headers, body and JSON parsing together, even for trickling peers.

    This standalone Unix runner uses its main thread's real-time timer so an
    idle socket timeout cannot be renewed indefinitely by incoming bytes.
    """
    if timeout <= 0:
        raise TimeoutError("HTTP request deadline expired")

    def expired(*_):
        raise TimeoutError("HTTP request deadline expired")

    previous = signal.signal(signal.SIGALRM, expired)
    signal.setitimer(signal.ITIMER_REAL, timeout)
    try:
        with opener.open(url, timeout=timeout) as response:
            body = response.read(MAX_RESPONSE_BYTES + 1)
            if len(body) > MAX_RESPONSE_BYTES:
                raise ValueError("HTTP response exceeds 4 MiB")
            return json.loads(body)
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)


def stop_container(container):
    """Only the daemon can confirm shutdown; the attached client may be gone."""
    diagnostics = []
    for command, timeout in [(["stop", "--time", "15"], 25), (["rm"], 10)]:
        try:
            reply = subprocess.run(["docker", *command, container],
                                   capture_output=True, text=True, timeout=timeout)
            if reply.returncode:
                diagnostics.append(f"docker {command[0]}: {reply.stderr.strip() or reply.returncode}")
        except (OSError, subprocess.SubprocessError) as error:
            diagnostics.append(f"docker {command[0]}: {error}")
    try:
        reply = subprocess.run(
            ["docker", "inspect", "--type", "container", "--format", "{{.State.Running}}", container],
            capture_output=True, text=True, timeout=10,
        )
        if reply.returncode == 0 and reply.stdout.strip() == "false":
            return None
        # Successful removal (including --rm) makes inspect return this exact
        # daemon error. Connection/permission errors do not establish absence.
        absent = rf"Error(?: response from daemon)?: No such (?:container|object): {re.escape(container)}"
        if reply.returncode == 1 and not reply.stdout.strip() and re.fullmatch(absent, reply.stderr.strip()):
            return None
        diagnostics.append(f"docker inspect: {reply.stderr.strip() or reply.stdout.strip() or reply.returncode}")
    except (OSError, subprocess.SubprocessError) as error:
        diagnostics.append(f"docker inspect: {error}")
    return "; ".join(diagnostics)


def arguments():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--network", choices=["devnet", "mainnet"], default="devnet")
    source = p.add_mutually_exclusive_group()
    source.add_argument("--binary", default="mina")
    source.add_argument("--docker", metavar="IMAGE")
    p.add_argument("--port", type=int, default=3000)
    p.add_argument("--libp2p-port", type=int, default=8302)
    p.add_argument("--keep", action="store_true", help="wait for Ctrl-C after Synced")
    p.add_argument("--output", type=Path, help="new evidence directory to retain")
    p.add_argument("--temp-root", type=Path, help="parent for temporary node data (created if missing)")
    p.add_argument("--timeout", type=float, default=900, help="sync deadline in seconds")
    p.add_argument("--min-free-gib", type=float, default=1, help="stop below this disk floor")
    args = p.parse_args()
    if not 0 < args.timeout or not 0 <= args.min_free_gib:
        p.error("timeout must be positive and disk floor nonnegative")
    if not all(0 < port < 65536 for port in [args.port, args.libp2p_port]):
        p.error("ports must be in 1..65535")
    if args.port == args.libp2p_port:
        p.error("HTTP and libp2p need different ports")
    return args


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def check_disk(path, floor):
    free = shutil.disk_usage(path).free
    if free < floor * 1024**3:
        raise RuntimeError(f"disk floor reached: {free / 1024**3:.2f} GiB free, need {floor:g}")
    return free


def check_ports(ports):
    # Refuse an occupied port before polling: never report another node's status.
    for port in ports:
        with socket.socket() as sock:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            sock.bind(("0.0.0.0", port))


def metadata(command, env):
    version = subprocess.check_output(command + ["--version"], env=env, text=True, timeout=60).strip()
    info = subprocess.check_output(command + ["build-info"], env=env, text=True, timeout=60)
    match = re.search(r"^Commit SHA:\s*([0-9a-f]{40})\s*$", info, re.M)
    if not match:
        raise RuntimeError("build-info did not identify a full commit SHA")
    return {"version": version, "commit": match[1], "build_info": info}


def rss_kib(pid):
    try:
        text = Path(f"/proc/{pid}/status").read_text()
        return int(re.search(r"^VmHWM:\s+(\d+)", text, re.M)[1])
    except (OSError, TypeError):
        return None


def stop(process):
    if process and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def run(args):
    check_ports([args.port, args.libp2p_port])
    root = (args.temp_root or Path(tempfile.gettempdir())).resolve()
    root.mkdir(parents=True, exist_ok=True)
    check_disk(root, args.min_free_gib)
    # A new HOME also isolates verifier and ledger caches from earlier runs.
    data = Path(tempfile.mkdtemp(prefix="mina-fresh-sync-", dir=root))
    output = args.output.resolve() if args.output else data / "evidence"
    process = None
    output_created = False
    container = None
    result = {"schema": 1, "network": args.network, "outcome": "failed", "transitions": [],
              "peak_rss_mib": None, "rss_method": "Linux /proc node VmHWM through first Synced",
              "cache_policy": "fresh work directory and HOME; OS page cache not flushed",
              "poll_seconds": 0.5, "http_port": args.port, "libp2p_port": args.libp2p_port}
    start = None
    # Never inherit producer keys, node settings, tracing or archive credentials.
    env = {k: os.environ[k] for k in ("PATH", "LANG", "LC_ALL", "SSL_CERT_FILE", "SSL_CERT_DIR")
           if k in os.environ}
    env.update(HOME=str(data), TMPDIR=str(data))
    try:
        output.mkdir(parents=True, exist_ok=False)
        output_created = True
        check_disk(output, args.min_free_gib)
        if args.docker:
            # Match compose: caller UID/GID, /data, loopback HTTP, separate peer port.
            image_id = subprocess.check_output(
                ["docker", "image", "inspect", "--format", "{{.Id}}", args.docker], text=True).strip()
            result.update(image=args.docker, image_id=image_id)
            probe = ["docker", "run", "--rm", image_id]
            result.update(metadata(probe, None))
            container = "mina-fresh-sync-" + uuid.uuid4().hex
            command = ["docker", "run", "--rm", "--name", container,
                       "--user", f"{os.getuid()}:{os.getgid()}",
                       "--mount", f"type=bind,source={data},target=/data",
                       "-p", f"127.0.0.1:{args.port}:3000",
                       "-p", f"{args.libp2p_port}:{args.libp2p_port}", image_id,
                       "node", "--network", args.network, "--libp2p-port", str(args.libp2p_port),
                       "--disable-filesystem-logging"]
        else:
            binary = shutil.which(args.binary)
            if not binary:
                raise RuntimeError(f"binary not found: {args.binary}")
            binary = str(Path(binary).resolve())
            result.update(metadata([binary], env))
            with open(binary, "rb") as stream:
                digest = hashlib.sha256()
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(chunk)
            result["binary_sha256"] = digest.hexdigest()
            command = [binary, "node", "--network", args.network, "--work-dir", str(data),
                       "--http-addr", "127.0.0.1", "--port", str(args.port),
                       "--libp2p-port", str(args.libp2p_port), "--disable-filesystem-logging"]
        result["cpu_affinity"] = sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None
        try:
            cgroup = next(line.split(":", 2)[2] for line in Path("/proc/self/cgroup").read_text().splitlines()
                          if line.startswith("0::"))
            group = Path("/sys/fs/cgroup") / cgroup.lstrip("/")
            result["cgroup_memory_max"] = (group / "memory.max").read_text().strip()
            result["cgroup_memory_swap_max"] = (group / "memory.swap.max").read_text().strip()
        except (OSError, StopIteration):
            pass
        base = f"http://127.0.0.1:{args.port}"
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

        print(f"Dashboard: {base}/", flush=True)
        print(f"Evidence: {output}" if args.output else "Use --output DIR to retain evidence.", flush=True)
        with (output / "node.log").open("w") as log, (output / "status.jsonl").open("w") as samples:
            result["started_at_unix_ns"] = time.time_ns()
            start = time.monotonic()
            process = subprocess.Popen(command, stdout=log, stderr=log, env=None if args.docker else env)
            node_pid = None if args.docker else process.pid
            last_phase = "Starting"
            last_change = 0.0
            durations = {}
            last_print = -10
            while True:
                elapsed = time.monotonic() - start
                result["free_bytes"] = check_disk(root, args.min_free_gib)
                result["minimum_free_bytes"] = min(result.get("minimum_free_bytes", result["free_bytes"]), result["free_bytes"])
                check_disk(output, args.min_free_gib)
                if process.poll() is not None:
                    raise RuntimeError(f"node exited with code {process.returncode}; see node.log")
                if elapsed >= args.timeout:
                    raise RuntimeError(f"sync timed out after {args.timeout:g} seconds")
                if args.docker and node_pid is None:
                    top = subprocess.run(["docker", "top", container, "-eo", "pid,comm"],
                                         capture_output=True, text=True, timeout=5)
                    for line in top.stdout.splitlines()[1:]:
                        fields = line.split()
                        if len(fields) == 2 and fields[1] == "mina":
                            candidate = int(fields[0])
                            try:
                                if Path(f"/proc/{candidate}/root/data").samefile(data):
                                    node_pid = candidate
                            except OSError:
                                pass
                rss = rss_kib(node_pid) if node_pid else None
                if rss is not None:
                    result["peak_rss_mib"] = max(result["peak_rss_mib"] or 0, rss / 1024)
                try:
                    remaining = args.timeout - (time.monotonic() - start)
                    status = get_json(opener, base + "/status", min(1, remaining))
                except (OSError, ValueError):
                    status = None
                elapsed = time.monotonic() - start
                if elapsed >= args.timeout:
                    raise RuntimeError(f"sync timed out after {args.timeout:g} seconds")
                if status:
                    frontier = status["transition_frontier"]
                    sync = frontier["sync"]
                    phase = sync["phase"]
                    peers = sum(p["connection_status"] == "Connected" for p in status["peers"])
                    sample = {"elapsed_s": elapsed, "observed_at_unix_ns": time.time_ns(), "status": status}
                    samples.write(json.dumps(sample) + "\n")
                    samples.flush()
                    if phase != last_phase:
                        durations[last_phase] = durations.get(last_phase, 0) + elapsed - last_change
                        transition = {"phase": phase, "elapsed_s": elapsed,
                                      "status_time_unix_ns": sync.get("time"),
                                      "observed_at_unix_ns": sample["observed_at_unix_ns"]}
                        result["transitions"].append(transition)
                        stamp = sync.get("time")
                        stamp = datetime.fromtimestamp(stamp / 1e9, timezone.utc).isoformat(timespec="milliseconds") if stamp else "unavailable"
                        print(f"\n{elapsed:6.1f} s  {phase} (node time {stamp})", flush=True)
                        last_phase, last_change = phase, elapsed
                    if phase == "Synced" and frontier.get("best_tip"):
                        result.update(outcome="synced", elapsed_s=elapsed, phase_seconds=durations,
                                      height=frontier["best_tip"]["height"], peers=peers)
                        write_json(output / "synced-status.json", status)
                        result["build_env"] = get_json(opener, base + "/build_env")
                        rss = rss_kib(node_pid) if node_pid else None
                        if rss is not None:
                            result["peak_rss_mib"] = max(result["peak_rss_mib"] or 0, rss / 1024)
                        memory = f"{result['peak_rss_mib']:.0f} MiB" if result["peak_rss_mib"] is not None else "unavailable"
                        write_json(output / "result.json", result)
                        print(f"Synced {args.network} at height {result['height']} in {elapsed:.1f} s (peak RSS {memory})", flush=True)
                        break
                if sys.stdout.isatty() or elapsed - last_print >= 5:
                    end = "\r" if sys.stdout.isatty() else "\n"
                    print(f"{elapsed:6.1f} s  {last_phase:<12}", end=end, flush=True)
                    last_print = elapsed
                time.sleep(0.5)
            if args.keep:
                print("Keeping the dashboard open. Press Ctrl-C to stop and remove temporary data.", flush=True)
                while process.poll() is None:
                    check_disk(root, args.min_free_gib)
                    check_disk(output, args.min_free_gib)
                    time.sleep(1)
                raise RuntimeError(f"node exited while keeping dashboard: {process.returncode}")
        return 0
    except (Exception, KeyboardInterrupt) as error:
        if isinstance(error, KeyboardInterrupt) and result["outcome"] == "synced":
            return 0
        result.update(outcome="interrupted" if isinstance(error, KeyboardInterrupt) else "failed",
                      error=str(error) or "interrupted", elapsed_s=time.monotonic() - start if start else None)
        print(f"\nSync failed: {result['error']}", file=sys.stderr, flush=True)
        return 1
    finally:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        cleanup_error = None
        if container:
            error = stop_container(container)
            if error:
                cleanup_error = f"Docker shutdown unconfirmed for {container}; data retained at {data}: {error}"
                result.update(outcome="failed", error=cleanup_error)
        stop(process)
        try:
            if output_created:
                write_json(output / "result.json", result)
        finally:
            if not cleanup_error:
                shutil.rmtree(data)
        if cleanup_error:
            raise RuntimeError(cleanup_error)


def main():
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        return run(arguments())
    except (OSError, RuntimeError) as error:
        print(f"fresh-sync: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
