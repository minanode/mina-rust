"""Run one live restore experiment; mutate only a new, unlocked private copy.

Every launch takes the repository node lock. SIGKILL triggers are observed
events, not sleeps. Results preserve raw logs, status samples and checker output.
The checker is offline (node stopped) and uses full Merkle/index verification.
"""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import time
import urllib.request


CHAIN = "0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886"
DEVNET_CHAIN = "ebfce0d570bc22eb041e1a7b0a46bbc3ed5d0a1030ef2ab5e36eef908b93eba8"
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def copy_store(source, dest):
    # Never copy node homes, identities or other private material. Lock the
    # source for the entire copy so a live store cannot become a torn fixture.
    with (source / "LOCK").open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        dest.mkdir(parents=True)
        for p in source.iterdir():
            if p.name in ("MANIFEST-A", "MANIFEST-B", "ATTEMPTS") or p.name.startswith(
                ("block-", "ledger-", "staged-", "parts-")
            ):
                assert p.is_file() and not p.is_symlink(), p
                shutil.copyfile(p, dest / p.name)  # no hardlinks for fault injection


def generation(path):
    data = path.read_bytes()
    # Bin_prot unsigned nat0 at the start of the framed manifest body.
    tag = data[16]
    if tag < 128:
        return tag
    return int.from_bytes(data[17:17 + {254: 2, 253: 4, 252: 8}[tag]], "little")


def mutate(store, kind):
    if kind == "manifest":
        p = max(store.glob("MANIFEST-*"), key=generation)
    else:
        pattern = {"block": "block-*", "ledger": "ledger-*.full",
                   "staged": "staged-*", "parts": "parts-*",
                   "truncate": "block-*"}[kind]
        # Require the damaged file to be referenced by both generations when
        # possible. A separate manifest variant tests fallback.
        manifests = [m.read_bytes() for m in store.glob("MANIFEST-*")]
        choices = sorted(store.glob(pattern))
        p = next((p for p in choices if all(p.name.encode() in m for m in manifests)), choices[0])
    before = p.read_bytes()
    offset = len(before) // 2
    after = before[:offset] if kind == "truncate" else (
        before[:offset] + bytes([before[offset] ^ 0x80]) + before[offset + 1:]
    )
    p.write_bytes(after)
    return {"file": p.name, "offset": offset, "before_bytes": len(before),
            "after_bytes": len(after), "before_sha256": hashlib.sha256(before).hexdigest(),
            "after_sha256": hashlib.sha256(after).hexdigest()}


