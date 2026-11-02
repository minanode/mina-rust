#!/usr/bin/env python3
"""Loopback-only credential issuer behind nginx. Never log TURN credentials."""
import base64
import hashlib
import hmac
from http.server import BaseHTTPRequestHandler, HTTPServer
import ipaddress
import json
import os
from pathlib import Path
import secrets
import time


def credentials(secret, address, now=None):
    expiry = int(time.time() if now is None else now) + 86400
    username = f"{expiry}:{secrets.token_hex(12)}"
    password = base64.b64encode(hmac.digest(secret, username.encode(), hashlib.sha1)).decode()
    return [{"urls": [f"stun:{address}:3478"]},
            {"urls": [f"turn:{address}:3478?transport=udp",
                      f"turn:{address}:3478?transport=tcp"],
             "username": username, "credential": password}]


def main():
    address = str(ipaddress.IPv4Address(os.environ["TURN_IPV4"]))
    secret = (Path(os.environ["CREDENTIALS_DIRECTORY"]) / "turn_secret").read_bytes().strip()
    if len(secret) < 32:
        raise SystemExit("TURN secret must contain at least 32 bytes")

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path != "/ice-servers.json":
                self.send_error(404)
                return
            payload = json.dumps(credentials(secret, address)).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("Access-Control-Allow-Origin", "https://minaweb.org")
            self.send_header("Cross-Origin-Resource-Policy", "cross-origin")
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, *_):
            pass

    HTTPServer(("127.0.0.1", int(os.environ.get("CREDENTIAL_PORT", "3002"))), Handler).serve_forever()


if __name__ == "__main__":
    main()
