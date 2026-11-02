#!/usr/bin/env python3
"""Loopback-only credential issuer behind nginx. Never log TURN credentials.

Every request from one client address (an IPv4 address or an IPv6 /64) in one
12-hour window gets the same username, so coturn's user-quota caps the TURN
allocations of each address, however many times it asks. The username is
EXPIRY:TOKEN, coturn's REST form: TOKEN is an HMAC of the address and expiry
under the TURN secret, so it does not reveal the address. A credential expires
12 to 24 hours after it is issued.
"""
import base64
import hashlib
import hmac
from http.server import BaseHTTPRequestHandler, HTTPServer
import ipaddress
import json
import os
from pathlib import Path
import time

WINDOW = 12 * 3600


def client_key(client):
    """The address a quota belongs to: IPv4 as is, IPv6 by its /64."""
    ip = ipaddress.ip_address(client)
    if ip.version == 6 and ip.ipv4_mapped:
        ip = ip.ipv4_mapped
    if ip.version == 6:
        return str(ipaddress.IPv6Network((ip, 64), strict=False))
    return str(ip)


def credentials(secret, address, client, now=None):
    now = int(time.time() if now is None else now)
    expiry = (now // WINDOW + 2) * WINDOW
    token = hmac.digest(secret, f"mina-turn-user\0{expiry}\0{client_key(client)}".encode(),
                        hashlib.sha256)[:12].hex()
    username = f"{expiry}:{token}"
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
            # The service listens on loopback only. nginx replaces the header
            # with the visitor's address; a local request without it counts
            # as the loopback address.
            forwarded = self.headers.get_all("X-Forwarded-For") or []
            client = (forwarded[-1].split(",")[-1].strip() if forwarded
                      else self.client_address[0])
            try:
                payload = json.dumps(credentials(secret, address, client)).encode()
            except ValueError:
                self.send_error(400)
                return
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
