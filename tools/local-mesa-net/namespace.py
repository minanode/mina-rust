#!/usr/bin/env python3
"""Keep an unprivileged, loopback-only network namespace alive for this run."""

import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--inside", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    work = args.dir.resolve()
    if not args.inside:
        os.execvp("unshare", ["unshare", "--user", "--map-root-user", "--net",
                            sys.executable, str(Path(__file__).resolve()),
                            "--dir", str(work), "--inside"])
    subprocess.run(["ip", "link", "set", "lo", "up"], check=True)
    interfaces = json.loads(subprocess.check_output(["ip", "-json", "address"], text=True))
    routes = json.loads(subprocess.check_output(["ip", "-json", "route", "show", "table", "all"], text=True))
    routes6 = json.loads(subprocess.check_output(["ip", "-6", "-json", "route", "show", "table", "all"], text=True))
    assert [row["ifname"] for row in interfaces] == ["lo"]
    assert all(row.get("dev") == "lo" and row.get("dst") != "default" for row in routes + routes6)
    (work / "namespace.pid").write_text(str(os.getpid()) + "\n")
    (work / "namespace.json").write_text(json.dumps({
        "pid": os.getpid(), "netns": os.readlink("/proc/self/ns/net"),
        "interfaces": interfaces, "routes": routes, "routes6": routes6,
    }, indent=2) + "\n")
    print(f"Private network namespace ready, PID {os.getpid()}", flush=True)
    while True:
        signal.pause()


if __name__ == "__main__":
    main()
