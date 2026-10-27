#!/usr/bin/env python3
"""Reserve two shared node slots with a bounded wait for the second slot."""

import argparse
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def stop(child):
    if child.poll() is None:
        child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lease", type=Path, required=True)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--mina", type=Path, required=True)
    parser.add_argument("--blobs", type=Path, required=True)
    parser.add_argument("--partial-wait", type=int, default=0,
                        help="seconds to hold the first slot while waiting for the second (0-120)")
    parser.add_argument("--rust-slot", type=int, default=26)
    parser.add_argument("--ocaml-slot", type=int, default=27)
    args = parser.parse_args()
    assert 0 <= args.partial_wait <= 120
    os.umask(0o077)
    lease = args.lease.resolve()
    lease.mkdir(mode=0o700)  # Fresh markers only.
    (lease / "reserve.pid").write_text(str(os.getpid()) + "\n")
    work = args.dir.resolve()
    root = Path(__file__).resolve().parents[2]
    common = subprocess.check_output([
        "git", "-C", str(root), "rev-parse", "--path-format=absolute", "--git-common-dir"
    ], text=True).strip()
    lock = Path(common).parent / "tools/regression/lock.sh"
    env = dict(os.environ, LOCK_WAIT="0")
    assert env.get("MINA_NO_LOCK") != "1", "shared locks are required"
    children = []

    def interrupted(_signal, _frame):
        raise SystemExit(1)

    signal.signal(signal.SIGTERM, interrupted)
    try:
        while True:
            children = []
            for role in ("ocaml", "rust"):
                ticket = lease / role
                held = ticket.with_suffix(".held")
                held.unlink(missing_ok=True)
                command = [str(lock), "node", "--", sys.executable,
                           str(root / "tools/local-mesa-net/hold.py"),
                           "--ticket", str(ticket), "--log", str(work / f"logs/{role}-netns.log"),
                           "--", "bash", str(root / "tools/local-mesa-net/enter.sh"), str(work),
                           sys.executable, str(root / "tools/local-mesa-net/node.py"), role,
                           "--dir", str(work), "--mina", str(args.mina.resolve()),
                           "--blobs", str(args.blobs.resolve()),
                           "--rust-slot", str(args.rust_slot), "--ocaml-slot", str(args.ocaml_slot)]
                with (lease / f"{role}-reservation.log").open("a") as log:
                    wait = args.partial_wait if role == "rust" else 0
                    child = subprocess.Popen(command, env=dict(env, LOCK_WAIT=str(wait)),
                                             stdout=log, stderr=subprocess.STDOUT)
                children.append(child)
                (lease / f"{role}.pid").write_text(str(child.pid) + "\n")
                deadline = time.monotonic() + wait + 5
                while not held.exists() and child.poll() is None and time.monotonic() < deadline:
                    time.sleep(0.05)
                if not held.exists() or child.poll() is not None:
                    break
            if len(children) == 2 and all(child.poll() is None for child in children):
                if all((lease / f"{role}.held").exists() for role in ("ocaml", "rust")):
                    print(f"Both shared node slots reserved; prepare {work}", flush=True)
                    for child in children:
                        child.wait()
                    return
            for child in children:
                stop(child)
            for role in ("ocaml", "rust"):
                (lease / f"{role}.held").unlink(missing_ok=True)
            print("Pair unavailable; released any partial reservation", flush=True)
            time.sleep(30)
    finally:
        for child in children:
            stop(child)


if __name__ == "__main__":
    main()
