#!/usr/bin/env python3
"""On the TURN host, prove allowed delivery and blocked same-IP UDP targets.

Run before starting the node, or select a currently unused WebRTC UDP port.
Uses short-lived credentials from the loopback issuer; never logs credentials.
Both test receiver ports must be free. Does not change any firewall rules.
"""
import argparse
import base64
import hashlib
import hmac
import json
from pathlib import Path
import secrets
import socket
import struct
import time
from urllib.request import urlopen

COOKIE = 0x2112A442


def attr(kind, value):
    return struct.pack("!HH", kind, len(value)) + value + b"\0" * (-len(value) % 4)


def packet(kind, body, key=None):
    header = struct.pack("!HHI", kind, len(body) + (24 if key else 0), COOKIE)
    wire = header + secrets.token_bytes(12) + body
    return wire + attr(8, hmac.digest(key, wire, "sha1")) if key else wire


def parse(wire):
    fields, offset = {}, 20
    while offset + 4 <= len(wire):
        kind, length = struct.unpack("!HH", wire[offset:offset + 4])
        fields[kind] = wire[offset + 4:offset + 4 + length]
        offset += 4 + (length + 3) // 4 * 4
    return struct.unpack("!H", wire[:2])[0], fields


def address(ip, port):
    return b"\0\1" + struct.pack("!HI", port ^ (COOKIE >> 16),
        int.from_bytes(socket.inet_aton(ip), "big") ^ COOKIE)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--ip", required=True, help="local public relay/TURN IPv4")
    p.add_argument("--turn-port", type=int, default=3478)
    p.add_argument("--allowed-port", type=int, default=40000)
    p.add_argument("--blocked-port", type=int, default=39999)
    p.add_argument("--target", choices=("unrelated", "listener"), default="unrelated")
    p.add_argument("--issuer", default="http://127.0.0.1:3002/ice-servers.json")
    p.add_argument("--secret-file", type=Path, help="isolated fixture only; never prints it")
    p.add_argument("--client-ip", default="0.0.0.0")
    p.add_argument("--client-port", type=int, default=0)
    args = p.parse_args()
    assert len({args.turn_port, args.allowed_port, args.blocked_port}) == 3
    if args.secret_file:
        username = f"{int(time.time()) + 600}:egress-check"
        password = base64.b64encode(hmac.digest(args.secret_file.read_bytes().strip(),
                                               username.encode(), "sha1")).decode()
    else:
        with urlopen(args.issuer, timeout=10) as response:
            servers = json.load(response)
        server = next(s for s in servers if "credential" in s)
        username, password = server["username"], server["credential"]
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client, \
            socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as allowed, \
            socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as blocked:
        allowed.bind((args.ip, args.allowed_port))
        blocked.bind((args.ip, args.blocked_port))
        allowed.settimeout(2)
        blocked.settimeout(2)
        client.bind((args.client_ip, args.client_port))
        client.connect((args.ip, args.turn_port))
        client.settimeout(2)
        allocation = attr(0x19, b"\x11\0\0\0")
        client.send(packet(3, allocation))
        code, fields = parse(client.recv(4096))
        assert code == 0x113 and fields[9][2:4] == b"\4\1", "anonymous allocation accepted"
        realm, nonce = fields[0x14], fields[0x15]
        key = hashlib.md5(username.encode() + b":" + realm + b":" + password.encode()).digest()
        auth = attr(6, username.encode()) + attr(0x14, realm) + attr(0x15, nonce)
        client.send(packet(3, allocation + auth, key))
        assert parse(client.recv(4096))[0] == 0x103, "authenticated allocation failed"
        try:
            client.send(packet(8, attr(0x12, address(args.ip, args.allowed_port)) + auth, key))
            assert parse(client.recv(4096))[0] == 0x108, "same-IP permission failed"
            marker = secrets.token_bytes(32)

            def send(port, payload):
                client.send(packet(0x16, attr(0x12, address(args.ip, port)) + attr(0x13, payload)))

            send(args.allowed_port, marker)
            assert allowed.recv(4096) == marker, "WebRTC-range delivery failed"
            if args.target == "unrelated":
                send(args.blocked_port, marker)
                receiver = blocked
            else:
                # Unguarded, this Binding request produces a DATA indication
                # via the allocation. Use a fresh allocation for each target:
                # a firewall EPERM may close coturn's allocation socket.
                send(args.turn_port, packet(1, b""))
                receiver = client
            try:
                receiver.recv(4096)
            except socket.timeout:
                pass
            else:
                raise AssertionError(f"TURN reached {args.target} UDP target")
        finally:
            client.send(packet(4, attr(0x0D, struct.pack("!I", 0)) + auth, key))
            assert parse(client.recv(4096))[0] == 0x104, "allocation cleanup failed"
    print(json.dumps({"anonymous": "rejected", "authenticated": "accepted",
                      "WebRTC-range UDP": "delivered", args.target + " UDP target": "blocked"}))


if __name__ == "__main__":
    main()
