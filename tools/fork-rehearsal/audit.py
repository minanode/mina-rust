#!/usr/bin/env python3
"""Every rehearsal account's balance, nonce and delegate on both nodes, over
GraphQL, in `audit-<label>.json`. Run through enter.sh."""

import argparse
import datetime
import json
import os
from pathlib import Path
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--label", required=True)
    args = parser.parse_args()
    work = args.dir.resolve()
    assert os.readlink("/proc/self/ns/net") == json.loads(
        (work / "namespace.json").read_text())["netns"], "run through enter.sh"
    accounts = json.loads((work / "run.json").read_text())["accounts"]
    query = "{ " + " ".join(
        f'{name.replace("-", "_")}: account(publicKey: {json.dumps(pk)}) '
        "{ nonce balance { total } delegate }"
        for name, pk in accounts.items()) + " }"
    report = {"time": datetime.datetime.now(datetime.timezone.utc).isoformat()}
    for node, port in (("rust", 3660), ("ocaml", 3670)):
        request = urllib.request.Request(
            f"http://127.0.0.1:{port}/graphql", data=json.dumps({"query": query}).encode(),
            headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=20) as response:
                report[node] = json.load(response)
        except Exception as error:
            report[node] = {"audit_error": str(error)}
    report["equal"] = report["rust"].get("data") == report["ocaml"].get("data")
    (work / f"audit-{args.label}.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"equal": report["equal"], "rust": report["rust"].get("data")}, indent=1))


if __name__ == "__main__":
    main()
