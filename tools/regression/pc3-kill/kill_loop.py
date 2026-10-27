"""PC3-KILL: SIGKILL a devnet follower at named points of a three-ledger
bootstrap, a frontier-store checkpoint or a refreeze, restart it on the same
work directory, and check what the restart gives.

  python3 -I tools/regression/pc3-kill/kill_loop.py --run DIR --bin MINA \
      --http 4160 --p2p 9360 --ref 3540 --cases staking,between --per-case 5

Each iteration is two starts of one `systemd-run --user` unit on the same work
directory (DIR/home), through tools/regression/devnet/node.sh:

1. a kill start, killed at its case's point (below), by a kill point
   (MINA_KILL_AT, `ledger::database::disk::kill_point`) or by
   `systemctl --user kill -s SIGKILL` timed from the log's `kind:` events or
   from files appearing on disk;
2. a check start, default flags, no reset. It restores from the frontier
   store if the kill start left a checkpoint, else bootstraps. Once synced:
   - its best tip must be within a block of the reference's (--ref, read
     only: /status and GraphQL);
   - two blocks below its tip and one above its root must have the
     reference's state hash, snarked and staged ledger hashes, and staking
     and next epoch ledger hashes;
   - if it bootstrapped, its first checkpoint must list the staking and next
     epoch ledgers by those hashes; if it restored, the load has rehashed
     every ledger against its stored hash, and the next checkpoint is
     --checkpoint-interval away;
   - three producers' `delegators` must equal the reference's (the staking
     ledger's contents);
   - `ledger-accounts/` must hold only files the process has open, and the
     sizes of the work directory are recorded, to see growth across loops.
   It is then stopped with SIGTERM.

Cases (--cases), each kill start with --frontier-store-reset unless noted:

- staking: a random fraction of the staking ledger sync's expected length
  after it starts (each phase's expected length is the shortest of the last
  three bootstraps', from their log timestamps);
- between: the staking ledger synced, the next epoch's not started:
  `sync-copy:1` (the next epoch ledger just made a mask) or 0 to 50 ms
  after TransitionFrontierSyncLedgerStakingSuccess;
- next: in the next epoch ledger's sync, a random fraction of its expected
  length after TransitionFrontierSyncLedgerNextEpochPending;
- root: the same for the root ledger (RootPending to RootSuccess);
- root-commit: the bootstrap commit, once its ledgers are frozen or detached:
  `commit-frozen:1`, `commit-end:1`, `epoch-detach:1`, or 0 to 100 ms after
  the commit's last ledger line (the next epoch ledger's detach, or the
  root's when it holds a sixty-fourth of the accounts);
- checkpoint: the first checkpoint after the sync, a random fraction of its
  expected length after its first ledger, staged or parts file appears in the
  frontier store;
- refreeze: no reset, `--ledger-refreeze-slots 0` (every commit writes every
  ledger again, PC3b); once synced, 0 to 400 ms after a new accounts file
  appears.

Writes DIR/kill-loop.jsonl, one line per start, and the logs as
DIR/node-NN-{kill,check}.log.
"""

import argparse
import datetime
import json
import os
import random
import re
import subprocess
import time
import urllib.request

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "../../.."))

