#!/usr/bin/env python3
"""Sign a payment or a delegation with a throwaway key of this run and submit
it to the Rust node's GraphQL, through the existing wallet CLI. Run through
enter.sh. The key's password goes to the child's environment only."""

import argparse
import datetime
import json
import os
from pathlib import Path
import subprocess


def mina(nanomina):
    """`wallet send` takes MINA: nanomina as an exact decimal string."""
    return f"{nanomina // 10**9}.{nanomina % 10**9:09d}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kind", choices=("payment", "delegation"))
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--mina", type=Path, required=True)
    parser.add_argument("--from", dest="sender", required=True, help="key name under keys/")
    parser.add_argument("--to", required=True, help="key name under keys/")
    parser.add_argument("--amount", type=int, default=1_000_000_000, help="nanomina")
    parser.add_argument("--fee", type=int, default=100_000_000, help="nanomina")
    parser.add_argument("--nonce", type=int, required=True)
    parser.add_argument("--label", required=True)
    args = parser.parse_args()
    work = args.dir.resolve()
    assert os.readlink("/proc/self/ns/net") == json.loads(
        (work / "namespace.json").read_text())["netns"], "run through enter.sh"
    network = json.loads((work / "network.json").read_text())["network"]
    receiver = (work / "keys" / f"{args.to}.pub").read_text().strip()
    command = [str(args.mina.resolve()), "--network", network, "wallet",
               "send" if args.kind == "payment" else "delegate",
               "--from", str(work / "keys" / args.sender), "--to", receiver,
               "--fee", mina(args.fee), "--nonce", str(args.nonce),
               "--memo", f"R43f {args.label}", "--node", "http://127.0.0.1:3660"]
    if args.kind == "payment":
        command[command.index("--fee"):command.index("--fee")] = ["--amount", mina(args.amount)]
    env = {k: v for k, v in os.environ.items() if not k.startswith("MINA_")}
    env["MINA_PRIVKEY_PASS"] = (work / "keys/password").read_text()
    result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=120)
    record = {
        "time": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "kind": args.kind, "from": args.sender, "to": args.to, "nonce": args.nonce,
        "amount": args.amount if args.kind == "payment" else None, "fee": args.fee,
        "returncode": result.returncode, "stdout": result.stdout, "stderr": result.stderr,
    }
    (work / "logs" / f"tx-{args.label}.json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps({k: record[k] for k in ("kind", "from", "to", "nonce", "returncode")}))
    print(result.stdout.strip())
    if result.returncode:
        print(result.stderr.strip())


if __name__ == "__main__":
    main()