class Run:
    def __init__(self, args, directory, store):
        self.args, self.directory, self.store = args, directory, store
        directory.mkdir()
        self.log = (directory / "node.log").open("w")
        self.samples = (directory / "status.jsonl").open("w")
        self.reader = (directory / "node.log").open()
        self.text = ""
        self.started = time.monotonic()
        self.wait_started = self.started
        self.acquired = False
        self.first_sync = None
        self.at_tip = False
        self.next_sample = 0
        for port in (args.http, args.p2p):
            with socket.socket() as sock:
                sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                sock.bind(("0.0.0.0", port))
        command = [str(args.lock), "node", "--", str(args.binary), "--network", args.network,
                   "node", "-d", str(directory / "home"), "-p", str(args.http),
                   "--libp2p-port", str(args.p2p),
                   "--genesis-config" if args.compact else "--config", str(args.config),
                   "--chain-id", args.chain, "--peer-list-url",
                   f"https://storage.googleapis.com/seed-lists/{args.network}_seeds.txt",
                   "--disable-filesystem-logging", "--frontier-store",
                   "--frontier-store-dir", str(store.parent.parent),
                   "--checkpoint-interval", str(args.checkpoint_interval)] + args.node_arg
        self.proc = subprocess.Popen(command, stdout=self.log, stderr=subprocess.STDOUT,
                                     env={**os.environ, "BEST_TIP_REQUEST_INTERVAL": "80",
                                          "LOCK_WAIT": str(getattr(args, "lock_wait", 600))})
        save(directory / "launch.json", {"pid": self.proc.pid, "utc": time.time(), "command": command})

    def poll(self):
        self.text += ANSI.sub("", self.reader.read())
        if self.proc.poll() is not None:
            raise RuntimeError(f"node exited {self.proc.returncode}; see {self.directory}")
        if shutil.disk_usage(self.directory).free < 55 * 1024**3:
            raise RuntimeError("free disk below 55 GB")
        now = time.monotonic()
        if not self.acquired:
            if "lock.sh: took node-" not in self.text:
                if now - self.wait_started > getattr(self.args, "lock_wait", 600) + 30:
                    raise TimeoutError("node lock wait expired")
                return
            self.acquired = True
            self.started = now
            save(self.directory / "lock-acquired.json", {
                "utc": time.time(), "wait_seconds": now - self.wait_started,
                "timing": "observed lock acquisition; nominal 100ms polling cadence"})
        if now >= self.next_sample:
            self.next_sample = now + 1
            row = {"seconds": now - self.started, "utc": time.time()}
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{self.args.http}/status", timeout=1) as r:
                    status = json.load(r)
                tf = status.get("transition_frontier") or {}
                tip, sync = tf.get("best_tip") or {}, tf.get("sync") or {}
                peers = [p for p in status.get("peers", []) if p.get("connection_status") == "Connected"]
                heights = [p["best_tip_height"] for p in peers if p.get("best_tip_height")]
                row.update(status=sync.get("status"), tip=tip.get("height"), hash=tip.get("hash"),
                           net=max(heights, default=0), reporting=len(heights), peers=len(peers))
                self.at_tip = (row["status"] == "Synced" and len(heights) >= 3 and
                               row["tip"] is not None and row["tip"] >= max(heights))
                if self.at_tip and self.first_sync is None:
                    self.first_sync = row
                    save(self.directory / "synced.json", row)
            except Exception as error:
                self.at_tip = False
                row["error"] = type(error).__name__
            self.samples.write(json.dumps(row) + "\n")
            self.samples.flush()
        if now - self.started > self.args.timeout:
            raise TimeoutError(f"timeout after {self.args.timeout}s")

    def wait_synced(self):
        while self.first_sync is None:
            self.poll()
            time.sleep(0.1)
        # Wait for a complete persisted checkpoint before the offline check.
        until = time.monotonic() + self.args.hold
        while time.monotonic() < until or not self.at_tip:
            self.poll()
            time.sleep(0.1)

    def stop(self, sig=signal.SIGTERM):
        if self.proc.poll() is None:
            self.proc.send_signal(sig)
            try:
                self.proc.wait(timeout=30)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.text += ANSI.sub("", self.reader.read())
        save(self.directory / "stop.json", {"signal": sig, "exit": self.proc.returncode, "utc": time.time()})
        (self.directory / "persist.log").write_text("\n".join(
            line for line in self.text.splitlines() if "Persist" in line or "panicked" in line
        ) + "\n")
        self.reader.close()
        self.log.close()
        self.samples.close()

    def summary(self):
        if "frontier store reset: another chain id or genesis" in self.text:
            outcome = "foreign store deleted; bootstrap"
        elif "frontier store deleted, the node bootstraps" in self.text:
            outcome = "store deleted; bootstrap"
        elif "frontier store not restored, the node bootstraps" in self.text:
            outcome = "ledgers only; bootstrap"
        elif "PersistRestored" in self.text:
            outcome = "restored"
            opened = re.search(r"frontier store opened generation (\d+)", self.text)
            loaded = re.search(r'frontier store loaded:.*?generation: (\d+)', self.text)
            if ((opened and loaded and int(loaded[1]) < int(opened[1])) or
                    re.search(r'slots:.*Invalid', self.text)):
                outcome = "previous generation restored"
        elif "nothing to restore, the node bootstraps" in self.text:
            outcome = "empty store; bootstrap"
        else:
            outcome = "interrupted or incomplete"
        return {"synced": self.first_sync, "outcome": outcome,
                "panic_logged": "panicked at" in self.text}


def check(args, store, path):
    with path.open("w") as out:
        result = subprocess.run([str(args.binary), "--network", args.network, "node",
                                 "frontier-store-check", str(store)], stdout=out,
                                stderr=subprocess.STDOUT, timeout=180)
    return {"exit": result.returncode, "verdict": path.read_text().splitlines()[-1:]}