EV = {
    "start": r'kind: "NodeStart"',
    "staking_start": r"no ledger to copy for sync|kind: \"TransitionFrontierSyncBestTipUpdate\"",
    "staking_done": r'kind: "TransitionFrontierSyncLedgerStakingSuccess"',
    "next_start": r'kind: "TransitionFrontierSyncLedgerNextEpochPending"',
    "next_done": r'kind: "TransitionFrontierSyncLedgerNextEpochSuccess"',
    "root_start": r'kind: "TransitionFrontierSyncLedgerRootPending"',
    "root_done": r'kind: "TransitionFrontierSyncLedgerRootSuccess"',
    "freeze_epoch": r'kind: "LedgerService::freeze_epoch_ledger"',
    "detach_epoch": r'kind: "LedgerService::detach_epoch_ledger"',
    "detach_root": r'kind: "LedgerService::detach_root_snarked_ledger"',
    # The bootstrap commit's last ledger line: the next epoch ledger's detach,
    # or the root's when it holds a sixty-fourth of the accounts.
    "commit_ledgers": r'kind: "LedgerService::detach_(epoch_ledger|root_snarked_ledger)"',
    "freeze_root": r'kind: "LedgerService::freeze_root_snarked_ledger"',
    "synced": r'kind: "NodeSynced"',
    "restored": r'kind: "PersistRestored"',
    "checkpoint": r'kind: "PersistCheckpoint"',
    "kill_point": r"MINA_KILL_AT: SIGKILL at",
    "panic": r"panicked",
}
# The order phases come in, to name where a kill landed.
PHASES = [
    ("start", "starting"),
    ("restored", "restored"),
    ("staking_start", "staking sync"),
    ("staking_done", "between staking and next"),
    ("next_start", "next epoch sync"),
    ("next_done", "between next and root"),
    ("root_start", "root sync"),
    ("root_done", "blocks and commit"),
    ("commit_ledgers", "in the bootstrap commit, ledgers detached"),
    ("synced", "synced"),
    ("checkpoint", "after the first checkpoint"),
]

ROOT_COMMIT = ["commit-frozen:1", "ext:commit_ledgers", "commit-end:1", "epoch-detach:1", "commit-frozen:1",
               "ext:commit_ledgers", "commit-end:1"]
BETWEEN = ["sync-copy:1", "ext:staking_done"]


class Log:
    """A node's log, read as it grows; the first time each event is seen."""

    def __init__(self, path):
        self.path, self.pos, self.tail, self.seen, self.lines = path, 0, "", {}, []

    def poll(self):
        try:
            with open(self.path, errors="replace") as f:
                f.seek(self.pos)
                chunk = f.read()
                self.pos = f.tell()
        except FileNotFoundError:
            return
        if not chunk:
            return
        text = self.tail + chunk
        lines = text.split("\n")
        self.tail = lines.pop()
        now = time.time()
        for line in lines:
            for name, pattern in EV.items():
                if name not in self.seen and re.search(pattern, line):
                    self.seen[name] = now
            if re.search(EV["checkpoint"] + "|" + EV["freeze_epoch"] + "|" + EV["freeze_root"] + "|" +
                         EV["detach_epoch"] + "|" + EV["detach_root"] + "|" + EV["kill_point"] + "|" +
                         EV["panic"], line):
                self.lines.append(line)

    def has(self, name):
        self.poll()
        return name in self.seen

    def text(self):
        try:
            return open(self.path, errors="replace").read()
        except FileNotFoundError:
            return ""


def systemctl(*args, check=False):
    return subprocess.run(["systemctl", "--user", *args], capture_output=True, text=True, check=check)


def unit_active(unit):
    return systemctl("is-active", unit).stdout.strip() in ("active", "activating", "deactivating")


def main_pid(unit):
    try:
        return int(systemctl("show", "-p", "MainPID", "--value", unit).stdout.strip() or 0)
    except ValueError:
        return 0


def wait_for(cond, timeout, step=0.05):
    end = time.time() + timeout
    while time.time() < end:
        v = cond()
        if v:
            return v
        time.sleep(step)
    return None


def http_json(port, path, body=None, timeout=20):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request("http://127.0.0.1:%d%s" % (port, path), data=data,
                                 headers={"content-type": "application/json"})
    return json.load(urllib.request.urlopen(req, timeout=timeout))


def gql(port, query, timeout=30):
    return http_json(port, "/graphql", {"query": query}, timeout)


def status(port):
    try:
        s = http_json(port, "/status", timeout=10)["transition_frontier"]
        return {"phase": s["sync"]["phase"], "tip": s["best_tip"]["height"] if s["best_tip"] else None,
                "root": s["root"]["height"] if s["root"] else None}
    except Exception:
        return None


