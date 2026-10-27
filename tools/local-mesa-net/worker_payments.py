#!/usr/bin/env python3
"""P5.2: keep payments flowing into an OCaml producer's transaction pool.

Payments from keys/sender to keys/receiver.pub are signed by the existing
wallet CLI and sent, unchanged, to OCaml's GraphQL `sendPayment`. The CLI
posts its signed mutation to a local capture shim instead of a node: the shim
answers the CLI's status preflight (synced, this profile's network) and keeps
the mutation. No signature or OCaml validation is bypassed; OCaml's answer to
each submission is recorded in payments.jsonl.

After OCaml's first block, one burst (default 126) fills the first scan-state
tree, then the sender's pending commands in OCaml's pool are kept near a small
target, so the single worker's proofs, not a backlog of trees, set the pace.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
import datetime
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import re
import subprocess
import threading
import time
import urllib.request

from network_profile import load_profile


def mina(nanomina):
    """`wallet send` takes MINA: nanomina as an exact decimal string."""
    return f"{nanomina // 10**9}.{nanomina % 10**9:09d}"


def graphql(port, query):
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/graphql", data=json.dumps({"query": query}).encode(),
        headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--mina", type=Path, required=True)
    parser.add_argument("--ocaml-port", type=int, default=3670)
    parser.add_argument("--capture-port", type=int, default=3662)
    parser.add_argument("--initial", type=int, default=126)
    parser.add_argument("--target", type=int, default=12)
    parser.add_argument("--amount", type=int, default=1_000_000)
    parser.add_argument("--fee", type=int, default=100_000_000)
    parser.add_argument("--interval", type=int, default=20)
    parser.add_argument("--seconds", type=int, default=14400)
    args = parser.parse_args()
    assert 0 < args.initial <= 128 and 0 < args.target <= 64
    work = args.dir.resolve()
    network = load_profile(work)
    assert os.readlink("/proc/self/ns/net") == json.loads((work / "namespace.json").read_text())["netns"]
    (work / "payments.pid").write_text(str(os.getpid()) + "\n")
    sender = (work / "keys/sender.pub").read_text().strip()
    receiver = (work / "keys/receiver.pub").read_text().strip()
    captured = {}
    lock = threading.Lock()

    class Capture(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            query = body["query"]
            if "sendPayment(" in query:
                nonce = int(re.search(r'nonce:\s*"(\d+)"', query).group(1))
                with lock:
                    captured[nonce] = query
                reply = {"errors": [{"message": "captured for OCaml submission"}]}
            else:
                # The CLI's preflight: only status fields, never a node's data.
                reply = {"data": {"syncStatus": "SYNCED", "networkID": f"mina:{network}"}}
            data = json.dumps(reply).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    server = HTTPServer(("127.0.0.1", args.capture_port), Capture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    env = dict(os.environ, MINA_PRIVKEY_PASS=(work / "keys/password").read_text(),
               TMPDIR=str(work / "tmp"))
    log = (work / "logs/worker-payments-sign.log").open("a")
    out = (work / "payments.jsonl").open("a", buffering=1)

    def sign(nonce):
        subprocess.run([
            str(args.mina.resolve()), "--network", network, "wallet", "send",
            "--from", str(work / "keys/sender"), "--to", receiver,
            "--amount", mina(args.amount), "--fee", mina(args.fee),
            "--memo", "P5.2 private payment", "--nonce", str(nonce),
            "--node", f"http://127.0.0.1:{args.capture_port}",
        ], env=env, stdout=log, stderr=subprocess.STDOUT, timeout=120)

    def send(nonces):
        with ThreadPoolExecutor(max_workers=4) as executor:
            list(executor.map(sign, nonces))
        for nonce in nonces:
            with lock:
                mutation = captured.pop(nonce, None)
            row = {"time": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                   "nonce": nonce}
            if mutation is None:
                row["error"] = "wallet did not sign"
            else:
                try:
                    reply = graphql(args.ocaml_port, mutation)
                    row["ocaml"] = reply
                except Exception as error:
                    row["error"] = str(error)
            out.write(json.dumps(row) + "\n")

    state_query = ("{ daemonStatus { blockchainLength } "
                   f'account(publicKey: "{sender}") {{ nonce }} '
                   f'pooledUserCommands(publicKey: "{sender}") {{ nonce }} }}')
    end = time.monotonic() + args.seconds
    burst_done = False
    next_nonce = 0
    while time.monotonic() < end:
        try:
            state = graphql(args.ocaml_port, state_query)["data"]
            # During bootstrap OCaml has no frontier: no length, no account.
            length = state["daemonStatus"]["blockchainLength"] or 0
            account_nonce = int(state["account"]["nonce"])
            pending = len(state["pooledUserCommands"])
        except Exception as error:
            out.write(json.dumps({"poll_error": str(error)}) + "\n")
            time.sleep(args.interval)
            continue
        next_nonce = max(next_nonce, account_nonce + pending)
        if not burst_done:
            if length >= 2:
                send(list(range(next_nonce, next_nonce + args.initial)))
                next_nonce += args.initial
                burst_done = True
        elif pending < args.target:
            count = args.target - pending
            send(list(range(next_nonce, next_nonce + count)))
            next_nonce += count
        time.sleep(args.interval)


if __name__ == "__main__":
    main()
