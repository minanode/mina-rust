#!/usr/bin/env python3
"""On the TURN host, prove allowed delivery and blocked same-IP UDP targets.

Run before starting the node, or select a currently unused WebRTC UDP port.
Uses short-lived credentials from the loopback issuer; never logs credentials.
Both test receiver ports must be free. Does not change any firewall rules.

It also checks, on its own allocations only: the lifetime cap, that TCP
allocations and other peer addresses are refused, that an unsigned attribute
after MESSAGE-INTEGRITY is ignored (CVE-2026-68554), and with --user-quota
that one username gets that many allocations and no more.
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
from urllib.request import Request, urlopen

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


def lifetime(wire):
    fields = parse(wire)[1]
    return struct.unpack("!I", fields[0x0D])[0] if 0x0D in fields else None


def error(wire):
    fields = parse(wire)[1]
    return fields[9][2] * 100 + fields[9][3] if 9 in fields else None


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
    p.add_argument("--max-lifetime", type=int, default=600)
    p.add_argument("--user-quota", type=int, default=0,
                   help="also check this per-username allocation quota; 0 skips it")
    args = p.parse_args()
    assert len({args.turn_port, args.allowed_port, args.blocked_port}) == 3
    if args.secret_file:
        username = f"{int(time.time()) + 600}:egress-check"
        password = base64.b64encode(hmac.digest(args.secret_file.read_bytes().strip(),
                                               username.encode(), "sha1")).decode()
    else:
        # A random benchmarking address (198.18.0.0/15) gets this run its own
        # username, so earlier runs' allocations do not count against its
        # quota and visitors' usernames are untouched.
        client = f"198.{18 + secrets.randbelow(2)}.{secrets.randbelow(256)}.{1 + secrets.randbelow(254)}"
        request = Request(args.issuer, headers={"X-Forwarded-For": client})
        with urlopen(request, timeout=10) as response:
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
        client.send(packet(3, attr(0x19, b"\x06\0\0\0") + auth, key))
        assert error(client.recv(4096)) == 442, "TCP allocation not refused"
        client.send(packet(3, allocation + attr(0x0D, struct.pack("!I", 3600)) + auth, key))
        wire = client.recv(4096)
        assert parse(wire)[0] == 0x103, f"authenticated allocation failed ({error(wire)})"
        extra = []
        try:
            assert 0 < lifetime(wire) <= args.max_lifetime, "allocation lifetime above the cap"
            # A signed Refresh for 600 s with an unsigned LIFETIME 0 after
            # MESSAGE-INTEGRITY: a patched coturn ignores the trailing 0.
            wire = packet(4, attr(0x0D, struct.pack("!I", 600)) + auth, key)
            wire += attr(0x0D, struct.pack("!I", 0))
            client.send(wire[:2] + struct.pack("!H", len(wire) - 20) + wire[4:])
            reply = client.recv(4096)
            assert parse(reply)[0] != 0x104 or lifetime(reply) != 0, \
                "unsigned attribute after MESSAGE-INTEGRITY was used"
            for peer in ("192.0.2.1", "127.0.0.1"):
                client.send(packet(8, attr(0x12, address(peer, args.allowed_port)) + auth, key))
                assert error(client.recv(4096)) == 403, f"permission to {peer} not refused"
            for n in range(args.user_quota):
                s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                s.bind((args.client_ip, 0))
                s.connect((args.ip, args.turn_port))
                s.settimeout(2)
                # Each 5-tuple gets its own nonce.
                s.send(packet(3, allocation))
                own = parse(s.recv(4096))[1][0x15]
                own_auth = attr(6, username.encode()) + attr(0x14, realm) + attr(0x15, own)
                extra.append((s, own_auth))
                s.send(packet(3, allocation + own_auth, key))
                reply = s.recv(4096)
                # The main allocation is the first of the quota.
                if n < args.user_quota - 1:
                    assert parse(reply)[0] == 0x103, f"allocation {n + 2} within the quota failed"
                else:
                    assert error(reply) == 486, "allocation past the user quota accepted"
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
            for s, a in [(client, auth), *extra]:
                s.send(packet(4, attr(0x0D, struct.pack("!I", 0)) + a, key))
                try:
                    s.recv(4096)
                except socket.timeout:
                    pass
                if s is not client:
                    s.close()
    result = {"anonymous": "rejected", "TCP allocation": "refused",
              "authenticated": "accepted", "lifetime": "capped",
              "unsigned trailing attribute": "ignored", "other peers": "refused",
              "WebRTC-range UDP": "delivered", args.target + " UDP target": "blocked"}
    if args.user_quota:
        result["user quota"] = args.user_quota
    print(json.dumps(result))


if __name__ == "__main__":
    main()
