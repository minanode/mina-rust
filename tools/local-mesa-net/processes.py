"""Track only this experiment's process trees, with Linux PID reuse guards."""

import json
import fcntl
import os
from pathlib import Path
import tempfile


def snapshot():
    result = {}
    for path in Path("/proc").iterdir():
        if not path.name.isdigit():
            continue
        try:
            fields = (path / "stat").read_text().rsplit(")", 1)[1].split()
            result[int(path.name)] = {"ppid": int(fields[1]), "start": fields[19]}
        except (OSError, IndexError):
            pass
    return result


def track(work, namespace=None):
    # The sampler and supervisor both track descendants. Serialize their
    # read/modify/write operations and publish atomically for all other readers.
    with (work / "processes.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        return _track_locked(work, namespace)


def _track_locked(work, namespace):
    path = work / "processes.json"
    known = json.loads(path.read_text()) if path.exists() else {}
    current = snapshot()
    roots = set()
    if namespace is not None:
        for pid in current:
            try:
                if os.readlink(f"/proc/{pid}/ns/net") == namespace:
                    roots.add(pid)
            except OSError:
                pass
    pidfiles = [work / f"{role}.pid" for role in
                ("rust", "ocaml", "rust-launcher", "ocaml-launcher", "sample", "payments", "zkapp-send", "namespace", "reserve")]
    pidfiles.extend(work.glob("worker-*.pid"))
    for pidfile in pidfiles:
        if pidfile.exists():
            pid = int(pidfile.read_text())
            try:
                cmdline = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
                if any(str(work).encode() in arg for arg in cmdline) and pid in current:
                    roots.add(pid)
            except OSError:
                pass
    while True:
        descendants = roots | {pid for pid, row in current.items() if row["ppid"] in roots}
        if descendants == roots:
            break
        roots = descendants
    for pid in roots:
        known[str(pid)] = current[pid]
    with tempfile.NamedTemporaryFile(mode="w", dir=work, prefix="processes.", suffix=".tmp", delete=False) as output:
        json.dump(known, output, indent=2)
        output.write("\n")
        temporary = Path(output.name)
    temporary.replace(path)
    return known
