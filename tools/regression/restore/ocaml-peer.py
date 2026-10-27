#!/usr/bin/env python3
"""Supervise the I19 isolated OCaml daemon during a separate restart.sh run.

Prepare WORK using scripts/ocaml-isolated-daemon.sh's documented inputs. Supply
MINA_LIBP2P_PASS privately in the environment. --peer must identify only the test
Rust node. This supervisor holds a node slot, samples OCaml trust/peer state,
records descendants across all OCaml threads and stops only its own processes.
"""

import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import time
import urllib.request


def birth(pid):
    return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--lock", type=Path, required=True)
    parser.add_argument("--peer", required=True)
    parser.add_argument("--run", default="replay")
    parser.add_argument("--slot", type=int, default=24)
    parser.add_argument("--duration", type=int, default=1800)
    parser.add_argument("--seed", action="store_true",
                        help="allow startup while the sole test peer is stopped")
    parser.add_argument("--discovery-interval", default="10m",
                        help="OCaml's discovery retry interval; 5s suits restart tests")
    args = parser.parse_args()
    if args.slot < 0 or args.slot == 14:
        parser.error("choose a nonnegative, unprotected slot")
    args.work = args.work.resolve()
    evidence = args.work / (args.run + "-evidence")
    evidence.mkdir()
    http, p2p = 3400 + 10 * args.slot, 8600 + 10 * args.slot
    for port in (http, p2p, p2p + 1):
        with socket.socket() as sock:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            sock.bind(("0.0.0.0", port))
    repo = Path(__file__).resolve().parents[3]
    env = {**os.environ, "WORK": str(args.work), "RUN": args.run,
           "PEER": args.peer, "LIBP2P_PORT": str(p2p), "CLIENT_PORT": str(p2p + 1),
           "REST_PORT": str(http),
           "MINA_LIBP2P_DISCOVERY_MIN_INTERVAL": args.discovery_interval,
           "LOCK_WAIT": "0"}
    owned = {}

    def remember(pid):
        try:
            owned[pid] = birth(pid)
            # Children can be forked by any thread, not just the main thread.
            for task in Path(f"/proc/{pid}/task").iterdir():
                try:
                    for child in (task / "children").read_text().split():
                        remember(int(child))
                except FileNotFoundError:
                    pass
        except (FileNotFoundError, ProcessLookupError):
            pass

    def interrupted(signum, frame):
        raise KeyboardInterrupt(f"signal {signum}")

    signal.signal(signal.SIGTERM, interrupted)
    query = """{ syncStatus daemonStatus { blockchainLength highestBlockLengthReceived
    stateHash peers {peerId host libp2pPort} }
    trustStatusAll {peerId ipAddr trust bannedStatus} }"""
    request = urllib.request.Request(f"http://127.0.0.1:{http}/graphql",
                                    data=json.dumps({"query": query}).encode(),
                                    headers={"Content-Type": "application/json"})
    expected_peer = args.peer.rsplit("/p2p/", 1)[-1]
    result = {"peer": args.peer, "utc_start": time.time()}
    with (evidence / "supervisor.log").open("w") as log:
        proc = subprocess.Popen([str(args.lock.resolve()), "node", "--",
                                 str(repo / "scripts/ocaml-isolated-daemon.sh"),
                                 "--min-connections", "1", "--max-connections", "1"] +
                                (["--seed"] if args.seed else []),
                                env=env, stdout=log, stderr=subprocess.STDOUT)
        result["pid"] = proc.pid
        (evidence / "supervisor.pid").write_text(str(os.getpid()))
        start, next_sample = time.monotonic(), 0
        try:
            while time.monotonic() - start < args.duration:
                remember(proc.pid)
                (evidence / "owned-pids.json").write_text(json.dumps(owned))
                if proc.poll() is not None:
                    raise RuntimeError(f"OCaml exited {proc.returncode}")
                if (evidence / "stop").exists():
                    break
                if time.monotonic() >= next_sample:
                    next_sample = time.monotonic() + 5
                    try:
                        with urllib.request.urlopen(request, timeout=2) as response:
                            sample = json.load(response)
                    except Exception as error:
                        sample = {"error": str(error)}
                    sample["utc"] = time.time()
                    with (evidence / "status.jsonl").open("a") as out:
                        out.write(json.dumps(sample) + "\n")
                    peers = (sample.get("data") or {}).get("daemonStatus") or {}
                    unexpected = {p["peerId"] for p in peers.get("peers", [])
                                  if p["peerId"] != expected_peer}
                    if unexpected:
                        raise RuntimeError(f"OCaml isolation failed: unexpected peers {sorted(unexpected)}")
                time.sleep(0.5)
        except BaseException as error:
            result["error"] = f"{type(error).__name__}: {error}"
            raise
        finally:
            remember(proc.pid)
            for sig in (signal.SIGTERM, signal.SIGKILL):
                for pid, original_birth in owned.items():
                    try:
                        if birth(pid) == original_birth:
                            os.kill(pid, sig)
                    except (FileNotFoundError, ProcessLookupError):
                        pass
                if sig == signal.SIGTERM:
                    time.sleep(3)
            proc.wait()
            result.update(owned=owned, exit=proc.returncode, utc_stop=time.time())
            (evidence / "result.json").write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