def main():
    def interrupted(signum, frame):
        raise KeyboardInterrupt(f"received signal {signum}")

    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "lock", "config", "out"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--source", type=Path, help="stopped frontier directory; omitted for fresh bootstrap")
    parser.add_argument("--network", choices=["mainnet", "devnet"], default="mainnet")
    parser.add_argument("--compact", action="store_true", help="config is a --genesis-config file")
    parser.add_argument("--variant", choices=["fresh", "clean", "kill-normal", "kill-checkpoint",
                        "kill-replay", "block", "ledger", "staged", "parts", "manifest", "truncate", "other-network"], required=True)
    parser.add_argument("--http", type=int, default=3630)
    parser.add_argument("--p2p", type=int, default=8830)
    parser.add_argument("--checkpoint-interval", type=int, default=1800)
    parser.add_argument("--timeout", type=int, default=1200)
    parser.add_argument("--lock-wait", type=int, default=600)
    parser.add_argument("--hold", type=int, default=5)
    parser.add_argument("--node-arg", action="append", default=[])
    args = parser.parse_args()
    if args.variant == "other-network" and args.source is None:
        parser.error("other-network requires a real stopped foreign-network store")
    args.chain = CHAIN if args.network == "mainnet" else DEVNET_CHAIN
    if args.http < 3400 or (args.http - 3400) % 10 or args.p2p != args.http + 5200:
        parser.error("use one slot's HTTP 3400+10N and libp2p 8600+10N")
    if args.http == 3540:
        parser.error("the protected devnet relay's slot is not available")
    for name in ("binary", "lock", "config", "out", "source"):
        if getattr(args, name) is not None:
            setattr(args, name, getattr(args, name).resolve())
    args.out.mkdir(parents=True, exist_ok=False)
    store = args.out / "data" / args.chain / "frontier"
    result = {"variant": args.variant, "source": str(args.source), "runs": []}
    if args.source:
        copy_store(args.source, store)
    else:
        store.mkdir(parents=True)
    if args.variant in ("block", "ledger", "staged", "parts", "manifest", "truncate"):
        result["mutation"] = mutate(store, args.variant)
    run = None
    try:
        if args.variant in ("clean", "kill-normal", "kill-checkpoint", "kill-replay"):
            run = Run(args, args.out / "before", store)
            if args.variant in ("clean", "kill-normal"):
                run.wait_synced()
            elif args.variant == "kill-replay":
                while True:
                    run.poll()
                    if "frontier store loaded: root" in run.text and "PersistRestored" not in run.text:
                        # RestoreInit enters BlocksPending. Kill after that
                        # action, while the replay commit has not happened.
                        if "TransitionFrontierSyncRestoreInit" in run.text:
                            time.sleep(0.2)
                            run.poll()
                            if "PersistRestored" in run.text:
                                raise RuntimeError("replay finished before kill; retry required")
                            result["trigger"] = {"seconds": time.monotonic() - run.started,
                                                 "evidence": run.text.splitlines()[-20:]}
                            break
                    time.sleep(0.01)
            else:
                original = {p.name for p in store.iterdir()}
                while True:
                    run.poll()
                    # A newly created nonempty checkpoint payload, while still
                    # open for write by this exact child, proves mid-write.
                    for fd in Path(f"/proc/{run.proc.pid}/fd").iterdir():
                        try:
                            p = Path(os.readlink(fd))
                            if (p.parent == store and p.name not in original and
                                    p.name.startswith(("ledger-", "parts-", "staged-")) and p.stat().st_size > 0):
                                info = Path(str(fd).replace("/fd/", "/fdinfo/")).read_text()
                                flags = int(re.search(r"flags:\s+([0-7]+)", info)[1], 8)
                                if flags & 3:
                                    result["trigger"] = {"file": p.name, "size": p.stat().st_size,
                                                         "fdinfo": info, "seconds": time.monotonic() - run.started}
                                    break
                        except (FileNotFoundError, ProcessLookupError):
                            continue
                    if "trigger" in result:
                        break
                    time.sleep(0.005)
            sig = signal.SIGTERM if args.variant == "clean" else signal.SIGKILL
            run.stop(sig)
            result["runs"].append({"name": "before", **run.summary()})
            run = None
            result["interrupted_store"] = check(args, store, args.out / "interrupted-check.log")
        run = Run(args, args.out / "after", store)
        run.wait_synced()
        run.stop()
        result["runs"].append({"name": "after", **run.summary()})
        run = None
        result["checker"] = check(args, store, args.out / "check.log")
        if result["checker"]["exit"] or not any(
                "with a checkpoint" in line for line in result["checker"]["verdict"]):
            raise RuntimeError("final store lacks a valid checkpoint; see check.log")
        if result["runs"][-1]["panic_logged"]:
            raise RuntimeError("node logged a panic; see after/node.log")
    except BaseException as error:
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        if run:
            run.stop()
        save(args.out / "result.json", result)
        print(json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
