#!/usr/bin/env python3
"""External launch gate: HTTPS/headers, STUN, native peers, direct and TURN sync.

CHROME and PUPPETEER_DIR select the browser. RELAY_HTTP is an SSH-forwarded
operator API; never expose that API publicly. Evidence goes to the output dir.
"""
import argparse
import json
import os
from pathlib import Path
import re
import secrets
import socket
import struct
import subprocess
import time
import hashlib
import ipaddress
from urllib.error import HTTPError
from urllib.request import Request, urlopen
from urllib.parse import urljoin

HERE = Path(__file__).resolve().parent


def get(url, method="GET", headers=None):
    try:
        with urlopen(Request(url, method=method, headers=headers or {}), timeout=15) as r:
            return r.status, r.headers, r.read()
    except HTTPError as e:
        return e.code, e.headers, e.read()


def check_assets(build, origin):
    assets = [(build["wasm"]["path"], "application/wasm", None)]
    for name, source in (("block", "blockchain"), ("transaction", "transaction")):
        path = HERE.parents[2] / "crates/ledger/src/proofs/data" / (
            f"devnet_{source}_verifier_index.json")
        expected = hashlib.sha256(path.read_bytes()).digest()
        assets.append((build["circuitsUrl"].rstrip("/") +
                       f"/4.0.2devnet/{name}_verifier_index.postcard",
                       "application/octet-stream", expected))
    for url, mime, expected in assets:
        for request_headers in ({}, {"Origin": origin}):
            code, h, body = get(urljoin(origin + "/", url), "GET" if expected else "HEAD", request_headers)
            assert code == 200 and h.get_content_type() == mime, "asset status or MIME"
            assert h.get("Access-Control-Allow-Origin") == origin, "asset CORS"
            assert h.get("Cross-Origin-Resource-Policy") == "cross-origin", "asset CORP"
            if expected:
                assert len(body) > 32 and body[:32] == expected, "missing or stale verifier cache"


