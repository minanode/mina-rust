#!/usr/bin/env python3
"""P5.2e: send a few signature-only zkApp commands into the private network.

After OCaml's best tip reaches --after-height, builds --count zkApp transfers
from keys/receiver to keys/sender.pub (P4.15's shape: a fee payer and a
signed debit, an unauthorised credit; empty memo) with the
`local_zkapp_transfers` example, nonces from the account's next one, and
submits each GraphQL `sendZkapp` request to OCaml (default) or to our node.
Every answer goes to zkapps.jsonl. The key's password is read by the example
from keys/password; nothing secret is printed or logged. Run through
enter.sh, as the other helpers.
"""

import argparse
import datetime
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request

from network_profile import load_profile


def graphql(port, body):
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/graphql", data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--builder", type=Path, required=True,
                        help="the local_zkapp_transfers example binary")
    parser.add_argument("--count", type=int, default=5)
    parser.add_argument("--amount", type=int, default=1_000_000_000,
                        help="nanomina; more than the payer's balance makes a failing command")
    parser.add_argument("--after-height", type=int, default=2)
    parser.add_argument("--to", choices=("ocaml", "rust"), default="ocaml")
    parser.add_argument("--ocaml-port", type=int, default=3670)
    parser.add_argument("--rust-port", type=int, default=3660)
    parser.add_argument("--seconds", type=int, default=3600)
    args = parser.parse_args()
    assert 0 < args.count <= 20
    work = args.dir.resolve()
    network = load_profile(work)
    assert os.readlink("/proc/self/ns/net") == json.loads((work / "namespace.json").read_text())["netns"]
    (work / "zkapp-send.pid").write_text(str(os.getpid()) + "\n")
    payer = (work / "keys/receiver.pub").read_text().strip()
    recipient = (work / "keys/sender.pub").read_text().strip()
    out = (work / "zkapps.jsonl").open("a", buffering=1)

    def log(row):
        row["time"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        out.write(json.dumps(row) + "\n")

    query = ("{ daemonStatus { blockchainLength } "
             f'account(publicKey: "{payer}") {{ nonce }} '
             f'pooledZkappCommands(publicKey: "{payer}") {{ hash }} }}')
    end = time.monotonic() + args.seconds
    while time.monotonic() < end:
        try:
            state = graphql(args.ocaml_port, {"query": query})["data"]
            length = state["daemonStatus"]["blockchainLength"] or 0
            nonce = int(state["account"]["nonce"]) + len(state["pooledZkappCommands"])
        except Exception as error:
            log({"poll_error": str(error)})
            time.sleep(15)
            continue
        if length >= args.after_height:
            break
        time.sleep(15)
    else:
        log({"error": "OCaml's best tip did not reach the height in time"})
        return

    requests = work / "zkapp-requests"
    built = subprocess.run(
        [str(args.builder.resolve()), network, str(work / "keys/receiver"),
         str(work / "keys/password"), recipient, str(nonce), str(args.count), str(requests),
         str(args.amount)],
        env=dict(os.environ, TMPDIR=str(work / "tmp")), capture_output=True, text=True,
        timeout=300)
    log({"built": built.returncode, "stdout": built.stdout.splitlines(),
         "stderr": built.stderr[-2000:], "payer": payer, "recipient": recipient,
         "first_nonce": nonce, "amount": args.amount, "height": length})
    if built.returncode != 0:
        return
    port = args.ocaml_port if args.to == "ocaml" else args.rust_port
    for line in built.stdout.splitlines():
        command_nonce, command_hash, path = line.split(" ", 2)
        body = json.loads(Path(path).read_text())
        try:
            reply = graphql(port, body)
        except Exception as error:
            reply = {"submit_error": str(error)}
        log({"nonce": int(command_nonce), "hash": command_hash, "to": args.to, "reply": reply})


if __name__ == "__main__":
    main()