BLOCK = ("{ block(height: %d) { stateHash protocolState { blockchainState { snarkedLedgerHash stagedLedgerHash } "
         "consensusState { stakingEpochData { ledger { hash } } nextEpochData { ledger { hash } } } } } }")


def block(port, height):
    try:
        b = gql(port, BLOCK % height)["data"]["block"]
        bs, cs = b["protocolState"]["blockchainState"], b["protocolState"]["consensusState"]
        return {"state": b["stateHash"], "snarked": bs["snarkedLedgerHash"], "staged": bs["stagedLedgerHash"],
                "staking": cs["stakingEpochData"]["ledger"]["hash"], "next": cs["nextEpochData"]["ledger"]["hash"]}
    except Exception:
        return None


def delegators(port, key):
    q = '{ account(publicKey: "%s") { delegators { publicKey balance { total } } } }' % key
    try:
        d = gql(port, q, timeout=120)["data"]["account"]["delegators"]
        return sorted((x["publicKey"], x["balance"]["total"]) for x in d)
    except Exception as e:
        return "error: %s" % e


def producers(port, n=3):
    try:
        chain = gql(port, "{ bestChain(maxLength: 30) { creator } }")["data"]["bestChain"]
    except Exception:
        return []
    seen = []
    for b in chain:
        if b["creator"] not in seen:
            seen.append(b["creator"])
    return seen[:n]


def du(path):
    total = 0
    for root, _, names in os.walk(path):
        for n in names:
            try:
                total += os.lstat(os.path.join(root, n)).st_blocks * 512
            except FileNotFoundError:
                pass
    return total


