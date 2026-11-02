#!/usr/bin/env python3
"""Install the coturn UDP egress guard; run as root before coturn starts.

Targets coturn 4.18.0 from build-coturn.sh on Ubuntu 24.04, and one public
IPv4 on the host.
Never flush the host ruleset. A failed atomic replacement retains old rules.
"""
import argparse
import ipaddress
from pathlib import Path
import pwd
import subprocess


def settings(path):
    result = {}
    for line in path.read_text().splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            key, _, value = line.partition("=")
            result.setdefault(key.strip(), []).append(value.strip())
    return result


def one(config, key):
    values = config.get(key, [])
    if len(values) != 1:
        raise ValueError(f"require exactly one {key}")
    return values[0]


def rules(turn, relay, uid):
    ip = str(ipaddress.IPv4Address(one(turn, "relay-ip")))
    if one(turn, "listening-ip") != ip or one(turn, "allowed-peer-ip") != ip:
        raise ValueError("only a single local public IPv4 is qualified")
    for flag in ("no-tcp-relay", "no-tls", "use-auth-secret"):
        if one(turn, flag) != "":
            raise ValueError(f"require flag {flag}")
    # Off by default in coturn 4.18: never turned on here.
    for key in ("no-auth", "external-ip", "aux-server", "dtls", "cli", "rfc5780",
                "software-attribute", "mobility", "prometheus", "web-admin",
                "allow-loopback-peers", "server-relay", "alternate-server",
                "tls-alternate-server", "tcp-proxy-port"):
        if key in turn:
            raise ValueError(f"unsupported coturn option {key}")
    for key, most in (("user-quota", 64), ("total-quota", 4096),
                      ("max-allocate-lifetime", 3600)):
        if not 0 < int(one(turn, key)) <= most:
            raise ValueError(f"{key} must be 1 to {most}")
    secret = one(turn, "static-auth-secret")
    if len(secret) < 32 or "<" in secret:
        raise ValueError("install the private TURN secret first")
    listener, low, high = [int(one(turn, k)) for k in
                           ("listening-port", "min-port", "max-port")]
    first, last = map(int, one(relay, "WEBRTC_UDP_PORTS").split("-"))
    if not (0 < listener < 65536 and 0 < low <= high < 65536 and
            0 < first <= last < 65536):
        raise ValueError("invalid UDP port range")
    if low <= listener <= high or first <= listener <= last or not (
            high < first or last < low):
        raise ValueError("listener, allocation and WebRTC ports must be disjoint")
    return f"""add table inet mina_turn
delete table inet mina_turn
table inet mina_turn {{
  chain output {{
    type filter hook output priority -10; policy accept;
    meta skuid {uid} udp sport {listener} counter accept
    meta skuid {uid} ip daddr {ip} udp sport {low}-{high} udp dport {first}-{last} counter accept
    meta skuid {uid} meta l4proto udp counter drop
  }}
}}
"""


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--turn-config", type=Path, default=Path("/etc/turnserver.conf"))
    p.add_argument("--relay-env", type=Path, default=Path("/etc/mina-relay/relay.env"))
    p.add_argument("--user", default="turnserver")
    p.add_argument("--check", action="store_true", help="validate without applying")
    args = p.parse_args()
    batch = rules(settings(args.turn_config), settings(args.relay_env),
                  pwd.getpwnam(args.user).pw_uid)
    subprocess.run(["/usr/sbin/nft", *( ["--check"] if args.check else []),
                    "--file", "-"], input=batch, text=True, check=True)


if __name__ == "__main__":
    main()
