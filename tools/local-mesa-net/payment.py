#!/usr/bin/env python3
"""Sign with the existing wallet CLI, then submit through the existing REST API.

The original P4.6c baseline's GraphQL converter rejected nonce zero. Retain its
submission path: capture the CLI's signed mutation locally and submit the same
fields to /send-payment instead (also supported after P6's GraphQL fix).
This changes neither the signature nor node validation. Only generated keys in
this run directory may be used.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import re
import subprocess
import threading
import urllib.error
import urllib.request

from network_profile import load_profile


def mina(nanomina):
    """`wallet send` takes MINA: nanomina as an exact decimal string."""
    return f"{nanomina // 10**9}.{nanomina % 10**9:09d}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--mina", type=Path, required=True)
    parser.add_argument("--rust-port", type=int, default=3660)
    parser.add_argument("--nonce", type=int, default=0)
    parser.add_argument("--count", type=int, default=1)
    parser.add_argument("--amount", type=int, default=1000000000)
    parser.add_argument("--fee", type=int, default=100000000)
    parser.add_argument("--label", default="payment")
    args = parser.parse_args()
    assert re.fullmatch(r"[a-z0-9-]+", args.label)
    assert 0 < args.count <= 128 and args.nonce >= 0
    work = args.dir.resolve()
    network = load_profile(work)
    assert os.readlink("/proc/self/ns/net") == json.loads((work / "namespace.json").read_text())["netns"]
    base = f"http://127.0.0.1:{args.rust_port}"
    captured = []

    def post(path, body):
        request = urllib.request.Request(base + path, data=json.dumps(body).encode(),
                                         headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)

    class Capture(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if "sendPayment(" in body["query"]:
                captured.append(body["query"])
                reply = {"errors": [{"message": "Captured locally for REST submission"}]}
            else:
                reply = post("/graphql", body)
            data = json.dumps(reply).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    receiver = (work / "keys/receiver.pub").read_text().strip()
    env = dict(os.environ, MINA_PRIVKEY_PASS=(work / "keys/password").read_text())
    server = HTTPServer(("127.0.0.1", args.rust_port + 1), Capture)
    thread = threading.Thread(target=server.serve_forever)
    thread.start()
    try:
        with (work / f"logs/{args.label}-sign.log").open("w") as log:
            def sign(nonce):
                result = subprocess.run([
                    str(args.mina.resolve()), "--network", network, "wallet", "send",
                    "--from", str(work / "keys/sender"), "--to", receiver,
                    "--amount", mina(args.amount), "--fee", mina(args.fee),
                    "--memo", "Private closure payment", "--nonce", str(nonce),
                    "--node", f"http://127.0.0.1:{args.rust_port + 1}",
                ], env=env, stdout=log, stderr=subprocess.STDOUT, timeout=60)
                assert result.returncode == 1, "wallet signing/capture failed"
            with ThreadPoolExecutor(max_workers=min(4, args.count)) as executor:
                list(executor.map(sign, range(args.nonce, args.nonce + args.count)))
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
    assert len(captured) == args.count, "wallet signing/capture count mismatch"
    payments = []
    for mutation in captured:
        fields = dict(re.findall(r'(\w+):\s*"([^"]*)"', mutation))
        assert fields["from"] == (work / "keys/sender.pub").read_text().strip()
        assert fields["to"] == receiver
        payment = {key: fields[key] for key in ("from", "to", "memo")}
        payment.update({key: int(fields[key]) for key in ("amount", "fee", "nonce")})
        payment.update(valid_until=int(fields["validUntil"]),
                       signature_field=fields["field"], signature_scalar=fields["scalar"])
        payments.append(payment)
    payments.sort(key=lambda payment: payment["nonce"])
    assert [payment["nonce"] for payment in payments] == list(range(args.nonce, args.nonce + args.count))
    (work / f"{args.label}-request.json").write_text(json.dumps(payments[0] if args.count == 1 else payments, indent=2) + "\n")
    try:
        reply = post("/send-payment", payments)
    except urllib.error.HTTPError as error:
        # Retain the API's actual reason, rather than just a status code.
        (work / f"{args.label}-response.json").write_text(json.dumps({
            "http_status": error.code, "body": error.read().decode(),
        }, indent=2) + "\n")
        raise
    (work / f"{args.label}-response.json").write_text(json.dumps(reply, indent=2) + "\n")
    print(json.dumps(reply, indent=2))


if __name__ == "__main__":
    main()