class Loop:
    def __init__(self, a):
        self.a = a
        self.run = os.path.abspath(a.run)
        os.makedirs(self.run, exist_ok=True)
        self.home = os.path.join(self.run, "home")
        self.accounts = os.path.join(self.home, "ledger-accounts")
        self.unit = a.unit
        self.rng = random.Random(a.seed)
        self.out = open(os.path.join(self.run, "kill-loop.jsonl"), "a")
        # Expected lengths, seconds, refined from the check starts' logs.
        self.est = {"staking": 30.0, "next": 1.0, "root": 12.0, "checkpoint": 2.0}
        self.seen_lengths = {}

    # Starting and stopping.

    def start(self, name, kill_at=None, reset=False, extra=""):
        path = os.path.join(self.run, "node-%s.log" % name)
        args = (extra + (" --frontier-store-reset" if reset else "")).strip()
        cmd = ["systemd-run", "--user", "--unit", self.unit, "--collect", "--quiet",
               "-p", "StandardOutput=append:" + path, "-p", "StandardError=append:" + path,
               "-p", "KillMode=mixed", "-p", "TimeoutStopSec=60",
               "-E", "MINA=" + self.a.bin, "-E", "EXTRA_ARGS=" + args]
        if kill_at:
            cmd += ["-E", "MINA_KILL_AT=" + kill_at]
        cmd += [os.path.join(REPO, "tools/regression/devnet/node.sh"), self.run, str(self.a.http), str(self.a.p2p)]
        if unit_active(self.unit):
            raise SystemExit("unit %s is already running" % self.unit)
        subprocess.run(cmd, check=True)
        log = Log(path)
        wait_for(lambda: main_pid(self.unit) or not unit_active(self.unit), 30, 0.2)
        return log

    def kill(self):
        systemctl("kill", "--signal=SIGKILL", self.unit)
        wait_for(lambda: not unit_active(self.unit), 60, 0.1)

    def stop(self):
        systemctl("stop", self.unit)
        wait_for(lambda: not unit_active(self.unit), 90, 0.2)

    # What is on disk.

    def accounts_files(self):
        try:
            return {e.name: e.stat().st_size for e in os.scandir(self.accounts) if e.name.endswith(".accounts")}
        except FileNotFoundError:
            return {}

    def frontier_dir(self):
        for root, dirs, _ in os.walk(self.home):
            if os.path.basename(root) == "frontier":
                return root
            if root.count(os.sep) - self.home.count(os.sep) > 2:
                dirs[:] = []
        return None

    def audit(self):
        """Files in ledger-accounts the process has no descriptor for are
        orphans; descriptors to removed files are space du can't see."""
        pid = main_pid(self.unit)
        open_files, deleted = set(), 0
        if pid:
            fd_dir = "/proc/%d/fd" % pid
            try:
                for fd in os.listdir(fd_dir):
                    try:
                        target = os.readlink(os.path.join(fd_dir, fd))
                    except OSError:
                        continue
                    if target.startswith(self.accounts + "/"):
                        if target.endswith(" (deleted)"):
                            try:
                                deleted += os.stat(os.path.join(fd_dir, fd)).st_size
                            except OSError:
                                pass
                        else:
                            open_files.add(os.path.basename(target))
            except OSError:
                pass
        files = self.accounts_files()
        frontier = self.frontier_dir()
        return {
            "accounts_files": files,
            "orphans": sorted(set(files) - open_files),
            "deleted_open_bytes": deleted,
            "accounts_bytes": du(self.accounts) if os.path.isdir(self.accounts) else 0,
            "frontier_bytes": du(frontier) if frontier else 0,
            "home_bytes": du(self.home),
        }

    # The kill start.

    def where(self, log):
        landed = None
        for name, label in PHASES:
            if log.has(name):
                landed = label
        return landed

    def kill_start(self, i, case, variant):
        name = "%02d-kill" % i
        row = {"iteration": i, "start": "kill", "case": case, "variant": variant,
               "start_utc": time.strftime("%FT%TZ", time.gmtime())}
        point = variant if variant and not variant.startswith("ext") and ":" in variant else None
        reset = case != "refreeze"
        extra = "--ledger-refreeze-slots 0" if case == "refreeze" else ""
        log = self.start(name, kill_at=point, reset=reset, extra=extra)
        dead = lambda: not unit_active(self.unit)
        t0 = time.time()

        def after(event, delay, limit=900):
            seen = wait_for(lambda: log.has(event) or log.has("synced") or dead(), limit)
            if dead():
                return False
            if not log.has(event):
                row["not_reached"] = event
                return False
            time.sleep(max(0.0, log.seen[event] + delay - time.time()))
            return True

        delay = None
        if point:
            # The node kills itself; past the sync and 180 s more, it missed.
            wait_for(lambda: log.has("synced") or dead(), 900, 0.2)
            if not dead() and not wait_for(dead, 180, 0.2):
                row["not_reached"] = point
        elif case == "staking":
            delay = self.rng.uniform(0.05, 0.9) * self.est["staking"]
            after("staking_start", delay)
        elif case == "between":
            delay = self.rng.uniform(0, 0.05)
            after("staking_done", delay)
        elif case == "next":
            delay = self.rng.uniform(0.05, 0.9) * self.est["next"]
            after("next_start", delay)
        elif case == "root":
            delay = self.rng.uniform(0.05, 0.9) * self.est["root"]
            after("root_start", delay)
        elif case == "root-commit":
            delay = self.rng.uniform(0, 0.1)
            after("commit_ledgers", delay)
        elif case == "checkpoint":
            if after("synced", 0):
                frontier = None
                before = set()

                def new_file():
                    nonlocal frontier, before
                    frontier = frontier or self.frontier_dir()
                    if not frontier:
                        return None
                    names = {n for n in os.listdir(frontier) if re.match(r"(ledger|staged|parts)-", n)}
                    return (names - before) or None
                frontier = self.frontier_dir()
                before = {n for n in os.listdir(frontier) if re.match(r"(ledger|staged|parts)-", n)} if frontier else set()
                first = wait_for(lambda: new_file() or log.has("checkpoint") or dead(), 600, 0.02)
                if first and not dead() and not log.has("checkpoint"):
                    delay = self.rng.uniform(0, 0.4) * self.est["checkpoint"]
                    time.sleep(delay)
                    row["checkpoint_files_at_kill"] = {n: os.path.getsize(os.path.join(frontier, n))
                                                       for n in os.listdir(frontier)
                                                       if re.match(r"(ledger|staged|parts)-", n)}
                else:
                    row["not_reached"] = "checkpoint start"
        elif case == "refreeze":
            if after("synced", 0):
                synced_files = set(self.accounts_files())
                new = wait_for(lambda: (set(self.accounts_files()) - synced_files) or dead(), 900, 0.02)
                if new and not dead():
                    delay = self.rng.uniform(0, 0.4)
                    time.sleep(delay)
                    row["new_file"] = sorted(new)
                else:
                    row["not_reached"] = "a new accounts file"
        if delay is not None:
            row["delay_ms"] = round(delay * 1000)
        log.poll()
        row["landed"] = self.where(log)
        row["checkpoint_written"] = log.has("checkpoint")
        row["files_at_kill"] = self.accounts_files()
        if not dead():
            self.kill()
        log.poll()
        row["kill_point"] = bool(log.has("kill_point"))
        row["landed"] = self.where(log)
        row["seconds"] = round(time.time() - t0, 1)
        row["panic"] = bool(re.search(EV["panic"], log.text()))
        row["events"] = log.lines[-12:]
        self.emit(row)

    # The check start.

    def check_start(self, i, case):
        name = "%02d-check" % i
        row = {"iteration": i, "start": "check", "case": case, "start_utc": time.strftime("%FT%TZ", time.gmtime())}
        before = set(self.accounts_files())
        row["files_before"] = sorted(before)
        t0 = time.time()
        log = self.start(name)
        dead = lambda: not unit_active(self.unit)
        wait_for(lambda: log.has("synced") or dead(), self.a.sync_timeout, 0.5)
        row["restored"] = log.has("restored")
        row["synced_s"] = round(log.seen["synced"] - t0, 1) if log.has("synced") else None
        if dead():
            row["died"] = True
        elif not log.has("synced"):
            row["hang"] = "not synced in %d s" % self.a.sync_timeout
        else:
            self.learn(log, t0)
            row["previous_files_removed"] = not (before & set(self.accounts_files()))
            row["hashes"] = self.check_hashes()
            if row["restored"]:
                # The next checkpoint is --checkpoint-interval away (30
                # minutes by default); the load rehashed every ledger it
                # restored against its stored hash.
                row["checkpoint"] = {"written": False, "match": True, "restore_checked": True}
            else:
                wait_for(lambda: log.has("checkpoint") or dead(), 600, 0.5)
                row["checkpoint"] = self.checkpoint_ledgers(log, row["hashes"])
            row["delegators"] = self.check_delegators()
            row["audit"] = self.audit()
        row["panic"] = bool(re.search(EV["panic"], log.text()))
        if not dead():
            self.stop()
        row["seconds"] = round(time.time() - t0, 1)
        row["ok"] = bool(row.get("synced_s") and row.get("hashes", {}).get("match")
                         and row.get("checkpoint", {}).get("match")
                         and row.get("delegators", {}).get("match")
                         and not row.get("audit", {}).get("orphans") and not row["panic"])
        self.emit(row)
        return row

    def learn(self, log, t0):
        """Phase lengths from a bootstrap's log timestamps, for the next
        kill's delays."""
        at = {}
        for line in log.text().split("\n"):
            for name in ("staking_start", "staking_done", "next_start", "next_done", "root_start", "root_done"):
                if name not in at and re.search(EV[name], line):
                    m = re.match(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z", line)
                    if m:
                        at[name] = datetime.datetime.fromisoformat(m.group(1)).timestamp()
        # The shortest of the last three, as a sync's length varies with its
        # peers (the root's from 12 s to 3 minutes), and a kill past the end
        # misses the phase.
        for phase, floor in (("staking", 2.0), ("next", 0.2), ("root", 1.0)):
            if phase + "_start" in at and phase + "_done" in at:
                seen = self.seen_lengths.setdefault(phase, [])
                seen.append(max(floor, at[phase + "_done"] - at[phase + "_start"]))
                self.est[phase] = min(seen[-3:])

    def check_hashes(self):
        def caught_up():
            c, r = status(self.a.http), status(self.a.ref)
            return c and r and c["phase"] == "Synced" and c["tip"] and r["tip"] and c["tip"] >= r["tip"] - 1 and c
        c = wait_for(caught_up, 900, 2)
        if not c:
            return {"caught_up": False, "match": False}
        result = {"caught_up": True, "tip": c["tip"], "root": c["root"], "heights": {}}
        for h in [c["tip"] - 2, c["tip"] - 3, c["root"] + 1]:
            a = b = None
            for _ in range(12):
                a, b = block(self.a.http, h), block(self.a.ref, h)
                if a and b and a == b:
                    break
                time.sleep(10)
            result["heights"][h] = {"match": bool(a and b and a == b), "node": a, "ref": b}
        result["match"] = all(v["match"] for v in result["heights"].values())
        return result

    def checkpoint_ledgers(self, log, hashes):
        lines = [l for l in log.text().split("\n") if re.search(EV["checkpoint"], l)]
        if not lines:
            return {"written": False, "match": False}
        # The root's own block isn't served, so the block above it gives the
        # epoch ledgers (the same within an epoch); its snarked ledger is the
        # root's unless a ledger proof landed in it, so it is recorded only.
        listed = lines[0]
        root = hashes.get("heights", {}).get(hashes.get("root", 0) + 1) or {}
        node = root.get("node") or {}
        want = {k: node.get(k) for k in ("staking", "next")}
        found = {k: bool(v and v in listed) for k, v in want.items()}
        snarked_listed = bool(node.get("snarked") and node["snarked"] in listed)
        el = re.search(r"elapsed_ms: ([\d.]+)", "\n".join(l for l in log.text().split("\n")
                                                          if "TransitionFrontierPersistCheckpointSuccess" in l))
        if el:
            self.est["checkpoint"] = max(0.5, float(el.group(1)) / 1000)
        return {"written": True, "found": found, "match": all(found.values()), "snarked_listed": snarked_listed,
                "ledgers": len(re.findall(r"\bj[1-9A-HJ-NP-Za-km-z]{50}\b", listed)),
                "elapsed_ms": float(el.group(1)) if el else None}

    def check_delegators(self):
        keys = producers(self.a.ref)
        same = {k: delegators(self.a.http, k) == delegators(self.a.ref, k) for k in keys}
        return {"keys": len(keys), "same": sum(same.values()), "match": bool(keys) and all(same.values())}

    def emit(self, row):
        row["end_utc"] = time.strftime("%FT%TZ", time.gmtime())
        self.out.write(json.dumps(row) + "\n")
        self.out.flush()
        brief = {k: row.get(k) for k in ("iteration", "start", "case", "variant", "landed", "restored", "synced_s", "ok",
                                         "not_reached", "panic")}
        print(json.dumps(brief), flush=True)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--run", required=True)
    p.add_argument("--bin", required=True)
    p.add_argument("--http", type=int, required=True)
    p.add_argument("--p2p", type=int, required=True)
    p.add_argument("--ref", type=int, required=True, help="the reference node's HTTP port, read only")
    p.add_argument("--cases", required=True)
    p.add_argument("--per-case", type=int, default=5)
    p.add_argument("--seed", type=int, default=0x9c3d)
    p.add_argument("--unit", required=True)
    p.add_argument("--first", type=int, default=1, help="the first iteration's number, to continue a run")
    p.add_argument("--sync-timeout", type=int, default=1200)
    a = p.parse_args()
    a.bin = os.path.abspath(a.bin)
    loop = Loop(a)
    cases = a.cases.split(",")
    variants = {"root-commit": ROOT_COMMIT, "between": BETWEEN}
    plan = []
    for n in range(a.per_case):
        for c in cases:
            v = variants.get(c)
            plan.append((c, v[n % len(v)] if v else "ext"))
    for i, (case, variant) in enumerate(plan, start=a.first):
        loop.kill_start(i, case, variant)
        time.sleep(2)
        loop.check_start(i, case)
        time.sleep(2)


if __name__ == "__main__":
    main()