def valid_ice_url(value):
    # Same supported RFC 7064/7065 profile and shared cases as the browser.
    if not isinstance(value, str):
        return False
    match = re.fullmatch(r"(stun|stuns|turn|turns):([^?]+)(?:\?transport=(udp|tcp))?", value)
    if not match or (match[1].startswith("stun") and match[3]):
        return False
    authority = re.fullmatch(r"(\[[0-9a-fA-F:.]+\]|[A-Za-z0-9.-]+)(?::([0-9]{1,5}))?", match[2])
    if not authority:
        return False
    host, port = authority.groups()
    if port and not 1 <= int(port) <= 65535:
        return False
    if host.startswith("["):
        try:
            ipaddress.IPv6Address(host[1:-1])
        except ValueError:
            return False
    else:
        name = host.removesuffix(".")
        if len(name) > 253 or not all(re.fullmatch(
                r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?", label)
                for label in name.split(".")):
            return False
    return True


def check_credentials(ice, now=None):
    now = time.time() if now is None else now
    assert isinstance(ice, list), "invalid ICE server list"
    turn = []
    for server in ice:
        assert isinstance(server, dict), "invalid ICE server"
        urls = server.get("urls")
        urls = [urls] if isinstance(urls, str) else urls
        assert isinstance(urls, list) and urls and all(valid_ice_url(u) for u in urls), "invalid ICE URLs"
        if any(u.startswith(("turn:", "turns:")) for u in urls):
            turn.append(server)
            username = server.get("username")
            assert isinstance(username, str) and re.fullmatch(r"[0-9]{1,12}:[A-Za-z0-9]+", username), "invalid TURN expiry username"
            expires = int(username.split(":", 1)[0])
            assert now + 60 < expires <= now + 86400 + 60, "TURN credential expiry"
            assert isinstance(server.get("credential"), str) and server["credential"].strip(), "missing TURN credential"
    assert turn, "no TURN server"


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("output", type=Path)
    p.add_argument("--site", default="https://minaweb.org")
    p.add_argument("--relay", default="https://relay.minaweb.org")
    p.add_argument("--turn-ip", required=True)
    p.add_argument("--turn-port", type=int, default=3478)
    p.add_argument("--automated-only", action="store_true",
                   help="run HTTP, credential, STUN and native checks; browser sync remains a separate gate")
    args = p.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    code, headers, _ = get(args.site)
    assert code == 200 and headers.get("Cross-Origin-Opener-Policy") == "same-origin"
    assert headers.get("Cross-Origin-Embedder-Policy") == "require-corp"
    code, _, body = get(args.site + "/build-info.json")
    assert code == 200
    build = json.loads(body)
    assert build["network"] == "devnet" and build["wasm"]["pinnedInPage"]
    check_assets(build, args.site)
    for route in ("healthz", "readyz"):
        assert get(args.relay + "/" + route)[0] == 200
    for route in ("status", "graphql", "state", "metrics"):
        assert get(args.relay + "/" + route)[0] == 404
    code, _, _ = get(args.relay + "/mina/webrtc/signal/x")
    assert code == 400, f"signalling malformed-offer gate: {code}"
    code, _, _ = get(args.relay + "/mina/webrtc/signal", "OPTIONS", {
        "Origin": args.site, "Access-Control-Request-Method": "POST",
        "Access-Control-Request-Headers": "content-type"})
    assert code in (200, 204)
    # Credentials are deliberately neither logged nor saved in evidence.
    code, h, body = get(args.site + "/webnode-devnet/ice-servers.json")
    ice = json.loads(body)
    assert code == 200 and h.get("Cache-Control") == "no-store"
    check_credentials(ice)
    tx = secrets.token_bytes(12)
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.settimeout(8)
        sock.connect((args.turn_ip, args.turn_port))
        sock.send(struct.pack("!HHI", 1, 0, 0x2112A442) + tx)
        response = sock.recv(2048)
    assert len(response) >= 20 and response[8:20] == tx
    assert struct.unpack("!H", response[:2])[0] == 0x101
    native_url = os.environ["RELAY_HTTP"].rstrip("/")
    native = json.loads(get(native_url + "/status")[2])
    connected = [x for x in native["peers"] if x["connection_status"] == "Connected" and x["is_libp2p"]]
    assert native["network"] == "devnet" and connected, "no native devnet peer connections"
    assert native["transition_frontier"]["sync"]["status"] == "Synced"
    (out / "preflight.json").write_text(json.dumps({"build": build,
        "nativeConnectedPeers": len(connected), "stun": "passed"}, indent=2) + "\n")
    if args.automated_only:
        (out / "AUTOMATED-PASS").write_text(
            "HTTP assets, credential expiry, STUN and native peers passed; browser sync not run\n")
        return
    for mode in ("direct", "turn"):
        run = out / mode
        env = dict(os.environ, BASE=args.site, PAGE="/", AUTOSTART="1", OUT=str(run),
                   MIN_SYNC_HEIGHT=str(native["transition_frontier"]["best_tip"]["height"] - 1),
                   EVERY_MS="5000", UNTIL_SYNCED="30", ICE_POLICY="relay" if mode == "turn" else "")
        subprocess.run(["node", str(HERE.parents[1] / "regression/webnode/run.mjs"), "15"],
                       env=env, check=True)
        log = (run / "run.log").read_text()
        assert "crossOriginIsolated=true" in log
        statuses = [json.loads(s) for s in re.findall(r"status=(\{.*?\}) rtc=", log)]
        tip = json.loads(get(native_url + "/status")[2])["transition_frontier"]["best_tip"]["height"]
        assert any(s.get("sync") == "Synced" and s.get("peers", 0) > 0 and
                   abs(tip - (s.get("best_tip") or 0)) <= 1 for s in statuses), f"{mode}: no current synced tip"
        if mode == "turn":
            assert '"pairs":["relay>' in log, "TURN run never selected a relay candidate"
    (out / "PASS").write_text("HTTPS, STUN, native peers, direct sync and TURN sync passed\n")


if __name__ == "__main__":
    main()
