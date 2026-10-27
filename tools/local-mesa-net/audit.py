#!/usr/bin/env python3
"""Capture public account balances and OCaml trust status inside the namespace."""

import argparse
import datetime
import json
import os
from pathlib import Path
import re
import subprocess
import urllib.request

from network_profile import load_profile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--rust-port", type=int, default=3660)
    parser.add_argument("--ocaml-port", type=int, default=3670)
    parser.add_argument("--ocaml-client-port", type=int, default=8871)
    args = parser.parse_args()
    assert re.fullmatch(r"[a-z0-9-]+", args.label), "use a simple snapshot label"
    work = args.dir.resolve()
    assert os.readlink("/proc/self/ns/net") == json.loads((work / "namespace.json").read_text())["netns"]
    accounts = {name: (work / "keys" / f"{name}.pub").read_text().strip()
                for name in ("producer", "sender", "receiver", "small")}
    query = "{ " + " ".join(
        f'{name}: account(publicKey: {json.dumps(key)}) {{ publicKey nonce balance {{ total }} }}'
        for name, key in accounts.items()) + " }"
    report = {"time": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "accounts": accounts}
    for name, port in (("rust", args.rust_port), ("ocaml", args.ocaml_port)):
        try:
            request = urllib.request.Request(
                f"http://127.0.0.1:{port}/graphql", data=json.dumps({"query": query}).encode(),
                headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(request, timeout=10) as response:
                report[name] = json.load(response)
        except Exception as error:
            report[name] = {"audit_error": str(error)}
    root = Path(__file__).resolve().parents[2]
    trust = subprocess.run([
        str(root / "scripts/ocaml-mina"), "advanced", "get-trust-status-all",
        "--daemon-port", f"127.0.0.1:{args.ocaml_client_port}", "--json",
    ], env=dict(os.environ, TMPDIR=str(work / "tmp"), MINA_PROFILE=load_profile(work)),
        capture_output=True, text=True, timeout=30)
    report["ocaml_trust"] = {"returncode": trust.returncode,
                             "stdout": trust.stdout, "stderr": trust.stderr}
    path = work / f"audit-{args.label}.json"
    path.write_text(json.dumps(report, indent=2) + "\n")
    print(path)


if __name__ == "__main__":
    main()
