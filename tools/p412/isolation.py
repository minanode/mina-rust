#!/usr/bin/env python3
"""Recheck route isolation from inside the running P4.12 network namespace."""

import argparse
import json
import os
from pathlib import Path
import socket
import subprocess


def routes(*args):
    return json.loads(subprocess.check_output(["ip", *args], text=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", required=True, type=Path)
    parser.add_argument("--label", required=True, choices=("before", "during", "final"))
    args = parser.parse_args()
    run = args.run.resolve()
    namespace = json.loads((run / "namespace.json").read_text())
    assert os.readlink("/proc/self/ns/net") == namespace["netns"]
    interfaces = routes("-json", "address")
    ipv4 = routes("-json", "route", "show", "table", "all")
    ipv6 = routes("-6", "-json", "route", "show", "table", "all")
    names = [row["ifname"] for row in interfaces]
    assert names in (["lo"], ["lo", "p412dummy0"])
    if "p412dummy0" in names:
        dummy = interfaces[1]
        assert [address["local"] for address in dummy["addr_info"]
                if address["family"] == "inet"] == ["10.77.0.1"]
    assert all(row.get("dev") in names and row.get("dst") != "default"
               for row in ipv4 + ipv6)
    assert all(row.get("dev") != "p412dummy0" or row.get("dst") in
               ("10.77.0.0/24", "10.77.0.1", "10.77.0.255", "fe80::/64",
                "ff00::/8") or row.get("dst", "").startswith("fe80::")
               for row in ipv4 + ipv6)
    with socket.socket() as sock:
        sock.settimeout(2)
        try:
            sock.connect(("1.1.1.1", 443))
        except OSError as error:
            external = {"reachable": False, "errno": error.errno,
                        "reason": error.strerror}
        else:
            raise AssertionError("the namespace can reach an external address")
    output = {"netns": namespace["netns"], "interfaces": interfaces,
              "routes": ipv4, "routes6": ipv6, "external_probe": external}
    (run / f"isolation-{args.label}.json").write_text(json.dumps(output, indent=2) + "\n")
    print(f"Isolation confirmed: {args.label}")


if __name__ == "__main__":
    main()
