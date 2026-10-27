#!/usr/bin/env python3
"""Launch an explicitly allocated private node with a bounded shared deadline."""

import argparse
import json
import os
from pathlib import Path
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("role", choices=("rust", "ocaml"))
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--mina", type=Path, required=True)
    parser.add_argument("--blobs", type=Path, required=True)
    parser.add_argument("--private-isolated-allocation", action="store_true", required=True)
    parser.add_argument("--relay-only", action="store_true")
    parser.add_argument("--max-seconds", type=int, default=2700)
    parser.add_argument("--ocaml-snark-worker", action="store_true")
    parser.add_argument("--ocaml-producer", action="store_true")
    parser.add_argument("--rust-snark-worker-fee")
    args = parser.parse_args()
    assert not args.relay_only or args.role == "rust"
    assert 0 < args.max_seconds <= 14400, "private allocation cannot exceed four hours"
    os.umask(0o077)
    work = args.dir.resolve()
    root = Path(__file__).resolve().parents[2]
    matched = json.loads((work / "identity-match.json").read_text())
    assert matched["ocaml_genesis_proof"] == "generated with full proofs"
    if args.role == "rust":
        assert matched["ocaml_graphql_genesis_hash_confirmed"]
    deadline = work / "node-deadline.json"
    if not deadline.exists():
        now = time.time()
        deadline.write_text(json.dumps({"started_unix": now, "stop_unix": now + args.max_seconds,
                                        "max_seconds": args.max_seconds}, indent=2) + "\n")
    remaining = int(json.loads(deadline.read_text())["stop_unix"] - time.time())
    assert 0 < remaining <= args.max_seconds, "private run deadline expired or invalid"
    (work / f"{args.role}-launcher.pid").write_text(str(os.getpid()) + "\n")
    command = ["timeout", "--foreground", "--signal=TERM", "--kill-after=10s", str(remaining),
               "bash", str(root / "tools/local-mesa-net/enter.sh"), str(work),
               "python3", str(root / "tools/local-mesa-net/node.py"), args.role,
               "--dir", str(work), "--mina", str(args.mina.resolve()),
               "--blobs", str(args.blobs.resolve()), "--private-isolated-allocation"]
    if args.relay_only:
        command.append("--relay-only")
    if args.ocaml_snark_worker:
        command.append("--ocaml-snark-worker")
    if args.ocaml_producer:
        command.append("--ocaml-producer")
    if args.rust_snark_worker_fee:
        command.extend(("--rust-snark-worker-fee", args.rust_snark_worker_fee))
    os.execvp(command[0], command)


if __name__ == "__main__":
    main()
